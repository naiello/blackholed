use std::path::Path;
use std::fs::{File, OpenOptions};
use std::io::LineWriter;
use std::io::Write;
use std::net::IpAddr;
use anyhow::Result;

pub trait HostsWriter {
    fn write(&mut self, hostname: &str) -> Result<()>;
}

pub struct FilesystemHostsWriter {
    writer: LineWriter<File>,
    blackhole_addr: IpAddr,
}

impl FilesystemHostsWriter {
    pub fn new(filename: &Path, blackhole_addr: &IpAddr) -> Result<Self> {
        let handle = OpenOptions::new()
            .write(true)
            .create(true)
            .open(filename)?;

        let writer = LineWriter::new(handle);

        Ok(FilesystemHostsWriter { writer, blackhole_addr: blackhole_addr.to_owned() })
    }
}

impl HostsWriter for FilesystemHostsWriter {
    fn write(&mut self, hostname: &str) -> Result<()> {
        let line = format!("{} {}\n", self.blackhole_addr, hostname);
        self.writer.write_all(line.as_bytes())?;
        Ok(())
    }
}

