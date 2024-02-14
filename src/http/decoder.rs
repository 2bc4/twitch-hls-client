use std::io::{self, BufReader, Read};

use anyhow::{bail, Result};
use chunked_transfer::Decoder as ChunkDecoder;
use flate2::read::GzDecoder;
use log::debug;

use super::request::Transport;

enum Encoding<'a> {
    Unencoded(&'a mut BufReader<Transport>, u64),
    Chunked(ChunkDecoder<&'a mut BufReader<Transport>>),
    ChunkedGzip(GzDecoder<ChunkDecoder<&'a mut BufReader<Transport>>>),
    Gzip(GzDecoder<&'a mut BufReader<Transport>>),
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
    pub fn new(stream: &'a mut BufReader<Transport>, headers: &str) -> Result<Decoder<'a>> {
        let headers = headers.to_lowercase();
        let content_length = headers
            .lines()
            .find(|h| h.starts_with("content-length"))
            .and_then(|h| h.split_whitespace().nth(1))
            .and_then(|h| h.parse().ok());

        let is_chunked = headers.lines().any(|h| h == "transfer-encoding: chunked");
        let is_gzipped = headers.lines().any(|h| h == "content-encoding: gzip");
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
                Some(length) => {
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
