use std::{
    fs,
    io::{self, Write},
};

use anyhow::Result;
use log::info;

use super::Output;
use crate::config::Config;

pub struct File {
    file: fs::File,
}

impl Output for File {
    fn set_header(&mut self, header: &[u8]) -> io::Result<()> {
        self.file.write_all(header)
    }
}

impl Write for File {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        unreachable!();
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.file.write_all(buf)
    }
}

impl File {
    pub fn new() -> Result<Option<Self>> {
        let cfg = Config::get();

        let Some(path) = &cfg.record_path else {
            return Ok(None);
        };

        info!("Recording to: {path}");
        if cfg.overwrite {
            return Ok(Some(Self {
                file: fs::File::create(path)?,
            }));
        }

        Ok(Some(Self {
            file: fs::File::create_new(path)?,
        }))
    }
}
