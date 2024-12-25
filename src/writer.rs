use std::fs::{File, OpenOptions};
use std::io::LineWriter;
use std::io::Write;
use anyhow::Result;

use crate::config::Config;

pub trait HostsWriter {
    fn write_blocked(&mut self, hostname: &str) -> Result<()>;
    fn write_allowed(&mut self, hostname: &str) -> Result<()>;
}

pub struct FilesystemHostsWriter {
    writer: LineWriter<File>,
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
        };

        Ok(writer)
    }
}

impl HostsWriter for FilesystemHostsWriter {
    fn write_blocked(&mut self, hostname: &str) -> Result<()> {
        let line = format!("server=/{}/\n", hostname);
        self.writer.write_all(line.as_bytes())?;
        Ok(())
    }

    fn write_allowed(&mut self, hostname: &str) -> Result<()> {
        let line = format!("server=/{}/#\n", hostname);
        self.writer.write_all(line.as_bytes())?;
        Ok(())
    }
}

