use crate::request::ParsedRequest;
use crate::{Error, Method, ResponseLazy};
use std::io;
use tokio::net::TcpStream;
use std::net::ToSocketAddrs;
use tokio::io::AsyncWriteExt;

#[cfg(feature = "https")]
use rustls::{self, ClientConfig, RootCertStore, pki_types::ServerName};
#[cfg(feature = "https")]
use tokio_rustls::client::TlsStream;
#[cfg(feature = "https")]
use webpki_roots::TLS_SERVER_ROOTS;
#[cfg(feature = "https")]
static CONFIG: once_cell::sync::Lazy<std::sync::Arc<ClientConfig>> = once_cell::sync::Lazy::new(|| {
    rustls::crypto::ring::default_provider().install_default().unwrap();
    let mut root_certificates = RootCertStore::empty();
    root_certificates.roots.extend(TLS_SERVER_ROOTS.to_vec());
    let config = ClientConfig::builder()
        .with_root_certificates(root_certificates)
        .with_no_client_auth();
    std::sync::Arc::new(config)
});

type UnsecuredStream = TcpStream;
#[cfg(feature = "https")]
type SecuredStream = TlsStream<TcpStream>;

pub enum HttpStream {
    Unsecured(UnsecuredStream),
    #[cfg(feature = "https")]
    Secured(Box<SecuredStream>),
}

impl HttpStream {
    fn create_unsecured(reader: UnsecuredStream) -> HttpStream {
        HttpStream::Unsecured(reader)
    }
    #[cfg(feature = "https")]
    fn create_secured(reader: SecuredStream) -> HttpStream {
        HttpStream::Secured(Box::new(reader))
    }
}


impl tokio::io::AsyncRead for HttpStream {
    fn poll_read(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<io::Result<()>> {
        match self.get_mut() {
            #[cfg(feature = "https")]
            HttpStream::Secured(stream) => {
                let pinned = std::pin::pin!(stream);
                pinned.poll_read(cx, buf)
            }
            HttpStream::Unsecured(stream) => {
                let pinned = std::pin::pin!(stream);
                pinned.poll_read(cx, buf)
            }
        }
    }
}

/// A connection to the server for sending
/// [`Request`](struct.Request.html)s.
pub struct Connection {
    request: ParsedRequest,
}

impl Connection {
    /// Creates a new `Connection`. See [Request] and [ParsedRequest]
    /// for specifics about *what* is being sent.
    pub(crate) fn new(request: ParsedRequest) -> Connection {
        Connection {
            request,
        }
    }

    /// Sends the [`Request`](struct.Request.html), consumes this
    /// connection, and returns a [`Response`](struct.Response.html).
    #[cfg(feature = "https")]
    pub async fn send_https(self) -> Result<ResponseLazy, Error> {
        let bytes = self.request.as_bytes();

        // Rustls setup
        log::trace!("Setting up TLS parameters for {}.", self.request.url.host);
        let dns_name = match ServerName::try_from(self.request.url.host.clone()) {
            Ok(result) => result,
            Err(err) => return Err(Error::IoError(io::Error::new(io::ErrorKind::Other, err))),
        };

        log::trace!("Establishing TCP connection to {}.", self.request.url.host);
        let tcp = self.connect().await?;
        let connector = tokio_rustls::TlsConnector::from(CONFIG.clone());
        let mut tls = connector.connect(dns_name, tcp).await?;
        // Send request
        log::trace!("Establishing TLS session to {}.", self.request.url.host);
        log::trace!("Writing HTTPS request, {:?}", String::from_utf8_lossy(&bytes));
        tls.write_all(&bytes).await?;

        // Receive request
        log::trace!("Reading HTTPS response");
        let response = ResponseLazy::from_stream(
            HttpStream::create_secured(tls),
            self.request.config.max_headers_size,
            self.request.config.max_status_line_len,
        ).await?;
        handle_redirects(self, response).await.await
    }


