use std::{str::FromStr, thread, time::Duration as StdDuration};

use anyhow::{Context, Result};
use log::debug;
use url::Url;

use super::Error;

#[derive(Clone, PartialEq, Debug)]
pub struct Duration(StdDuration);

impl FromStr for Duration {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(
            StdDuration::try_from_secs_f32(
                s.split_once(':')
                    .and_then(|s| s.1.split_once(','))
                    .map(|s| s.0.parse())
                    .context("Invalid segment duration")??,
            )
            .context("Failed to parse segment duration")?,
        ))
    }
}

impl Duration {
    pub fn sleep(&self, elapsed: StdDuration) {
        Self::sleep_thread(self.0, elapsed);
    }

    pub fn sleep_half(&self, elapsed: StdDuration) {
        if let Some(half) = self.0.checked_div(2) {
            Self::sleep_thread(half, elapsed);
        }
    }

    fn sleep_thread(duration: StdDuration, elapsed: StdDuration) {
        if let Some(sleep_time) = duration.checked_sub(elapsed) {
            debug!("Sleeping thread for {:?}", sleep_time);
            thread::sleep(sleep_time);
        }
    }
}

//Used for av1/hevc streams
pub struct Header(pub Option<Url>);

impl FromStr for Header {
    type Err = anyhow::Error;

    fn from_str(playlist: &str) -> Result<Self, Self::Err> {
        let header_url = playlist
            .lines()
            .find(|s| s.starts_with("#EXT-X-MAP"))
            .and_then(|s| s.split_once('='))
            .map(|s| s.1.replace('"', ""));

        if let Some(header_url) = header_url {
            return Ok(Self(Some(
                header_url
                    .parse()
                    .context("Failed to parse segment header URL")?,
            )));
        }

        Ok(Self(None))
    }
}

#[derive(PartialEq, Debug, Clone)]
pub struct Segment {
    pub duration: Duration,
    pub url: Url,
}

impl Segment {
    pub fn new(extinf: &str, url: &str) -> Result<Self> {
        Ok(Self {
            duration: extinf.parse()?,
            url: url.parse()?,
        })
    }
}

#[derive(Copy, Clone)]
pub enum PrefetchSegmentKind {
    Newest,
    Next,
}

impl PrefetchSegmentKind {
    pub fn to_segment(self, duration: Duration, playlist: &str) -> Result<Segment> {
        Ok(Segment {
            duration,
            url: playlist
                .lines()
                .rev()
                .filter(|s| s.starts_with("#EXT-X-TWITCH-PREFETCH"))
                .nth(self as usize) //Newest = 0, Next = 1
                .and_then(|s| s.split_once(':'))
                .map(|s| s.1)
                .ok_or(Error::Advertisement)?
                .parse()
                .or(Err(Error::Advertisement))?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::playlist::tests::create_playlist;
    use super::super::tests::PLAYLIST;
    use super::*;

    #[test]
    fn parse_header() {
        assert_eq!(
            PLAYLIST.parse::<Header>().unwrap().0,
            Some(Url::parse("http://header.invalid").unwrap())
        );
    }

    #[test]
    fn parse_prefetch_segments() {
        let playlist = create_playlist();
        assert_eq!(
            playlist
                .prefetch_segment(PrefetchSegmentKind::Newest)
                .unwrap(),
            Segment {
                duration: Duration(StdDuration::from_secs_f32(0.978)),
                url: Url::parse("http://newest-prefetch-url.invalid").unwrap(),
            },
        );

        assert_eq!(
            playlist
                .prefetch_segment(PrefetchSegmentKind::Next)
                .unwrap(),
            Segment {
                duration: Duration(StdDuration::from_secs_f32(0.978)),
                url: Url::parse("http://next-prefetch-url.invalid").unwrap(),
            },
        );
    }
}
