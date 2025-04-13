use std::{
    io::{self, ErrorKind, Write},
    net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs},
};

use anyhow::{Context, Result, bail};
use log::{error, info};

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

struct Client {
    sock: TcpStream,
    addr: SocketAddr,
    has_written: bool,
}

pub struct Tcp {
    listener: TcpListener,
    clients: Vec<Client>,
}

impl Write for Tcp {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        unreachable!();
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        match self.listener.accept() {
            Ok((sock, addr)) => {
                info!("Connection accepted: {addr}");

                sock.set_nodelay(true)?;
                self.clients.push(Client {
                    sock,
                    addr,
                    has_written: false,
                });
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => (),
            Err(e) => error!("Failed to accept connection: {e}"),
        }

        self.clients.retain_mut(|client| {
            //Check for the MPEG-TS header to ensure we don't start writing in the middle of a packet
            if !client.has_written {
                //Sync byte == 0x47 -> PUSI bit == 1 -> PAT bit == 0
                if buf[0] != 0x47
                    || (buf[1] & 0x40) >> 6 != 1
                    || ((u16::from(buf[1]) & 0x1F) << 8) | u16::from(buf[2]) != 0x0000
                {
                    return true;
                }
            }

            match client.sock.write_all(buf) {
                Ok(()) => {
                    client.has_written = true;
                    true
                }
                Err(e) => match e.kind() {
                    ErrorKind::BrokenPipe
                    | ErrorKind::ConnectionReset
                    | ErrorKind::ConnectionAborted => {
                        info!("Connection closed: {}", client.addr);
                        false
                    }
                    _ => {
                        error!("Failed to write to TCP client: {e}");
                        false
                    }
                },
            }
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
        }))
    }
}
