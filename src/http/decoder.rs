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
    is_gzipped: bool,
    is_chunked: bool,
    content_length: Option<u64>,

    kind: Option<Encoding<R>>,
    consumed: u64,
}

impl<R: Read> Read for Decoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.kind.as_mut().expect("Missing encoding") {
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
    pub fn new(headers: &str) -> Self {
        let mut content_length = None;
        let mut is_chunked = false;
        let mut is_gzipped = false;

        for line in headers.lines() {
            let mut split = line.split_whitespace();
            let Some(key) = split.next() else {
                continue;
            };

            if key.eq_ignore_ascii_case("content-encoding:") {
                is_gzipped = split.next().is_some_and(|h| h == "gzip");
            } else if key.eq_ignore_ascii_case("transfer-encoding:") {
                is_chunked = split.next().is_some_and(|h| h == "chunked");
            } else if key.eq_ignore_ascii_case("content-length:") {
                content_length = split.next().and_then(|h| h.parse().ok());
            }
        }

        Self {
            is_gzipped,
            is_chunked,
            content_length,
            kind: Option::default(),
            consumed: u64::default(),
        }
    }

    pub fn set_reader(&mut self, reader: R) -> Result<()> {
        let kind = match (self.is_chunked, self.is_gzipped) {
            (true, true) => {
                debug!("Body is chunked and gzipped");
                Encoding::ChunkedGzip(GzDecoder::new(ChunkDecoder::new(reader)))
            }
            (true, false) => {
                debug!("Body is chunked");
                Encoding::Chunked(ChunkDecoder::new(reader))
            }
            (false, true) => {
                debug!("Body is gzipped");
                Encoding::Gzip(GzDecoder::new(reader))
            }
            (false, false) => match self.content_length {
                Some(length) => {
                    debug!("Content length: {length}");
                    Encoding::Unencoded(reader, length)
                }
                None => bail!("Failed to resolve encoding of HTTP response"),
            },
        };

        self.kind = Some(kind);
        Ok(())
    }
}
