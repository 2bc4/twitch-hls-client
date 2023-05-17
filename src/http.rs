//    Copyright (C) 2023 2bc4
//
//    This program is free software: you can redistribute it and/or modify
//    it under the terms of the GNU General Public License as published by
//    the Free Software Foundation, either version 3 of the License, or
//    (at your option) any later version.
//
//    This program is distributed in the hope that it will be useful,
//    but WITHOUT ANY WARRANTY; without even the implied warranty of
//    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
//    GNU General Public License for more details.
//
//    You should have received a copy of the GNU General Public License
//    along with this program.  If not, see <https://www.gnu.org/licenses/>.

use std::{
    fmt,
    io::{
        self, BufRead, BufReader,
        ErrorKind::{ConnectionAborted, ConnectionReset, UnexpectedEof},
        Read, Write,
    },
    net::TcpStream,
};

use anyhow::{bail, Context, Result};
use chunked_transfer::Decoder as ChunkDecoder;
use flate2::read::GzDecoder;
use httparse::{Header, Response, Status, EMPTY_HEADER};
use log::debug;
use url::Url;

#[derive(Debug)]
pub enum Error {
    Status(u16, String),
}

impl std::error::Error for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Status(code, url) => write!(f, "Status code {code} on {url}"),
        }
    }
}

type Stream = BufReader<Transport>;

pub struct Request {
    stream: Stream,
    request: String,
    url: Url,
}

impl Request {
    pub fn get(url: Url) -> Result<Self> {
        Ok(Self {
            stream: BufReader::new(Transport::new(&url)?),
            request: Self::format_request(&url)?,
            url,
        })
    }

    pub fn get_with_header(url: &str, header: &str) -> Result<Self> {
        let mut r = Self::get(url.parse()?)?;

        //Before end of headers.
        //Will be overwritten if set_url is called but this is only needed for the TTVLOL API.
        r.request.insert_str(r.request.len() - 2, header);
        r.request += "\r\n";
        Ok(r)
    }

    pub fn reader(&mut self) -> Result<Decoder> {
        const MAX_HEADERS: usize = 16;

        let buf = match self.do_io() {
            Ok(buf) => buf,
            Err(e) => match e.kind() {
                ConnectionReset | ConnectionAborted | UnexpectedEof => {
                    debug!("Connection reset/EOF, reconnecting");
                    self.reconnect(None)?;
                    self.do_io()? //if it happens again it's unrecoverable
                }
                _ => return Err(e.into()),
            },
        };

        let mut headers = [EMPTY_HEADER; MAX_HEADERS];
        let mut response = Response::new(&mut headers);
        match response.parse(&buf) {
            Err(e) => return Err(e.into()),
            Ok(Status::Partial) => bail!("Partial HTTP response"),
            Ok(Status::Complete(_)) => match response.code {
                Some(code) if code == 200 => (),
                Some(code) => return Err(Error::Status(code, self.url.to_string()).into()),
                None => bail!("Invalid HTTP response"),
            },
        }

        Decoder::new(&mut self.stream, &headers)
    }

    pub fn read_string(&mut self) -> Result<String> {
        Ok(io::read_to_string(&mut self.reader()?)?)
    }

    pub fn set_url(&mut self, url: Url) -> Result<()> {
        if get_host(&self.url)? == get_host(&url)? {
            self.url = url;
            self.request = Self::format_request(&self.url)?;
        } else {
            debug!("Host changed, creating new request");
            self.reconnect(Some(url))?;
        }

        Ok(())
    }

    fn reconnect(&mut self, url: Option<Url>) -> Result<()> {
        let request = if let Some(url) = url {
            Self::get(url)?
        } else {
            Self::get(self.url.clone())?
        };

        *self = request;
        Ok(())
    }

    fn do_io(&mut self) -> Result<Vec<u8>, io::Error> {
        const BUF_INIT_SIZE: usize = 1024;
        const HEADERS_END_SIZE: usize = 2; //read only \r\n

        debug!("Request:\n{}", self.request);
        self.stream.get_mut().write_all(self.request.as_bytes())?;

        let mut buf = vec![0u8; BUF_INIT_SIZE]; //has to be initialized or read_until can return 0
        let mut consumed = 0;
        while consumed != HEADERS_END_SIZE {
            if self.stream.fill_buf()?.is_empty() {
                return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
            }

            consumed = self.stream.read_until(b'\n', &mut buf)?;
        }
        buf.drain(..BUF_INIT_SIZE);
        debug!("Response:\n{}", String::from_utf8_lossy(&buf));

        Ok(buf)
    }

    fn format_request(url: &Url) -> Result<String> {
        let query = url
            .query()
            .map_or_else(String::default, |query| format!("?{query}"));

        Ok(format!(
            "GET {}{} HTTP/1.1\r\n\
             Host: {}\r\n\
             User-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:109.0) Gecko/20100101 Firefox/113.0\r\n\
             Accept: */*\r\n\
             Accept-Language: en-US\r\n\
             Accept-Encoding: gzip\r\n\
             Origin: https://player.twitch.tv\r\n\
             Connection: keep-alive\r\n\
             Sec-Fetch-Dest: empty\r\n\
             Sec-Fetch-Mode: cors\r\n\
             Sec-Fetch-Site: cross-site\r\n\
             \r\n",
            url.path(),
            query,
            get_host(url)?,
        ))
    }
}

#[allow(clippy::large_enum_variant)]
pub enum Transport {
    Http(TcpStream),

    #[cfg(any(feature = "rustls-webpki", feature = "rustls-native-certs"))]
    Https(rustls::StreamOwned<rustls::ClientConnection, TcpStream>),

