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
    let response = rt.block_on(
      async move{
        let mut fd = tokio::fs::File::options().create(true).write(true).open("index.html").await.unwrap();
        minreq::get("http://127.0.0.1:5000/target/release/control-gui")
            .with_redirect(false)
            .send_with_stream(&mut fd, |t,c|println!("{}/{}", c, t))
            .await
      }
    );

    println!("{}", response?);
    Ok(())
}
