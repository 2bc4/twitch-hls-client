use std::{
    fs::File,
    io::{self, Write},
};

use anyhow::Result;
use log::{debug, info};

use crate::args::{ArgParser, Parser};

#[derive(Default, Debug)]
pub struct Args {
    path: Option<String>,
    overwrite: bool,
}

impl ArgParser for Args {
    fn parse(&mut self, parser: &mut Parser) -> Result<()> {
        parser.parse_fn_cfg(&mut self.path, "-r", "record", Parser::parse_opt_string)?;
        parser.parse_switch(&mut self.overwrite, "--overwrite")?;

        Ok(())
    }
}

pub struct Recorder {
    file: File,
}

impl Write for Recorder {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        unimplemented!();
    }

    fn flush(&mut self) -> io::Result<()> {
        debug!("Finished writing segment");
        Ok(())
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.file.write_all(buf)
    }
}

impl Recorder {
    pub fn new(args: &Args) -> Result<Option<Self>> {
        let Some(ref path) = args.path else {
            return Ok(None);
        };

        info!("Recording to {path}");
        if args.overwrite {
            return Ok(Some(Self {
                file: File::create(path)?,
            }));
        }

        Ok(Some(Self {
            file: File::options().write(true).create_new(true).open(path)?,
        }))
    }
}
