use std::{
    io::{self, ErrorKind, Write},
    net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs},
    ops::Deref,
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender}, //change to mpmc when stabilized
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use log::{error, info};

use super::Output;
use crate::args::{Parse, Parser};

#[derive(Debug)]
pub struct Args {
    addr: Option<SocketAddr>,
    client_timeout: Duration,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            addr: Option::default(),
            client_timeout: Duration::from_secs(30),
        }
    }
}

impl Parse for Args {
    fn parse(&mut self, parser: &mut Parser) -> Result<()> {
        parser.parse_fn_cfg(&mut self.addr, "-t", "tcp-server", |arg| {
            match arg.to_socket_addrs()?.next() {
                Some(addr) => Ok(Some(addr)),
                None => bail!("Invalid socket address: {arg}"),
            }
        })?;
        parser.parse_duration(&mut self.client_timeout, "--tcp-client-timeout")?;

        Ok(())
    }
}

pub struct Tcp {
    listener: TcpListener,
    clients: Vec<Client>,
    client_timeout: Duration,

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
        let data: ClientData = buf.into();
        self.clients.retain_mut(|client| client.send(data.clone()));

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
            client_timeout: args.client_timeout,
            header: Option::default(),
        }))
    }

    fn accept(&mut self) -> io::Result<()> {
        match self.listener.accept() {
            Ok((sock, addr)) => {
                info!("Client accepted: {addr}");

                let client = Client::new(sock, addr, self.client_timeout)?;
                if let Some(header) = &self.header {
                    if !client.send(header.into()) {
                        return Ok(());
                    }
                }

                self.clients.push(client);
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => (),
            Err(e) => error!("Failed to accept client: {e}"),
        }

        Ok(())
    }
}

struct Client {
    handle: JoinHandle<()>,
    sender: Sender<ClientData>,
}

impl Client {
    fn new(mut sock: TcpStream, addr: SocketAddr, timeout: Duration) -> io::Result<Self> {
        sock.set_nodelay(true)?;
        sock.set_write_timeout(Some(timeout))?;

        let (sender, receiver): (Sender<ClientData>, Receiver<ClientData>) = mpsc::channel();
        let handle = thread::Builder::new()
            .name("tcp client".to_owned())
            .spawn(move || {
                loop {
                    let Ok(data) = receiver.recv() else {
                        return;
                    };

                    if let Err(e) = sock.write_all(&data) {
                        match e.kind() {
                            ErrorKind::BrokenPipe
                            | ErrorKind::ConnectionReset
                            | ErrorKind::ConnectionAborted => info!("Client disconnected: {addr}"),
                            ErrorKind::WouldBlock => info!("Client dropped (timed out): {addr}"),
                            _ => info!("Client dropped (write error: {e}): {addr}"),
                        }

                        return;
                    }
                }
            })
            .map_err(|_| io::Error::other("Failed to spawn TCP client thread"))?;

        Ok(Self { handle, sender })
    }

    fn send(&self, buf: ClientData) -> bool {
        !self.handle.is_finished() && self.sender.send(buf).is_ok()
    }
}

#[derive(Clone)]
struct ClientData(Arc<Box<[u8]>>);

impl From<&[u8]> for ClientData {
    fn from(data: &[u8]) -> Self {
        Self(Arc::new(data.into()))
    }
}

impl From<&Box<[u8]>> for ClientData {
    fn from(data: &Box<[u8]>) -> Self {
        Self(Arc::new(data.clone()))
    }
}

impl Deref for ClientData {
    type Target = Arc<Box<[u8]>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
