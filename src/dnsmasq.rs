use anyhow::Result;
use nix::unistd::Pid;
use nix::sys::signal::{Signal, kill};

use crate::config::DnsmasqConfig;

pub fn restart_dnsmasq(cfg: &DnsmasqConfig) -> Result<()> {
    if !cfg.hup_after_refresh {
        return Ok(());
    }

    let pid_raw = std::fs::read(&cfg.pid_file)?;
    let pid = std::str::from_utf8(&pid_raw)?.trim().parse()?;

    log::info!("Sending SIGHUP to dnsmasq (pid {})", pid);
    kill(Pid::from_raw(pid), Signal::SIGHUP)?;

    Ok(())
}
