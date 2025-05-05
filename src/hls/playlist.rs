use std::{
    collections::{VecDeque, vec_deque::IterMut},
    env,
};

use anyhow::{Context, Result, ensure};
use log::{debug, info};

use super::{
    OfflineError, map_if_offline,
    segment::{Duration, Segment},
};

use crate::{
    http::{Connection, Url},
    logger,
};

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
                    if total_segments > prev_segment_count {
                        if let Some(url) = lines.next() {
                            self.segments
                                .push_back(Segment::Normal(split.1.parse()?, url.into()));
                        }
                    }
                }
                "#EXT-X-TWITCH-PREFETCH" => {
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
        info!("Resetting playlist...");
        self.segments.clear();
        self.sequence = 0;
        self.added = 0;
    }

    pub(super) fn segment_queue(&mut self) -> QueueRange<'_> {
        if self.added == 0 {
            QueueRange::Empty
        } else if self.added == self.segments.len() {
            QueueRange::Back(self.segments.back_mut())
        } else {
            QueueRange::Partial(self.segments.range_mut(self.segments.len() - self.added..))
        }
    }

    pub(super) fn last_duration(&self) -> Option<Duration> {
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
