use std::{
    cmp::Ordering,
    collections::{VecDeque, vec_deque::IterMut},
    env, mem,
    str::FromStr,
    thread, time,
};

use anyhow::{Context, Result, ensure};
use log::debug;

use super::{OfflineError, map_if_offline};

use crate::{
    http::{Connection, Url},
    logger,
};

#[derive(Debug)]
pub enum Segment {
    Normal(Duration, Url),
    Prefetch(Url),
}

impl Segment {
    pub fn take_url(&mut self) -> Url {
        let url = match self {
            Self::Normal(_, url) | Self::Prefetch(url) => url,
        };

        mem::take(url)
    }
}

#[derive(Default, Copy, Clone, Debug)]
pub struct Duration {
    pub is_ad: bool,
    inner: time::Duration,
}

impl FromStr for Duration {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self {
            is_ad: s.contains('|'),
            inner: time::Duration::try_from_secs_f32(
                s.split_once(',')
                    .map(|d| d.0)
                    .and_then(|d| d.parse().ok())
                    .context("Invalid segment duration")?,
            )
            .context("Failed to parse segment duration")?,
        })
    }
}

impl PartialEq for Duration {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

impl PartialOrd for Duration {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.inner.cmp(&other.inner))
    }
}

impl Duration {
    //Can't wait too long or the server will close the socket
    pub const MAX: Self = Self {
        is_ad: false,
        inner: time::Duration::from_secs(3),
    };

    pub fn sleep(&self, elapsed: time::Duration) {
        if *self >= Self::MAX {
            self.sleep_half(elapsed);
            return;
        }

        Self::sleep_thread(self.inner, elapsed);
    }

    pub fn sleep_half(&self, elapsed: time::Duration) {
        if let Some(half) = self.inner.checked_div(2) {
            Self::sleep_thread(half, elapsed);
        }
    }

    fn sleep_thread(duration: time::Duration, elapsed: time::Duration) {
        if let Some(sleep_time) = duration.checked_sub(elapsed) {
            debug!("Sleeping thread for {sleep_time:?}");
            thread::sleep(sleep_time);
        }
    }
}

pub enum QueueRange<'a> {
    Partial(IterMut<'a, Segment>),
    Back(Option<&'a mut Segment>),
    Empty,
}

pub struct Playlist {
    pub header: Option<Url>, //used for av1/hevc streams

    conn: Connection,
    segments: VecDeque<Segment>,
    should_debug_log: bool,

    sequence: usize,
    added: usize,
}

impl Playlist {
    pub fn new(conn: Connection) -> Result<Self> {
        let mut playlist = Self {
            conn,
            segments: VecDeque::with_capacity(16),
            should_debug_log: logger::is_debug() && env::var_os("DEBUG_NO_PLAYLIST").is_none(),
            header: Option::default(),
            sequence: usize::default(),
            added: usize::default(),
        };

        playlist.reload()?;
        Ok(playlist)
    }

    pub fn reload(&mut self) -> Result<()> {
        let playlist = self.conn.text().map_err(map_if_offline)?;
        if self.should_debug_log {
            debug!("Playlist:\n{playlist}");
        }

        if playlist
            .lines()
            .next_back()
            .is_some_and(|l| l.trim() == "#EXT-X-ENDLIST")
        {
            return Err(OfflineError.into());
        }

        let mut prefetch_removed = Self::remove_prefetch(&mut self.segments);
        let mut prev_segment_count = self.segments.len();
        let mut total_segments = 0;
        let mut lines = playlist.lines();
        while let Some(line) = lines.next() {
            let Some(split) = line.split_once(':') else {
                continue;
            };

            match split.0 {
                "#EXT-X-MEDIA-SEQUENCE" => {
                    let sequence = split.1.parse()?;
                    ensure!(sequence >= self.sequence, "Sequence went backwards");

                    if sequence > 0 {
                        let removed = sequence - self.sequence;
                        if removed < self.segments.len() {
                            self.segments.drain(..removed);
                            prev_segment_count = self.segments.len();

                            debug!("Segments removed: {removed}");
                        } else {
                            self.segments.clear();
                            prev_segment_count = 0;
                            prefetch_removed = 0;

                            debug!("All segments removed");
                        }
                    }

                    self.sequence = sequence;
                }
                "#EXT-X-MAP" if self.header.is_none() => {
                    self.header = Some(
                        split
                            .1
                            .split_once('=')
                            .context("Failed to parse segment header")?
                            .1
                            .trim_matches('"')
                            .into(),
                    );
                }
                "#EXTINF" => {
                    total_segments += 1;
                    if total_segments > prev_segment_count
                        && let Some(url) = lines.next()
                    {
                        self.segments
                            .push_back(Segment::Normal(split.1.parse()?, url.into()));
                    }
                }
                "#EXT-X-TWITCH-PREFETCH" | "#EXT-X-PREFETCH" => {
                    total_segments += 1;
                    if total_segments > prev_segment_count {
                        self.segments.push_back(Segment::Prefetch(split.1.into()));
                    }
                }
                _ => (),
            }
        }

        self.added = total_segments - (prev_segment_count + prefetch_removed);
        debug!("Segments added: {}", self.added);

        Ok(())
    }

    pub fn reset(&mut self) {
        debug!("Resetting playlist...");
        self.segments.clear();
        self.sequence = 0;
        self.added = 0;
    }

    pub fn segment_queue(&mut self) -> QueueRange<'_> {
        if self.added == 0 {
            QueueRange::Empty
        } else if self.added == self.segments.len() {
            QueueRange::Back(self.segments.back_mut())
        } else {
            QueueRange::Partial(self.segments.range_mut(self.segments.len() - self.added..))
        }
    }

    pub fn last_duration(&self) -> Option<Duration> {
        self.segments
            .iter()
            .rev()
            .find_map(|s| match s {
                Segment::Normal(duration, _) => Some(duration),
                Segment::Prefetch(_) => None,
            })
            .copied()
    }

    fn remove_prefetch(segments: &mut VecDeque<Segment>) -> usize {
        let before = segments.len();
        segments.retain(|s| matches!(*s, Segment::Normal(_, _)));

        before - segments.len()
    }
}
