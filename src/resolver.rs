use std::{sync::Arc, time::Duration};

use anyhow::{anyhow, Context, Result};
use hickory_server::{
    authority::{Catalog, ZoneType},
    resolver::{
        config::{ResolveHosts, ResolverOpts},
        Name,
    },
    store::{
        file::{FileAuthority, FileConfig},
        forwarder::{ForwardAuthority, ForwardConfig},
    },
    ServerFuture,
};
use tokio::net::{TcpSocket, UdpSocket};
use tokio::select;
use tokio_graceful::ShutdownGuard;
use tokio_util::task::AbortOnDropHandle;

use crate::{
    blocklist::{BlocklistAuthority, BlocklistProvider},
    config::ResolverConfig,
    types::Shared,
};

pub struct Resolver {
    _handle: AbortOnDropHandle<()>,
}

impl Resolver {
    pub async fn new<BP: BlocklistProvider + Shared>(
        config: ResolverConfig,
        blocklist: Arc<BlocklistAuthority<BP>>,
        shutdown: ShutdownGuard,
    ) -> Result<Resolver> {
        let mut opts = ResolverOpts::default();
        opts.edns0 = true;
        opts.cache_size = config.cache_size;
        opts.use_hosts_file = ResolveHosts::Always;

        let upstream_config = ForwardConfig {
            name_servers: config.upstream.to_nameserver_config_group()?,
            options: Some(opts),
        };
        let upstream = ForwardAuthority::builder_tokio(upstream_config)
            .build()
            .map_err(|err| anyhow!("Forwarding authority did not build: {err}"))?;

        let mut catalog = Catalog::new();

        // Load file-based zones
        for zone_config in config.zones {
            let zone_name = Name::from_utf8(&zone_config.name)
                .with_context(|| format!("Invalid zone name: {}", zone_config.name))?;

            let file_config = FileConfig {
                zone_file_path: zone_config.file.clone(),
            };

            let authority = FileAuthority::try_from_config(
                zone_name.clone(),
                ZoneType::Primary,
                false, // allow_axfr
                None,  // root_dir
                &file_config,
            )
            .map_err(|err| {
                anyhow!(
                    "Failed to load zone {} from {:?}: {}",
                    zone_config.name,
                    zone_config.file,
                    err
                )
            })?;

            catalog.upsert(zone_name.into(), vec![Arc::new(authority)]);
            log::info!(
                "Loaded zone {} from {:?}",
                zone_config.name,
                zone_config.file
            );
        }

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

        let handle = shutdown.spawn_task_fn(|guard| async move {
            select! {
                result = server.block_until_done() => {
                    match result {
                        Ok(_) => log::error!("DNS Resolver exited unexpectedly"),
                        Err(err) => log::error!("DNS Resolver exited unexpectedly: {err}"),
                    }
                },
                _ = guard.cancelled() => {
                    log::info!("DNS Resolver shutting down");
                    if let Err(err) = server.shutdown_gracefully().await { log::error!("Error while stopping DNS resolver: {err}") }
                },
            }
        });

        Ok(Self {
            _handle: AbortOnDropHandle::new(handle),
        })
    }
}