    #[cfg(feature = "native-tls")]
    Https(native_tls::TlsStream<TcpStream>),
}

impl Read for Transport {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Http(sock) => sock.read(buf),
            Self::Https(stream) => stream.read(buf),
        }
    }
}

impl Write for Transport {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Http(sock) => sock.write(buf),
            Self::Https(stream) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Http(sock) => sock.flush(),
            Self::Https(stream) => stream.flush(),
        }
    }
}

impl Transport {
    pub fn new(url: &Url) -> Result<Self> {
        let scheme = url.scheme();
        let host = get_host(url)?;
        let port = url.port_or_known_default().context("Invalid port in URL")?;

        let sock = TcpStream::connect(format!("{host}:{port}"))?;
        sock.set_nodelay(true)?;

        match scheme {
            "http" => Ok(Self::Http(sock)),
            "https" => Ok(Self::Https(Self::init_tls(host, sock)?)),
            _ => bail!("{scheme} is not supported"),
        }
    }

    #[cfg(feature = "rustls-webpki")]
    fn root_certs() -> rustls::RootCertStore {
        let mut roots = rustls::RootCertStore::empty();
        roots.add_server_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.0.iter().map(|ta| {
            rustls::OwnedTrustAnchor::from_subject_spki_name_constraints(
                ta.subject,
                ta.spki,
                ta.name_constraints,
            )
        }));

        roots
    }

    #[cfg(feature = "rustls-native-certs")]
    fn root_certs() -> rustls::RootCertStore {
        let mut roots = rustls::RootCertStore::empty();
        let certs = rustls_native_certs::load_native_certs().unwrap_or_default();
        for cert in certs {
            //Ignore parsing errors, OS can have broken certs.
            if let Err(e) = roots.add(&rustls::Certificate(cert.0)) {
                debug!("Invalid certificate: {e}");
            }
        }

        roots
    }

    #[cfg(any(feature = "rustls-webpki", feature = "rustls-native-certs"))]
    fn init_tls(
        host: &str,
        mut sock: TcpStream,
    ) -> Result<rustls::StreamOwned<rustls::ClientConnection, TcpStream>> {
        use std::sync::Arc;

        let roots = Self::root_certs();
        let config = rustls::ClientConfig::builder()
            .with_safe_defaults()
            .with_root_certificates(roots)
            .with_no_client_auth();

        let mut conn = rustls::ClientConnection::new(Arc::new(config), host.try_into()?)?;
        conn.complete_io(&mut sock)?; //handshake

        Ok(rustls::StreamOwned::new(conn, sock))
    }

    #[cfg(feature = "native-tls")]
    fn init_tls(host: &str, sock: TcpStream) -> Result<native_tls::TlsStream<TcpStream>> {
        Ok(native_tls::TlsConnector::new()?.connect(host, sock)?)
    }
}

enum Encoding<'a> {
    Unencoded(&'a mut Stream, u64),
    Chunked(ChunkDecoder<&'a mut Stream>),
    ChunkedGzip(GzDecoder<ChunkDecoder<&'a mut Stream>>),
    Gzip(GzDecoder<&'a mut Stream>),
}

pub struct Decoder<'a> {
    kind: Encoding<'a>,
    consumed: u64,
}

impl Read for Decoder<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match &mut self.kind {
            Encoding::Unencoded(stream, length) => {
                let consumed = stream.take(*length - self.consumed).read(buf)?;
                self.consumed += consumed as u64;

                Ok(consumed)
            }
            Encoding::Chunked(reader) => reader.read(buf),
            Encoding::ChunkedGzip(reader) => {
                let consumed = reader.read(buf)?;
                if consumed == 0 {
                    //Gzip decoder doesn't consume trailing bytes in chunk decoder
                    io::copy(&mut reader.get_mut(), &mut io::sink())?;
                }

                Ok(consumed)
            }
            Encoding::Gzip(reader) => reader.read(buf),
        }
    }
}

impl<'a> Decoder<'a> {
    pub fn new(stream: &'a mut Stream, headers: &[Header]) -> Result<Decoder<'a>> {
        let content_length = headers
            .iter()
            .find(|h| h.name.to_lowercase() == "content-length");

        let is_chunked = headers.iter().any(|h| {
            h.name.to_lowercase() == "transfer-encoding"
                && String::from_utf8_lossy(h.value) == "chunked"
        });

        let is_gzipped = headers.iter().any(|h| {
            h.name.to_lowercase() == "content-encoding"
                && String::from_utf8_lossy(h.value) == "gzip"
        });

        match (is_chunked, is_gzipped) {
            (true, true) => {
                debug!("Body is chunked and gzipped");

                return Ok(Self {
                    kind: Encoding::ChunkedGzip(GzDecoder::new(ChunkDecoder::new(stream))),
                    consumed: u64::default(),
                });
            }
            (true, false) => {
                debug!("Body is chunked");

                return Ok(Self {
                    kind: Encoding::Chunked(ChunkDecoder::new(stream)),
                    consumed: u64::default(),
                });
            }
            (false, true) => {
                debug!("Body is gzipped");

                return Ok(Self {
                    kind: Encoding::Gzip(GzDecoder::new(stream)),
                    consumed: u64::default(),
                });
            }
            _ => match content_length {
                Some(header) => {
                    let length = String::from_utf8_lossy(header.value).parse()?;
                    debug!("Content length: {length}");

                    return Ok(Self {
                        kind: Encoding::Unencoded(stream, length),
                        consumed: u64::default(),
                    });
                }
                _ => bail!("Could not resolve encoding of HTTP response"),
            },
        }
    }
}

#[inline]
fn get_host(url: &Url) -> Result<&str> {
    url.host_str().context("Invalid host in URL")
}
