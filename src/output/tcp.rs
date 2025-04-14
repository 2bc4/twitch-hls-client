use std::{
    io::{self, ErrorKind, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs},
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
    clients: Vec<Client>,

    header: Option<Box<[u8]>>,
}

impl Output for Tcp {
    fn set_header(&mut self, header: &[u8]) -> io::Result<()> {
        self.header = Some(header.into());
        Ok(())
    }

    fn should_wait(&self) -> bool {
        self.clients.is_empty()
    }

    fn wait_for_output(&mut self) -> io::Result<()> {
        self.listener.set_nonblocking(false)?;
        self.accept()?;

        self.listener.set_nonblocking(true)
    }
}

impl Write for Tcp {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        unreachable!();
    }

    fn flush(&mut self) -> io::Result<()> {
        self.accept()
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.clients
            .retain_mut(|client| client.write_all(buf).is_ok());

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

    fn accept(&mut self) -> io::Result<()> {
        match self.listener.accept() {
            Ok((sock, addr)) => {
                info!("Connection accepted: {addr}");

                let mut client = Client::new(sock, addr)?;
                if let Some(header) = &self.header {
                    if client.write_all(header).is_err() {
                        return Ok(());
                    }
                }

                self.clients.push(client);
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => (),
            Err(e) => error!("Failed to accept connection: {e}"),
        }

        Ok(())
    }
}

struct Client {
    sock: TcpStream,
    addr: SocketAddr,
}

impl Write for Client {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        unreachable!();
    }

    fn flush(&mut self) -> io::Result<()> {
        unreachable!();
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        if let Err(e) = self.sock.write_all(buf) {
            match e.kind() {
                ErrorKind::BrokenPipe
                | ErrorKind::ConnectionReset
                | ErrorKind::ConnectionAborted => info!("Connection closed: {}", self.addr),
                _ => error!("Failed to write to TCP client: {e}"),
            }

            self.sock.shutdown(Shutdown::Both)?;
            return Err(io::Error::from(ErrorKind::BrokenPipe));
        }

        Ok(())
    }
}

impl Client {
    pub fn new(sock: TcpStream, addr: SocketAddr) -> io::Result<Self> {
        sock.set_nodelay(true)?;
        Ok(Self { sock, addr })
    }
}
