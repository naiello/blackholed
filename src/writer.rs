use std::fs::{File, OpenOptions};
use std::io::LineWriter;
use std::io::Write;
use std::net::{Ipv4Addr, Ipv6Addr};
use anyhow::Result;

use crate::config::Config;

pub trait HostsWriter {
    fn write(&mut self, hostname: &str) -> Result<()>;
}

pub struct FilesystemHostsWriter {
    writer: LineWriter<File>,
    blackhole_addr: Ipv4Addr,
    blackhole_addr_v6: Ipv6Addr,
    blackhole_v6: bool,
}

impl FilesystemHostsWriter {
    pub fn new(cfg: &Config) -> Result<Self> {
        let handle = OpenOptions::new()
            .write(true)
            .create(true)
            .open(&cfg.hosts_file)?;

        let line_writer = LineWriter::new(handle);

        let writer = FilesystemHostsWriter {
            writer: line_writer,
            blackhole_addr: cfg.blackhole_address,
            blackhole_addr_v6: cfg.blackhole_address_v6,
            blackhole_v6: cfg.blackhole_v6,
        };

        Ok(writer)
    }
}

impl HostsWriter for FilesystemHostsWriter {
    fn write(&mut self, hostname: &str) -> Result<()> {
        let line = format!("{} {}\n", self.blackhole_addr, hostname);

        self.writer.write_all(line.as_bytes())?;
        if self.blackhole_v6 {
            let line_v6 = format!("{} {}\n", self.blackhole_addr_v6, hostname);
            self.writer.write_all(line_v6.as_bytes())?;
        }

        Ok(())
    }
}

