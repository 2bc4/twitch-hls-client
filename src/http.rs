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
    fmt, io,
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    sync::Arc,
};

use anyhow::{bail, Context, Result};
use chunked_transfer::Decoder as ChunkDecoder;
use flate2::read::GzDecoder;
use httparse::{Header, Response, Status, EMPTY_HEADER};
use log::debug;
use rustls::{Certificate, ClientConfig, ClientConnection, RootCertStore, StreamOwned};
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

pub trait ReadWrite: Read + Write {}
impl ReadWrite for TcpStream {}
impl ReadWrite for StreamOwned<ClientConnection, TcpStream> {}

type Stream = BufReader<Box<dyn ReadWrite>>;

pub struct Request {
    stream: Stream,
    request: String,
    accept_header: String,
    url: Url,
}

impl Request {
    pub fn get(url: &str) -> Result<Self> {
        const DEFAULT_ACCEPT_HEADER: &str = "*/*";

        let url = Url::parse(url).context("Invalid request URL")?;
        let scheme = url.scheme();
        let host = get_host(&url)?;
        let port = url
            .port_or_known_default()
            .context("Invalid port in request URL")?;

        let stream: Box<dyn ReadWrite> = match scheme {
            "http" => {
                let sock = TcpStream::connect(format!("{host}:{port}"))?;
                sock.set_nodelay(true)?;

                Box::new(sock)
            }
            "https" => {
                let mut roots = RootCertStore::empty();
                for cert in rustls_native_certs::load_native_certs()? {
                    roots.add(&Certificate(cert.0))?;
                }

                let config = ClientConfig::builder()
                    .with_safe_defaults()
                    .with_root_certificates(roots)
                    .with_no_client_auth();

                let conn = ClientConnection::new(Arc::new(config), host.try_into()?)?;
                let sock = TcpStream::connect(format!("{host}:{port}"))?;
                sock.set_nodelay(true)?;

                let mut stream = StreamOwned::new(conn, sock);
                stream.flush()?; //handshake

                Box::new(stream)
            }
            _ => bail!("{scheme} is not supported"),
        };

        Ok(Self {
            stream: BufReader::new(stream),
            request: Self::format_request(&url, DEFAULT_ACCEPT_HEADER)?,
            accept_header: DEFAULT_ACCEPT_HEADER.to_owned(),
            url,
        })
    }

    pub fn get_with_header(url: &str, header: &str) -> Result<Self> {
        let mut r = Self::get(url)?;

        //Before end of headers.
        //Will be overwritten if set_url is called but this is only needed for the TTVLOL API.
        r.request.insert_str(r.request.len() - 2, header);
        r.request += "\r\n";
        Ok(r)
    }

    pub fn reader(&mut self) -> Result<Decoder> {
        self.process()
    }

    pub fn read_string(&mut self) -> Result<String> {
        Ok(io::read_to_string(&mut self.process()?)?)
    }

    pub fn set_url(&mut self, url: &str) -> Result<()> {
        assert!(!self.is_different_host(url)?);

        self.url = Url::parse(url).context("Invalid updated request URL")?;
        self.request = Self::format_request(&self.url, &self.accept_header)?;
        Ok(())
    }

    pub fn set_accept_header(&mut self, accept_header: &str) -> Result<()> {
        self.accept_header = accept_header.to_owned();
        self.request = Self::format_request(&self.url, &self.accept_header)?;

        Ok(())
    }

    pub fn is_different_host(&self, url: &str) -> Result<bool> {
        Ok(get_host(&self.url)? != get_host(&Url::parse(url)?)?)
    }

    fn process(&mut self) -> Result<Decoder> {
        const BUF_INIT_SIZE: usize = 1024;
        const HEADERS_END_SIZE: usize = 2; //read only \r\n
        const MAX_HEADERS: usize = 16;

        debug!("Request:\n{}", self.request);
        self.stream.get_mut().write_all(self.request.as_bytes())?;

        let mut buf = vec![0u8; BUF_INIT_SIZE]; //has to be initialized or read_until can return 0
        let mut consumed = 0;
        while consumed != HEADERS_END_SIZE {
            consumed = self.stream.read_until(b'\n', &mut buf)?;
            if consumed == 0 {
                bail!("EOF on HTTP stream");
            }
        }
        buf.drain(..BUF_INIT_SIZE);
        debug!("Response:\n{}", String::from_utf8_lossy(&buf));

        let mut headers = [EMPTY_HEADER; MAX_HEADERS];
        let mut response = Response::new(&mut headers);
        match response.parse(&buf) {
            Err(e) => return Err(e.into()),
            Ok(Status::Partial) => bail!("Partial HTTP response"),
            Ok(Status::Complete(_)) => match response.code {
                Some(code) if code == 200 => (),
                Some(code) => return Err(Error::Status(code, self.url.as_str().to_owned()).into()),
                None => bail!("Invalid HTTP response"),
            },
        }

        Decoder::new(&mut self.stream, &headers)
    }

    fn format_request(url: &Url, accept_header: &str) -> Result<String> {
        //because url crate doesn't prepend ? to the first query param
        let query = url
            .query()
            .map_or_else(String::new, |query| "?".to_owned() + query);

        Ok(format!(
            "GET {}{} HTTP/1.1\r\n\
             Host: {}\r\n\
             User-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:109.0) Gecko/20100101 Firefox/112.0\r\n\
             Accept: {}\r\n\
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
            accept_header,
        ))
    }
}

enum Encoding<'a> {
    Unencoded(&'a mut Stream, usize),
    Chunked(ChunkDecoder<&'a mut Stream>),
    ChunkedGzip(GzDecoder<ChunkDecoder<&'a mut Stream>>),
    Gzip(GzDecoder<&'a mut Stream>),
}

pub struct Decoder<'a> {
    kind: Encoding<'a>,
    consumed: usize,
}

impl Read for Decoder<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match &mut self.kind {
            Encoding::Unencoded(stream, length) => {
                let consumed = stream.take((*length - self.consumed) as u64).read(buf)?;
                self.consumed += consumed;

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
                && String::from_utf8_lossy(h.value).to_lowercase() == "chunked"
        });

        let is_gzipped = headers.iter().any(|h| {
            h.name.to_lowercase() == "content-encoding"
                && String::from_utf8_lossy(h.value).to_lowercase() == "gzip"
        });

        match (is_chunked, is_gzipped) {
            (true, true) => {
                debug!("Body is chunked and gzipped");

                return Ok(Self {
                    kind: Encoding::ChunkedGzip(GzDecoder::new(ChunkDecoder::new(stream))),
                    consumed: usize::default(),
                });
            }
            (true, false) => {
                debug!("Body is chunked");

                return Ok(Self {
                    kind: Encoding::Chunked(ChunkDecoder::new(stream)),
                    consumed: usize::default(),
                });
            }
            (false, true) => {
                debug!("Body is gzipped");

                return Ok(Self {
                    kind: Encoding::Gzip(GzDecoder::new(stream)),
                    consumed: usize::default(),
                });
            }
            _ => match content_length {
                Some(header) => {
                    let length = String::from_utf8_lossy(header.value).parse()?;
                    debug!("Content length: {length}");

                    return Ok(Self {
                        kind: Encoding::Unencoded(stream, length),
                        consumed: usize::default(),
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
