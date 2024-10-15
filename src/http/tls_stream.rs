use std::{
    io::{
        self,
        ErrorKind::{ConnectionReset, InvalidData, OutOfMemory},
        Read, Write,
    },
    net::TcpStream,
};

use anyhow::Result;
use rustls::{
    client::{ClientConnectionData, UnbufferedClientConnection},
    unbuffered::{ConnectionState, EncodeTlsData, UnbufferedStatus, WriteTraffic},
};

use super::Agent;

const OVERHEAD: usize = 22;
pub const TLS_MAX_FRAG_SIZE: usize = 16384 + OVERHEAD;

pub struct TlsStream {
    conn: UnbufferedClientConnection,
    sock: TcpStream,

    incoming: State,
    outgoing: State,

    sent_request: bool,
}

impl Read for TlsStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut read = 0;
        self.converse(None, Some((buf, &mut read)))?;

        Ok(read)
    }
}

impl Write for TlsStream {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        unreachable!();
    }

    fn flush(&mut self) -> io::Result<()> {
        self.sent_request = true;
        Ok(())
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        if buf.is_empty() {
            return Ok(());
        }
        self.sent_request = false;

        self.converse(Some(buf), None)
    }
}

impl TlsStream {
    const INCOMING_SIZE: usize = TLS_MAX_FRAG_SIZE;
    const OUTGOING_SIZE: usize = 2048;

    pub fn new(sock: TcpStream, host: &str, agent: &Agent) -> Result<Self> {
        Ok(Self {
            conn: UnbufferedClientConnection::new(
                agent.tls_config.clone(),
                host.to_owned().try_into()?,
            )?,
            sock,
            incoming: State::new(Self::INCOMING_SIZE),
            outgoing: State::new(Self::OUTGOING_SIZE),
            sent_request: bool::default(),
        })
    }

    fn converse(
        &mut self,
        read: Option<&[u8]>,
        mut write: Option<(&mut [u8], &mut usize)>,
    ) -> io::Result<()> {
        debug_assert!(read.is_some() || write.is_some());

        let mut completed_io = false;
        while !completed_io {
            let UnbufferedStatus { mut discard, state } =
                self.conn.process_tls_records(self.incoming.used_mut());

            match state.map_err(|e| io::Error::new(InvalidData, e))? {
                ConnectionState::ReadTraffic(mut state) => {
                    let Some((write, out_written)) = &mut write else {
                        continue;
                    };

                    let mut written = 0;
                    while let Some(res) = state.next_record() {
                        let record = res.map_err(|e| io::Error::new(InvalidData, e))?;

                        let end = written + record.payload.len();
                        if end > write.len() {
                            return Err(io::Error::from(OutOfMemory));
                        }

                        write[written..end].copy_from_slice(record.payload);

                        discard += record.discard;
                        written = end;
                    }

                    **out_written = written;
                    completed_io = true;
                }
                ConnectionState::WriteTraffic(may_encrypt) => {
                    if let (false, Some(read)) = (self.sent_request, read) {
                        self.outgoing.encrypt(read, may_encrypt)?;
                        self.outgoing.send(&mut self.sock)?;

                        completed_io = true;
                    } else {
                        self.incoming.recv(&mut self.sock)?;
                    }
                }
                ConnectionState::TransmitTlsData(mut state) => {
                    if let Some((may_encrypt, read)) = state.may_encrypt_app_data().zip(read) {
                        self.outgoing.encrypt(read, may_encrypt)?;
                        completed_io = true;
                    }

                    self.outgoing.send(&mut self.sock)?;
                    state.done();
                }
                ConnectionState::EncodeTlsData(state) => self.outgoing.encode(state)?,
                ConnectionState::BlockedHandshake => self.incoming.recv(&mut self.sock)?,
                ConnectionState::Closed => return Err(io::Error::from(ConnectionReset)),
                _ => unreachable!(),
            }

            if discard != 0 {
                self.incoming.discard(discard);
            }
        }

        Ok(())
    }
}

struct State {
    inner: Box<[u8]>,
    used: usize,
}

impl State {
    fn new(size: usize) -> Self {
        Self {
            inner: vec![0u8; size].into_boxed_slice(),
            used: usize::default(),
        }
    }

    fn unused_mut(&mut self) -> &mut [u8] {
        &mut self.inner[self.used..]
    }

    fn used(&self) -> &[u8] {
        &self.inner[..self.used]
    }

    fn used_mut(&mut self) -> &mut [u8] {
        &mut self.inner[..self.used]
    }

    fn send(&mut self, sock: &mut TcpStream) -> io::Result<()> {
        sock.write_all(self.used())?;
        self.used = 0;

        Ok(())
    }

    fn recv(&mut self, sock: &mut TcpStream) -> io::Result<()> {
        if self.used >= self.inner.len() {
            return Err(io::Error::from(OutOfMemory));
        }

        self.used += sock.read(self.unused_mut())?;
        Ok(())
    }

    fn encrypt(
        &mut self,
        buf: &[u8],
        mut may_encrypt: WriteTraffic<'_, ClientConnectionData>,
    ) -> io::Result<()> {
        self.used += may_encrypt
            .encrypt(buf, self.unused_mut())
            .map_err(|e| io::Error::new(OutOfMemory, e))?;

        Ok(())
    }

    fn encode(&mut self, mut state: EncodeTlsData<'_, ClientConnectionData>) -> io::Result<()> {
        self.used += state
            .encode(self.unused_mut())
            .map_err(|e| io::Error::new(OutOfMemory, e))?;

        Ok(())
    }

    fn discard(&mut self, size: usize) {
        self.inner.copy_within(size..self.used, 0);
        self.used -= size;
    }
}
