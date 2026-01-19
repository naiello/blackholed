use std::{sync::Arc, time::Duration};

use anyhow::{anyhow, Context, Result};
use hickory_server::{
    authority::Catalog,
    resolver::{
        config::{NameServerConfigGroup, ResolverOpts},
        Name,
    },
    store::forwarder::{ForwardAuthority, ForwardConfig},
    ServerFuture,
};
use tokio::net::{TcpSocket, UdpSocket};

use crate::blocklist::{BlocklistAuthority, BlocklistProvider};

pub struct ResolverConfig {
    pub port: u16,
    pub upstream: NameServerConfigGroup,
    pub cache_size: usize,
}

impl Default for ResolverConfig {
    fn default() -> Self {
        Self {
            port: 5353,
            upstream: NameServerConfigGroup::cloudflare_tls(),
            cache_size: 10000,
        }
    }
}

pub async fn start<BP: BlocklistProvider + Send + Sync + 'static>(
    config: ResolverConfig,
    blocklist: Arc<BlocklistAuthority<BP>>,
) -> Result<ServerFuture<Catalog>> {
    let mut opts = ResolverOpts::default();
    opts.edns0 = true;
    opts.cache_size = config.cache_size;

    let upstream_config = ForwardConfig {
        name_servers: config.upstream,
        options: Some(opts),
    };
    let upstream = ForwardAuthority::builder_tokio(upstream_config)
        .build()
        .map_err(|err| anyhow!("Forwarding authority did not build: {err}"))?;

    let mut catalog = Catalog::new();
    catalog.upsert(Name::root().into(), vec![blocklist, Arc::new(upstream)]);

    let addr = format!("0.0.0.0:{}", config.port)
        .parse()
        .context("Listen address did not parse")?;

    let udp = UdpSocket::bind(addr)
        .await
        .context("Could not bind UDP socket")?;

    let tcp = {
        let sock = TcpSocket::new_v4()?;
        sock.set_reuseaddr(true)?;
        sock.bind(addr)?;
        sock.listen(1024)?
    };

    let mut server = ServerFuture::new(catalog);
    server.register_socket(udp);
    server.register_listener(tcp, Duration::from_secs(5));

    log::info!("Server is listening on {addr}");

    Ok(server)
}
