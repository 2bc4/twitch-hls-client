use std::io::{self, Read};

use anyhow::{bail, Result};
use chunked_transfer::Decoder as ChunkDecoder;
use flate2::read::GzDecoder;
use log::debug;

enum Encoding<R: Read> {
    Unencoded(R, u64),
    Chunked(ChunkDecoder<R>),
    ChunkedGzip(GzDecoder<ChunkDecoder<R>>),
    Gzip(GzDecoder<R>),
}

pub struct Decoder<R: Read> {
    kind: Encoding<R>,
    consumed: u64,
}

impl<R: Read> Read for Decoder<R> {
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

impl<R: Read> Decoder<R> {
    pub fn new(reader: R, headers: &str) -> Result<Self> {
        let mut content_length = None;
        let mut is_chunked = false;
        let mut is_gzipped = false;

        for line in headers.lines() {
            let mut split = line.split_whitespace();
            match split.next() {
                Some("content-encoding:") => {
                    is_gzipped = split.next().is_some_and(|h| h == "gzip");
                }
                Some("transfer-encoding:") => {
                    is_chunked = split.next().is_some_and(|h| h == "chunked");
                }
                Some("content-length:") => {
                    content_length = split.next().and_then(|h| h.parse().ok());
                }
                _ => continue,
            }
        }

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
            (false, false) => match content_length {
                Some(length) => {
                    debug!("Content length: {length}");

                    Ok(Self {
                        kind: Encoding::Unencoded(reader, length),
                        consumed: u64::default(),
                    })
                }
                None => bail!("Failed to resolve encoding of HTTP response"),
            },
        }
    }
}
