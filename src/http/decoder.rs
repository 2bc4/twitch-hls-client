use std::io::{self, Read};

use anyhow::{bail, Result};
use chunked_transfer::Decoder as ChunkDecoder;
use flate2::read::GzDecoder;
use log::debug;

enum Encoding<T: Read> {
    Unencoded(T, u64),
    Chunked(ChunkDecoder<T>),
    ChunkedGzip(GzDecoder<ChunkDecoder<T>>),
    Gzip(GzDecoder<T>),
}

pub struct Decoder<T: Read> {
    kind: Encoding<T>,
    consumed: u64,
}

impl<T: Read> Read for Decoder<T> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match &mut self.kind {
            Encoding::Unencoded(reader, length) => {
                let consumed = reader.take(*length - self.consumed).read(buf)?;
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

impl<T: Read> Decoder<T> {
    pub fn new(reader: T, headers: &str) -> Result<Decoder<T>> {
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

                Ok(Self {
                    kind: Encoding::ChunkedGzip(GzDecoder::new(ChunkDecoder::new(reader))),
                    consumed: u64::default(),
                })
            }
            (true, false) => {
                debug!("Body is chunked");

                Ok(Self {
                    kind: Encoding::Chunked(ChunkDecoder::new(reader)),
                    consumed: u64::default(),
                })
            }
            (false, true) => {
                debug!("Body is gzipped");

                Ok(Self {
                    kind: Encoding::Gzip(GzDecoder::new(reader)),
                    consumed: u64::default(),
                })
            }
            _ => match content_length {
                Some(length) => {
                    debug!("Content length: {length}");

                    Ok(Self {
                        kind: Encoding::Unencoded(reader, length),
                        consumed: u64::default(),
                    })
                }
                _ => bail!("Could not resolve encoding of HTTP response"),
            },
        }
    }
}
