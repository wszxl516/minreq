use indicatif::{ProgressBar, ProgressStyle};
fn main() -> anyhow::Result<()> {
    env_logger::Builder::new()
        .format(|buf, record| {
            use std::io::Write;
            writeln!(
                buf,
                "{}:{} {} [{}] - {}",
                record.file().unwrap_or("unknown"),
                record.line().unwrap_or(0),
                chrono::Local::now().format("%Y-%m-%dT%H:%M:%S"),
                record.level(),
                record.args()
            )
        })
        // .filter(Some("minreq"), log::LevelFilter::Debug)
        .filter_level(log::LevelFilter::Debug)
        .init();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let pb = ProgressBar::new(0);
    pb.set_style(ProgressStyle::with_template("{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})")
        .unwrap()
        .progress_chars("#>-"));
    let p = pb.clone();
    let response = rt.block_on(async move {
        let mut fd = tokio::fs::File::options()
            .create(true)
            .write(true)
            .open("index.html")
            .await
            .unwrap();
        minreq::get("https://dl-cdn.alpinelinux.org/alpine/v3.21/releases/x86_64/alpine-standard-3.21.2-x86_64.iso")
            .with_redirect(false)
            .send_with_stream(&mut fd, |t, c| {
                p.set_length(t);
                p.set_position(c);
            })
            .await
    });
    pb.finish_with_message("downloaded");
    println!("{}", response?);
    Ok(())
}
