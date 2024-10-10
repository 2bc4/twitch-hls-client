use std::{
    fs::File,
    io::{self, Write},
};

use anyhow::Result;
use log::info;

use crate::args::{Parse, Parser};

#[derive(Default, Debug)]
pub struct Args {
    path: Option<String>,
    overwrite: bool,
}

impl Parse for Args {
    fn parse(&mut self, parser: &mut Parser) -> Result<()> {
        parser.parse_opt_string_cfg(&mut self.path, "-r", "record")?;
        parser.parse_switch(&mut self.overwrite, "--overwrite")?;

        Ok(())
    }
}

pub struct Recorder {
    file: File,
}

impl Write for Recorder {
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

impl Recorder {
    pub fn new(args: &Args) -> Result<Option<Self>> {
        let Some(path) = &args.path else {
            return Ok(None);
        };

        info!("Recording to: {path}");
        if args.overwrite {
            return Ok(Some(Self {
                file: File::create(path)?,
            }));
        }

        Ok(Some(Self {
            file: File::create_new(path)?,
        }))
    }
}