    /// Sends the [`Request`](struct.Request.html), consumes this
    /// connection, and returns a [`Response`](struct.Response.html).
    pub async fn send(self) -> Result<ResponseLazy, Error> {

        let bytes = self.request.as_bytes();

        log::trace!("Establishing TCP connection to {}.", self.request.url.host);
        let mut tcp = self.connect().await?;

        // Send request
        log::trace!("Writing HTTP request, {:?}", String::from_utf8_lossy(&bytes));
        tcp.write_all(&bytes).await?;

        // Receive response
        log::trace!("Reading HTTP response.");
        let stream = HttpStream::create_unsecured(tcp);
        let response = ResponseLazy::from_stream(
            stream,
            self.request.config.max_headers_size,
            self.request.config.max_status_line_len,
        ).await?;
        handle_redirects(self, response).await.await
    }

    async fn connect(&self) -> Result<TcpStream, Error> {
        let tcp_connect = async|host: &str, port: u32| -> Result<TcpStream, Error> {
            let addrs = (host, port as u16)
                .to_socket_addrs()
                .map_err(Error::IoError)?;
            let addrs_count = addrs.len();

            // Try all resolved addresses. Return the first one to which we could connect. If all
            // failed return the last error encountered.
            for (i, addr) in addrs.enumerate() {
                let stream =  tokio::net::TcpStream::connect(addr).await;
                if stream.is_ok() || i == addrs_count - 1 {
                    return stream.map_err(Error::from);
                }
            }

            Err(Error::AddressNotFound)
        };

        #[cfg(feature = "proxy")]
        match self.request.config.proxy {
            Some(ref proxy) => {
                use tokio::io::AsyncReadExt;
                // do proxy things
                let mut tcp = tcp_connect(&proxy.server, proxy.port).await?;

                tcp.write_all(format!("{}", proxy.connect(&self.request)).as_bytes()).await?;
                tcp.flush().await?;

                let mut proxy_response = Vec::new();

                loop {
                    let mut buf = vec![0; 256];
                    let total = tcp.read(&mut buf).await?;
                    proxy_response.append(&mut buf);
                    if total < 256 {
                        break;
                    }
                }

                crate::Proxy::verify_response(&proxy_response)?;

                Ok(tcp)
            }
            None => tcp_connect(&self.request.url.host, self.request.url.port.port()).await,
        }

        #[cfg(not(feature = "proxy"))]
        tcp_connect(&self.request.url.host, self.request.url.port.port()).await
    }
}

async fn handle_redirects(
    connection: Connection,
    mut response: ResponseLazy,
) -> std::pin::Pin<Box<dyn Future<Output = Result<ResponseLazy, Error>>>> {
    Box::pin(async move{
        let status_code = response.status_code;
        let url = response.headers.get("location");
        match get_redirect(connection, status_code, url) {
            NextHop::Redirect(connection) => {
                log::trace!("Redirecting response headers: {:?}", response.headers);

                let connection = connection?;
                if connection.request.url.https {
                    #[cfg(feature = "https")]
                    return connection.send_https().await;
                    #[cfg(not(feature = "https"))]
                    return Err(Error::HttpsFeatureNotEnabled);
                } else {
                    connection.send().await
                }
            }
            NextHop::Destination(connection) => {
                let dst_url = connection.request.url;
                dst_url.write_base_url_to(&mut response.url).unwrap();
                dst_url.write_resource_to(&mut response.url).unwrap();
                log::trace!("Response headers: {:?}", response.headers);
                Ok(response)
            }
        }
    })

}

enum NextHop {
    Redirect(Result<Connection, Error>),
    Destination(Connection),
}

fn get_redirect(mut connection: Connection, status_code: i32, url: Option<&String>) -> NextHop {
    if !connection.request.config.redirect {
        return NextHop::Destination(connection)
    }
    match status_code {
        301 | 302 | 303 | 307 => {
            let url = match url {
                Some(url) => url,
                None => return NextHop::Redirect(Err(Error::RedirectLocationMissing)),
            };
            log::debug!("Redirecting ({}) to: {}", status_code, url);

            match connection.request.redirect_to(url.as_str()) {
                Ok(()) => {
                    if status_code == 303 {
                        match connection.request.config.method {
                            Method::Post | Method::Put | Method::Delete => {
                                connection.request.config.method = Method::Get;
                            }
                            _ => {}
                        }
                    }

                    NextHop::Redirect(Ok(connection))
                }
                Err(err) => NextHop::Redirect(Err(err)),
            }
        }
        _ => NextHop::Destination(connection),
    }
}
