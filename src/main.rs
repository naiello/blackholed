use futures::StreamExt;
use futures::stream::FuturesUnordered;
use anyhow::Result;
use simple_logger::SimpleLogger;

use blackholed::config::*;
use blackholed::loader::*;
use blackholed::parse::*;
use blackholed::writer::*;

#[tokio::main]
async fn main() -> Result<()> {
    SimpleLogger::new().with_level(log::LevelFilter::Info).init().expect("Logger did not initialize");

    let config = Config::load();
    let loader = WebLoader::new();
    let mut writer = FilesystemHostsWriter::new(
        &config.hosts_file, 
        &config.blackhole_address.into(),
    )?;

    let blocked_count = config.blocklists
        .iter()
        .map(|cfg| loader.load(cfg))
        .collect::<FuturesUnordered<_>>()
        .collect::<Vec<_>>()
        .await
        .iter()
        .flat_map(|content| content)
        .flat_map(|content| parse_blocklist(&content))
        .inspect(|host| writer.write(host).expect("Failed writing to hosts file"))
        .count();

    log::info!("blocked {} hosts", blocked_count);

    Ok(())
}
