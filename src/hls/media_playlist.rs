use std::{
    collections::{vec_deque::IterMut, VecDeque},
    env,
};

use anyhow::{ensure, Context, Result};
use log::debug;

use super::{
    segment::{Duration, Segment},
    OfflineError,
};

use crate::{
    http::{Agent, TextRequest, Url},
    logger,
};

pub struct MediaPlaylist {
    pub header: Option<Url>, //used for av1/hevc streams

    segments: VecDeque<Segment>,
    sequence: usize,
    added: usize,

    request: TextRequest,

    debug_log_playlist: bool,
}

impl MediaPlaylist {
    pub fn new(url: Url, agent: &Agent) -> Result<Self> {
        let mut playlist = Self {
            header: Option::default(),

            segments: VecDeque::with_capacity(16),
            sequence: usize::default(),
            added: usize::default(),

            request: agent.get(url)?,

            debug_log_playlist: logger::is_debug() && env::var_os("DEBUG_NO_PLAYLIST").is_none(),
        };

        playlist.reload()?;
        Ok(playlist)
    }

    pub fn reload(&mut self) -> Result<()> {
        debug!("----------RELOADING----------");
        let playlist = self.request.text().map_err(super::map_if_offline)?;
        if self.debug_log_playlist {
            debug!("Playlist:\n{playlist}");
        }

        if playlist
            .lines()
            .next_back()
            .is_some_and(|l| l.starts_with("#EXT-X-ENDLIST"))
        {
            return Err(OfflineError.into());
        }

        let mut prefetch_removed = 0;
        for _ in 0..2 {
            if let Some(segment) = self.segments.back() {
                match segment {
                    Segment::NextPrefetch(_) | Segment::NewestPrefetch(_) => {
                        self.segments.pop_back();
                        prefetch_removed += 1;
                    }
                    Segment::Normal(_, _) => (),
                }
            }
        }

        let mut prev_segment_count = self.segments.len();
        let mut total_segments = 0;
        let mut lines = playlist.lines().peekable();
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
                            .replace('"', "")
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
                        if lines.peek().is_some() {
                            self.segments
                                .push_back(Segment::NextPrefetch(split.1.into()));
                        } else {
                            self.segments
                                .push_back(Segment::NewestPrefetch(split.1.into()));
                        }
                    }
                }
                _ => continue,
            }
        }

        self.added = total_segments - (prev_segment_count + prefetch_removed);
        debug!("Segments added: {}", self.added);

        Ok(())
    }

    pub fn segments(&mut self) -> QueueRange<'_> {
        if self.added == 0 {
            QueueRange::Empty
        } else if self.added == self.segments.len() {
            QueueRange::Back(self.segments.back_mut())
        } else {
            QueueRange::Partial(self.segments.range_mut(self.segments.len() - self.added..))
        }
    }

    pub fn last_duration(&mut self) -> Option<Duration> {
        self.segments
            .iter()
            .rev()
            .find_map(|s| match s {
                Segment::Normal(duration, _) => Some(duration),
                _ => None,
            })
            .copied()
    }
}

pub enum QueueRange<'a> {
    Partial(IterMut<'a, Segment>),
    Back(Option<&'a mut Segment>),
    Empty,
}