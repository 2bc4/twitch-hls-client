mod file;
mod player;
mod tcp;

pub use player::{Player, PlayerClosedError};

use std::io::{self, Write};

use anyhow::{Result, ensure};
use log::{debug, info};

use file::{Args as FileArgs, File};
use player::Args as PlayerArgs;
use tcp::{Args as TcpArgs, Tcp};

use crate::args::{Parse, Parser};

pub trait Output: Write + Send {
    fn set_header(&mut self, header: &[u8]) -> io::Result<()>;

    fn should_wait(&self) -> bool {
        false
    }

    fn wait_for_output(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Default, Debug)]
pub struct Args {
    pub player: PlayerArgs,
    tcp: TcpArgs,
    file: FileArgs,
}

impl Parse for Args {
    fn parse(&mut self, parser: &mut Parser) -> Result<()> {
        self.player.parse(parser)?;
        self.tcp.parse(parser)?;
        self.file.parse(parser)?;

        Ok(())
    }
}

#[derive(Default)]
pub struct Writer {
    outputs: Vec<Box<dyn Output>>,
}

impl Output for Writer {
    fn set_header(&mut self, header: &[u8]) -> io::Result<()> {
        debug!("Outputting segment header");
        self.handle_outputs(|output| output.set_header(header))
    }

    fn should_wait(&self) -> bool {
        if self.outputs.len() == 1
            && let Some(output) = self.outputs.first()
        {
            return output.should_wait();
        }

        false
    }

    fn wait_for_output(&mut self) -> io::Result<()> {
        info!("Waiting for outputs...");
        for output in &mut self.outputs {
            output.wait_for_output()?;
        }

        Ok(())
    }
}

impl Write for Writer {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        unreachable!();
    }

    fn flush(&mut self) -> io::Result<()> {
        self.handle_outputs(Write::flush)?;

        debug!("Finished writing segment");
        Ok(())
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.handle_outputs(|output| output.write_all(buf))
    }
}

impl Writer {
    pub fn new(args: &Args, channel: &str) -> Result<Self> {
        let mut writer = Self::default();

        writer.add_output(Player::new(&args.player, channel)?);
        writer.add_output(Tcp::new(&args.tcp)?);
        writer.add_output(File::new(&args.file)?);

        ensure!(!writer.outputs.is_empty(), "No output configured");

        Ok(writer)
    }

    fn add_output(&mut self, output: Option<impl Output + 'static>) {
        if let Some(output) = output {
            self.outputs.push(Box::new(output));
        }
    }

    fn handle_outputs<F>(&mut self, mut f: F) -> io::Result<()>
    where
        F: FnMut(&mut Box<dyn Output>) -> io::Result<()>,
    {
        let has_multiple = self.outputs.len() > 1;

        let mut result = Ok(());
        self.outputs.retain_mut(|output| {
            if let Err(error) = f(output) {
                //Allow player to close without exiting program when there's multiple outputs
                #[allow(clippy::redundant_closure_for_method_calls)] //no
                if !(has_multiple && error.get_ref().is_some_and(|e| e.is::<PlayerClosedError>())) {
                    result = Err(error);
                }

                return false;
            }

            true
        });
        debug_assert!(!self.outputs.is_empty() || self.outputs.is_empty() && result.is_err());

        result
    }
}
