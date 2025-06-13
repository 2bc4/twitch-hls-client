use std::{
    io::{self, ErrorKind, Write},
    mem,
    net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs},
    sync::{
        Arc,
        mpsc::{self, Sender}, //change to mpmc when stabilized
    },
    thread::Builder as ThreadBuilder,
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
            client_timeout: Duration::from_secs(30),
            addr: Option::default(),
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
    client_timeout: Duration,
    state: State,
    header: Option<Arc<[u8]>>,
}

impl Output for Tcp {
    fn set_header(&mut self, header: &[u8]) -> io::Result<()> {
        self.header = Some(header.into());
        Ok(())
    }

    fn should_wait(&self) -> bool {
        matches!(self.state, State::Paused)
    }

    fn wait_for_output(&mut self) -> io::Result<()> {
        self.listener.set_nonblocking(false)?;
        self.accept()
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
        match &mut self.state {
            State::Paused => (),
            State::SingleThreaded(client) => {
                if !client.send(buf) {
                    self.state = State::Paused;
                }
            }
            State::MultiThreaded(threads) => {
                let data: Arc<[u8]> = buf.into();
                threads.retain_mut(|thread| thread.send(data.clone()));

                if threads.is_empty() {
                    self.state = State::Paused;
                }
            }
        }

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
            client_timeout: args.client_timeout,
            state: State::default(),
            header: Option::default(),
        }))
    }

    fn accept(&mut self) -> io::Result<()> {
        for incoming in self.listener.incoming() {
            match incoming {
                Ok(sock) => {
                    let mut client = Client::new(sock, self.client_timeout)?;

                    if let Some(header) = &self.header {
                        if !client.send(&header.clone()) {
                            return Ok(());
                        }
                    }

                    match &mut self.state {
                        State::Paused => self.state = State::SingleThreaded(client),
                        State::SingleThreaded(first) => {
                            self.state = State::MultiThreaded(vec![
                                ClientThread::spawn(mem::take(first))?,
                                ClientThread::spawn(client)?,
                            ]);
                        }
                        State::MultiThreaded(threads) => {
                            threads.push(ClientThread::spawn(client)?);
                        }
                    }

                    self.listener.set_nonblocking(true)?;
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) => error!("Failed to accept TCP client: {e}"),
            }
        }

        Ok(())
    }
}

#[derive(Default)]
enum State {
    #[default]
    Paused,

    SingleThreaded(Client),
    MultiThreaded(Vec<ClientThread>),
}

#[derive(Default)]
struct Client {
    sock: Option<TcpStream>,
    addr: Option<SocketAddr>,
}

impl Client {
    fn new(sock: TcpStream, timeout: Duration) -> io::Result<Self> {
        let addr = sock.peer_addr()?;
        info!("Client accepted: {addr}");

        sock.set_nodelay(true)?;
        sock.set_write_timeout(Some(timeout))?;

        Ok(Self {
            sock: Some(sock),
            addr: Some(addr),
        })
    }

    fn send(&mut self, data: &[u8]) -> bool {
        match self
            .sock
            .as_mut()
            .expect("Missing client socket")
            .write_all(data)
        {
            Ok(()) => true,
            Err(e) => {
                let addr = self.addr.as_ref().expect("Missing client address");
                match e.kind() {
                    ErrorKind::BrokenPipe
                    | ErrorKind::ConnectionReset
                    | ErrorKind::ConnectionAborted => info!("Client disconnected: {addr}"),
                    ErrorKind::WouldBlock => info!("Client dropped (timed out): {addr}"),
                    _ => info!("Client dropped (write error: {e}): {addr}"),
                }

                false
            }
        }
    }
}

struct ClientThread {
    sender: Sender<Arc<[u8]>>,
}

impl ClientThread {
    fn spawn(mut client: Client) -> io::Result<Self> {
        let (sender, receiver) = mpsc::channel::<Arc<[u8]>>();
        ThreadBuilder::new()
            .name("tcp client".to_owned())
            .spawn(move || {
                loop {
                    let Ok(data) = receiver.recv() else {
                        return;
                    };

                    if !client.send(&data) {
                        return;
                    }
                }
            })
            .map_err(|e| io::Error::other(format!("Failed to spawn TCP client thread: {e}")))?;

        Ok(Self { sender })
    }

    fn send(&self, data: Arc<[u8]>) -> bool {
        self.sender.send(data).is_ok()
    }
}
