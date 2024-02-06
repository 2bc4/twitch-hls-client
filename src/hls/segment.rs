use std::{str::FromStr, thread, time::Duration as StdDuration};

use anyhow::{Context, Result};
use log::debug;

//Used for av1/hevc streams
pub struct Header(pub Option<String>);

impl FromStr for Header {
    type Err = anyhow::Error;

    fn from_str(playlist: &str) -> Result<Self, Self::Err> {
        let header_url = playlist
            .lines()
            .find(|s| s.starts_with("#EXT-X-MAP"))
            .and_then(|s| s.split_once('='))
            .map(|s| s.1.replace('"', ""));

        if let Some(header_url) = header_url {
            return Ok(Self(Some(header_url)));
        }

        Ok(Self(None))
    }
}

#[derive(Default, Clone, PartialEq, Debug)]
pub struct Duration {
    pub is_ad: bool,
    duration: StdDuration,
}

impl FromStr for Duration {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self {
            duration: StdDuration::try_from_secs_f32(
                s.split_once(':')
                    .and_then(|s| s.1.split_once(','))
                    .map(|s| s.0.parse())
                    .context("Invalid segment duration")??,
            )
            .context("Failed to parse segment duration")?,
            is_ad: s.contains('|'),
        })
    }
}

impl Duration {
    pub fn sleep(&self, elapsed: StdDuration) {
        Self::sleep_thread(self.duration, elapsed);
    }

    pub fn sleep_half(&self, elapsed: StdDuration) {
        if let Some(half) = self.duration.checked_div(2) {
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

#[derive(Default, Clone, Debug)]
pub struct Segment {
    pub duration: Duration,
    pub url: String,
}

impl PartialEq for Segment {
    fn eq(&self, other: &Self) -> bool {
        self.url == other.url
    }
}

#[cfg(test)]
mod tests {
    use super::super::playlist::{tests::create_playlist, PrefetchSegmentKind};
    use super::super::tests::PLAYLIST;
    use super::*;

    #[test]
    fn parse_header() {
        assert_eq!(
            PLAYLIST.parse::<Header>().unwrap().0,
            Some("http://header.invalid".to_string()),
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
                duration: Duration {
                    duration: StdDuration::from_secs_f32(0.978),
                    is_ad: false,
                },
                url: "http://newest-prefetch-url.invalid".to_string(),
            },
        );

        assert_eq!(
            playlist
                .prefetch_segment(PrefetchSegmentKind::Next)
                .unwrap(),
            Segment {
                duration: Duration {
                    duration: StdDuration::from_secs_f32(0.978),
                    is_ad: false,
                },
                url: "http://next-prefetch-url.invalid".to_string(),
            },
        );
    }
}
