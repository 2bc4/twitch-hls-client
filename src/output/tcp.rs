use std::{
    io::{self, ErrorKind, Write},
    net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs},
};

use anyhow::{Context, Result, bail};
use log::{error, info};

use super::Output;
use crate::args::{Parse, Parser};

#[derive(Default, Debug)]
pub struct Args {
    addr: Option<SocketAddr>,
}

impl Parse for Args {
    fn parse(&mut self, parser: &mut Parser) -> Result<()> {
        parser.parse_fn_cfg(&mut self.addr, "-t", "tcp-server", |arg| {
            match arg.to_socket_addrs()?.next() {
                Some(addr) => Ok(Some(addr)),
                None => bail!("Invalid socket address: {arg}"),
            }
        })?;

        Ok(())
    }
}

pub struct Tcp {
    listener: TcpListener,
    clients: Vec<(TcpStream, SocketAddr)>,

    header: Option<Box<[u8]>>,
}

impl Output for Tcp {
    fn set_header(&mut self, header: &[u8]) -> io::Result<()> {
        self.header = Some(header.into());
        Ok(())
    }
}

impl Write for Tcp {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        unreachable!();
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.listener.accept() {
            Ok((mut sock, addr)) => {
                info!("Connection accepted: {addr}");

                sock.set_nodelay(true)?;
                if let Some(header) = &self.header {
                    sock.write_all(header)?;
                }

                self.clients.push((sock, addr));
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => (),
            Err(e) => error!("Failed to accept connection: {e}"),
        }

        Ok(())
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.clients
            .retain_mut(|(sock, addr)| match sock.write_all(buf) {
                Ok(()) => true,
                Err(e) => match e.kind() {
                    ErrorKind::BrokenPipe
                    | ErrorKind::ConnectionReset
                    | ErrorKind::ConnectionAborted => {
                        info!("Connection closed: {addr}");
                        false
                    }
                    _ => {
                        error!("Failed to write to TCP client: {e}");
                        false
                    }
                },
            });

        Ok(())
    }
}

impl Tcp {
    pub fn new(args: &Args) -> Result<Option<Self>> {
        let Some(addr) = &args.addr else {
            return Ok(None);
        };

        let listener = TcpListener::bind(addr).context("Failed to bind to address/port")?;
        listener.set_nonblocking(true)?;

        info!("Listening on: {addr}");
        Ok(Some(Self {
            listener,
            clients: Vec::default(),
            header: Option::default(),
        }))
    }
}
