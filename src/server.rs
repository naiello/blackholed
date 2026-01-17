use std::{str::FromStr, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use hickory_server::{
    authority::{AuthorityObject, Catalog},
    proto::rr::LowerName,
    resolver::{
        config::{NameServerConfigGroup, ResolverOpts},
        IntoName,
    },
    store::forwarder::{ForwardAuthority, ForwardConfig},
    ServerFuture,
};
use tokio::net::{TcpSocket, UdpSocket};

use crate::blocklist::{BlocklistAuthority, BlocklistConfig, BlocklistProvider};

pub struct ServerConfig {
    pub port: u16,
    pub upstream: NameServerConfigGroup,
    pub cache_size: usize,
}

pub async fn start_server(
    config: ServerConfig,
    provider: Arc<impl BlocklistProvider + Send + Sync + 'static>,
) -> Result<ServerFuture<Catalog>> {
    let mut opts = ResolverOpts::default();
    opts.edns0 = true;
    opts.cache_size = config.cache_size;

    let blocklist = Arc::new(
        BlocklistAuthority::new(".".into_name()?, &BlocklistConfig::default(), provider).await,
    );
    let upstream_config = ForwardConfig {
        name_servers: config.upstream,
        options: Some(opts),
    };
    let upstream = Arc::new(
        ForwardAuthority::builder_tokio(upstream_config)
            .build()
            .expect("Forwarding authority should build"),
    );
    let authorities: Vec<Arc<dyn AuthorityObject>> = vec![blocklist, upstream];
    let mut catalog = Catalog::new();
    catalog.upsert(
        LowerName::from_str(".").context("Root name should be valid")?,
        authorities,
    );
    let mut server = ServerFuture::new(catalog);

    let addr = format!("0.0.0.0:{}", config.port)
        .parse()
        .context("Listen address did not parse")?;

    let udp = UdpSocket::bind(addr)
        .await
        .context("Could not bind UDP socket")?;
    server.register_socket(udp);

    let tcp = TcpSocket::new_v4()?;
    tcp.set_reuseaddr(true)?;
    tcp.bind(addr)?;
    let listener = tcp.listen(1024)?;
    server.register_listener(listener, Duration::from_secs(5));

    log::info!("Server is listening on 0.0.0.0:5353");

    Ok(server)
}
