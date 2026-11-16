//! The play queue: the tracks of the list shown last, the current one, shuffle and repeat.
//! Session-only and in memory. Nothing here is ever written to disk.

use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    /// Content URI the platform can open (MediaStore item or a URI handed over by Android).
    pub uri: String,
    pub title: String,
    pub artist: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Repeat {
    Off,
    All,
    One,
}

pub struct Playlist {
    tracks: Vec<Track>,
    /// Playback order: indices into `tracks`; the identity when shuffle is off.
    order: Vec<usize>,
    /// Position inside `order` of the current track.
    pos: Option<usize>,
    shuffle: bool,
    repeat: Repeat,
    rng: u64,
}

impl Playlist {
    pub fn new() -> Playlist {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        Playlist {
            tracks: Vec::new(),
            order: Vec::new(),
            pos: None,
            shuffle: false,
            repeat: Repeat::Off,
            rng: nanos | 1,
        }
    }

    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    pub fn track(&self, index: usize) -> Option<&Track> {
        self.tracks.get(index)
    }

    pub fn shuffle(&self) -> bool {
        self.shuffle
    }

    pub fn repeat(&self) -> Repeat {
        self.repeat
    }

    pub fn current(&self) -> Option<&Track> {
        let index = self.pos.map(|p| self.order[p])?;
        self.tracks.get(index)
    }

    /// Replaces the tracks. `current_uri` stays current if it is still in the new list.
    pub fn set_tracks(&mut self, tracks: Vec<Track>, current_uri: Option<&str>) {
        self.tracks = tracks;
        let current = current_uri.and_then(|u| self.tracks.iter().position(|t| t.uri == u));
        self.rebuild_order(current);
    }

    /// Makes the track at `index` (index in the list) the current one.
    pub fn select(&mut self, index: usize) -> Option<&Track> {
        if index >= self.tracks.len() {
            return None;
        }
        self.pos = self.order.iter().position(|&i| i == index);
        self.current()
    }

    pub fn set_shuffle(&mut self, on: bool) {
        if self.shuffle == on {
            return;
        }
        self.shuffle = on;
        let current = self.pos.map(|p| self.order[p]);
        self.rebuild_order(current);
    }

    pub fn cycle_repeat(&mut self) {
        self.repeat = match self.repeat {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        };
    }

    /// True if a track that ended by itself is followed by another one.
    pub fn can_advance_after_end(&self) -> bool {
        let n = self.order.len();
        match self.pos {
            None => false,
            Some(p) => self.repeat != Repeat::Off || p + 1 < n,
        }
    }

    /// The track after the current one. `auto` is true when the last track ended by itself
    /// (repeat-one then repeats it); a manual Next never repeats the same track.
    pub fn advance(&mut self, auto: bool) -> Option<&Track> {
        let n = self.order.len();
        if n == 0 {
            return None;
        }
        let Some(p) = self.pos else {
            self.pos = Some(0);
            return self.current();
        };
        if auto && self.repeat == Repeat::One {
            return self.current();
        }
        if p + 1 < n {
            self.pos = Some(p + 1);
        } else if self.repeat == Repeat::All {
            if self.shuffle {
                // A new random round that does not start with the track just played.
                let last = self.order[p];
                self.rebuild_order(Some(last));
                self.pos = Some(if n > 1 { 1 } else { 0 });
            } else {
                self.pos = Some(0);
            }
        } else {
            return None;
        }
        self.current()
    }

    /// The track before the current one; at the start it wraps (repeat all) or restarts.
    pub fn previous(&mut self) -> Option<&Track> {
        let n = self.order.len();
        if n == 0 {
            return None;
        }
        let Some(p) = self.pos else {
            self.pos = Some(0);
            return self.current();
        };
        if p > 0 {
            self.pos = Some(p - 1);
        } else if self.repeat == Repeat::All {
            self.pos = Some(n - 1);
        }
        self.current()
    }

    fn rebuild_order(&mut self, current: Option<usize>) {
        let n = self.tracks.len();
        self.order = (0..n).collect();
        if self.shuffle && n > 1 {
            for i in (1..n).rev() {
                let j = (self.next_random() % (i as u64 + 1)) as usize;
                self.order.swap(i, j);
            }
            // The current track goes first, so "next" never repeats it at once.
            if let Some(c) = current {
                if let Some(p) = self.order.iter().position(|&i| i == c) {
                    self.order.swap(0, p);
                }
            }
        }
        self.pos = current.and_then(|c| self.order.iter().position(|&i| i == c));
    }

    /// xorshift64: a few lines instead of a random-number dependency.
    fn next_random(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        x
    }
}
