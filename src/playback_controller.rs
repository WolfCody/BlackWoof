//! The playback controller: the one place where the queue, the engine, the saved state, the
//! text screen and the system media controls come together. Rust decides everything here;
//! the Java side only runs the platform calls listed in `Platform` and reports user input.
//!
//! Locking rule: the `core` lock is never held while `Engine::play_at` runs (it joins the
//! previous playback thread, which may be waiting for this very lock inside the observer).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::audio_engine::{Engine, State};
use crate::error::ErrorKind;
use crate::fd::take_fd;
use crate::media_session;
use crate::playlist::{Playlist, Track};
use crate::storage::{self, Last};
use crate::ui::{
    self, Lang, Note, Screen, View, ID_BACK, ID_ITEM_BASE, ID_LANG, ID_MUSIC, ID_NEXT,
    ID_PLAY_PAUSE, ID_PREV, ID_RED, ID_REFRESH, ID_REPEAT, ID_SHUFFLE,
};

/// Effects returned to Java: things only an Activity or Service can do.
pub const EFFECT_START_SERVICE: i32 = 1;
pub const EFFECT_REQUEST_PERMISSION: i32 = 2;

/// Commands from the system media controls and the notification.
pub const CMD_PLAY: i32 = 1;
pub const CMD_PAUSE: i32 = 2;
pub const CMD_TOGGLE: i32 = 3;
pub const CMD_NEXT: i32 = 4;
pub const CMD_PREVIOUS: i32 = 5;
pub const CMD_STOP: i32 = 6;

/// Audio focus changes: the Android values, plus "the request was refused".
pub const FOCUS_GAIN: i32 = 1;
pub const FOCUS_LOSS: i32 = -1;
pub const FOCUS_LOSS_TRANSIENT: i32 = -2;
pub const FOCUS_CAN_DUCK: i32 = -3;
pub const FOCUS_DENIED: i32 = 100;

/// Volume while another app is speaking briefly (ducking).
const DUCK_GAIN: f32 = 0.3;

/// What the platform side (Java) can do for Rust. Every call is best effort: failures must
/// never disturb playback. None of them may call back into Rust synchronously.
pub trait Platform: Send + Sync {
    /// Opens `uri` for reading and returns a detached descriptor (ownership moves to Rust).
    fn open_fd(&self, uri: &str) -> Option<i32>;
    /// One line per track: `id<TAB>title<TAB>artist`. `None` if the query failed.
    fn query_tracks(&self) -> Option<String>;
    fn has_audio_permission(&self) -> bool;
    fn display_name(&self, uri: &str) -> Option<String>;
    fn show_session(&self, state: i32, title: &str, artist: &str, actions: i32);
    fn stop_service(&self);
    fn request_focus(&self);
    fn abandon_focus(&self);
    fn ui_changed(&self);
}

/// Work the caller must do after the observer returns (on another thread).
pub enum Followup {
    Advance,
}

struct Core {
    dir: Option<PathBuf>,
    playlist: Playlist,
    current: Option<Track>,
    /// Where to start `current` when Play is pressed (restored position).
    restore_ms: u64,
    screen: Screen,
    lang: Lang,
    red: bool,
    note: Note,
    /// Paused by a short audio focus loss, so it resumes by itself when focus returns.
    auto_paused: bool,
}

#[derive(Default)]
struct Outcome {
    start: Option<(Track, u64)>,
    effects: i32,
}

pub struct Controller {
    engine: Engine,
    platform: Arc<dyn Platform>,
    core: Mutex<Core>,
}

impl Controller {
    pub fn new(engine: Engine, platform: Arc<dyn Platform>) -> Controller {
        Controller {
            engine,
            platform,
            core: Mutex::new(Core {
                dir: None,
                playlist: Playlist::new(),
                current: None,
                restore_ms: 0,
                screen: Screen::Player,
                lang: Lang::Ru,
                red: false,
                note: Note::None,
                auto_paused: false,
            }),
        }
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    fn core(&self) -> MutexGuard<'_, Core> {
        self.core.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Once per process: remembers where the saved state lives and restores the last track and
    /// position. Nothing starts playing by itself.
    pub fn init(&self, dir: &str) {
        let mut core = self.core();
        if core.dir.is_some() {
            return;
        }
        let dir = PathBuf::from(dir);
        if let Some(last) = storage::load(&dir) {
            core.current = Some(Track { uri: last.uri, title: last.title, artist: last.artist });
            core.restore_ms = last.position_ms;
        }
        core.dir = Some(dir);
    }

    // ---- input from the screen ---------------------------------------------------------

    /// A button or list row was pressed. Returns effects for Java.
    pub fn click(&self, id: i32) -> i32 {
        let mut out = Outcome::default();
        {
            let mut core = self.core();
            match id {
                ID_PREV => out.start = Self::step(&mut core, false),
                ID_PLAY_PAUSE => out = self.toggle(&mut core),
                ID_NEXT => out.start = Self::step(&mut core, true),
                ID_SHUFFLE => {
                    let on = !core.playlist.shuffle();
                    core.playlist.set_shuffle(on);
                }
                ID_REPEAT => core.playlist.cycle_repeat(),
                ID_MUSIC | ID_REFRESH => out.effects = self.open_library(&mut core),
                ID_BACK => core.screen = Screen::Player,
                ID_LANG => {
                    core.lang = match core.lang {
                        Lang::Ru => Lang::En,
                        Lang::En => Lang::Ru,
                    }
                }
                ID_RED => core.red = !core.red,
                i if i >= ID_ITEM_BASE => out.start = Self::select(&mut core, (i - ID_ITEM_BASE) as usize),
                _ => {}
            }
        }
        let mut effects = out.effects;
        if let Some((track, at)) = out.start {
            effects |= self.start(track, at);
        }
        self.platform.ui_changed();
        effects
    }

    /// The audio permission dialog was answered.
    pub fn permission_result(&self, granted: bool) {
        {
            let mut core = self.core();
            if granted {
                self.load_library(&mut core);
            } else {
                core.note = Note::Denied;
            }
        }
        self.platform.ui_changed();
    }

    /// Android asked BLACK WOOF to open an audio file (ACTION_VIEW).
    pub fn open_external(&self, uri: &str) -> i32 {
        let title = self.platform.display_name(uri).map(|n| strip_extension(&n)).unwrap_or_default();
        let track = Track { uri: uri.to_string(), title, artist: String::new() };
        {
            let mut core = self.core();
            core.current = Some(track.clone());
            core.screen = Screen::Player;
        }
        let effects = self.start(track, 0);
        self.platform.ui_changed();
        effects
    }

    /// A command from the notification, the lock screen or a headset button.
    pub fn command(&self, cmd: i32) -> i32 {
        let mut out = Outcome::default();
        {
            let mut core = self.core();
            match cmd {
                CMD_PLAY => {
                    if !matches!(self.engine.state().0, State::Playing | State::Opening) {
                        out = self.toggle(&mut core);
                    }
                }
                CMD_PAUSE => self.engine.pause(),
                CMD_TOGGLE => out = self.toggle(&mut core),
                CMD_NEXT => out.start = Self::step(&mut core, true),
                CMD_PREVIOUS => out.start = Self::step(&mut core, false),
                CMD_STOP => self.engine.stop(),
                _ => {}
            }
        }
        let mut effects = out.effects;
        if let Some((track, at)) = out.start {
            effects |= self.start(track, at);
        }
        self.platform.ui_changed();
        effects
    }

    /// The audio focus changed (or the request was refused). Rust decides what that means.
    pub fn on_focus(&self, change: i32) {
        let mut core = self.core();
        match change {
            FOCUS_GAIN => {
                self.engine.set_gain(1.0);
                if core.auto_paused {
                    core.auto_paused = false;
                    self.engine.resume();
                }
            }
            FOCUS_LOSS => {
                core.auto_paused = false;
                self.engine.pause();
                self.platform.abandon_focus();
            }
            FOCUS_LOSS_TRANSIENT => {
                if self.engine.state().0 == State::Playing {
                    core.auto_paused = true;
                    self.engine.pause();
                }
            }
            FOCUS_CAN_DUCK => self.engine.set_gain(DUCK_GAIN),
            FOCUS_DENIED => {
                core.auto_paused = false;
                self.engine.pause();
            }
            _ => {}
        }
    }

    // ---- events from the engine ---------------------------------------------------------

    /// The engine changed state (runs on the playback thread, after the file is closed for
    /// final states). Returns work to do on another thread: starting the next track here
    /// would join the very thread that is calling.
    pub fn on_engine_event(&self, state: State, error: Option<ErrorKind>) -> Option<Followup> {
        let mut followup = None;
        {
            let mut core = self.core();
            match state {
                State::Playing => self.push_session(&core, state),
                State::Paused => {
                    let pos = self.engine.position_ms();
                    self.save(&core, pos);
                    self.push_session(&core, state);
                }
                State::Ended => {
                    core.restore_ms = 0;
                    self.save(&core, 0);
                    let same = match (&core.current, core.playlist.current()) {
                        (Some(a), Some(b)) => a.uri == b.uri,
                        _ => false,
                    };
                    if same && core.playlist.can_advance_after_end() {
                        followup = Some(Followup::Advance);
                    } else {
                        self.platform.stop_service();
                        self.platform.abandon_focus();
                    }
                }
                State::Stopped => {
                    let pos = self.position(&core);
                    self.save(&core, pos);
                    self.platform.stop_service();
                    self.platform.abandon_focus();
                }
                State::Failed => {
                    core.note = Note::Error(error.unwrap_or(ErrorKind::Internal));
                    self.platform.stop_service();
                    self.platform.abandon_focus();
                }
                State::Idle | State::Opening => {}
            }
        }
        self.platform.ui_changed();
        followup
    }

    /// Plays the next track after one ended by itself (runs on its own short-lived thread).
    pub fn advance(&self) {
        let next = {
            let mut core = self.core();
            let next = core.playlist.advance(true).cloned();
            if let Some(track) = &next {
                core.current = Some(track.clone());
            }
            next
        };
        match next {
            Some(track) => {
                self.start(track, 0);
            }
            None => {
                self.platform.stop_service();
                self.platform.abandon_focus();
            }
        }
        self.platform.ui_changed();
    }

    /// The service started (or restarted): hand it the current picture. False if nothing is
    /// playing, so the service has no reason to run.
    pub fn service_ready(&self) -> bool {
        let state = self.engine.state().0;
        if !state.is_active() {
            return false;
        }
        {
            let core = self.core();
            self.push_session(&core, state);
        }
        self.platform.request_focus();
        true
    }

    /// Clean close: remember where we are.
    pub fn save_now(&self) {
        let core = self.core();
        let pos = self.position(&core);
        self.save(&core, pos);
    }

    pub fn shutdown(&self) {
        self.engine.shutdown();
    }

    // ---- what the screen shows ----------------------------------------------------------

    pub fn screen(&self) -> String {
        let state = self.engine.state().0;
        let core = self.core();
        let (title, artist) = match &core.current {
            Some(t) => (t.title.as_str(), t.artist.as_str()),
            None => ("", ""),
        };
        ui::render(&View {
            screen: core.screen,
            lang: core.lang,
            red: core.red,
            title,
            artist,
            playing: state == State::Playing,
            shuffle: core.playlist.shuffle(),
            repeat: core.playlist.repeat(),
            note: core.note,
            list_len: core.playlist.len(),
        })
    }

    pub fn item(&self, index: usize) -> String {
        let core = self.core();
        match core.playlist.track(index) {
            Some(track) => ui::item_text(track, core.lang),
            None => String::new(),
        }
    }

    pub fn item_count(&self) -> usize {
        self.core().playlist.len()
    }

    // ---- internals ---------------------------------------------------------------------

    fn toggle(&self, core: &mut Core) -> Outcome {
        match self.engine.state().0 {
            State::Playing | State::Opening => {
                self.engine.pause();
                Outcome::default()
            }
            State::Paused => {
                core.auto_paused = false;
                self.platform.request_focus();
                self.engine.resume();
                Outcome::default()
            }
            _ => match core.current.clone() {
                Some(track) => Outcome { start: Some((track, core.restore_ms)), effects: 0 },
                None => {
                    core.note = Note::NoTrack;
                    Outcome::default()
                }
            },
        }
    }

    fn step(core: &mut Core, forward: bool) -> Option<(Track, u64)> {
        let next = if forward {
            core.playlist.advance(false).cloned()
        } else {
            core.playlist.previous().cloned()
        };
        let track = next?;
        core.current = Some(track.clone());
        Some((track, 0))
    }

    fn select(core: &mut Core, index: usize) -> Option<(Track, u64)> {
        let track = core.playlist.select(index)?.clone();
        core.current = Some(track.clone());
        core.screen = Screen::Player;
        Some((track, 0))
    }

    fn open_library(&self, core: &mut Core) -> i32 {
        core.screen = Screen::Library;
        core.note = Note::None;
        if !self.platform.has_audio_permission() {
            return EFFECT_REQUEST_PERMISSION;
        }
        self.load_library(core);
        0
    }

    /// One query, only now (the user opened the list or pressed refresh); nothing runs later.
    fn load_library(&self, core: &mut Core) {
        match self.platform.query_tracks() {
            None => core.note = Note::Denied,
            Some(text) => {
                let tracks = parse_tracks(&text);
                core.note = if tracks.is_empty() { Note::Empty } else { Note::None };
                let current = core.current.as_ref().map(|t| t.uri.clone());
                core.playlist.set_tracks(tracks, current.as_deref());
            }
        }
    }

    /// Opens the file and hands the descriptor to the engine. Called without the core lock.
    fn start(&self, track: Track, start_ms: u64) -> i32 {
        let Some(fd) = self.platform.open_fd(&track.uri) else {
            self.set_note(Note::Unavailable);
            return 0;
        };
        let file = match take_fd(fd) {
            Ok(file) => file,
            Err(_) => {
                self.set_note(Note::Unavailable);
                return 0;
            }
        };
        if let Err(e) = self.engine.play_at(file, start_ms) {
            self.set_note(Note::Error(e.kind));
            return 0;
        }
        {
            let mut core = self.core();
            core.note = Note::None;
            core.restore_ms = 0;
            core.auto_paused = false;
            self.save(&core, start_ms);
        }
        self.platform.request_focus();
        EFFECT_START_SERVICE
    }

    fn set_note(&self, note: Note) {
        self.core().note = note;
    }

    fn push_session(&self, core: &Core, state: State) {
        let (title, artist) = match &core.current {
            Some(t) => (t.title.as_str(), t.artist.as_str()),
            None => ("", ""),
        };
        self.platform.show_session(
            media_session::playback_state(state),
            title,
            artist,
            media_session::actions(core.playlist.len()),
        );
    }

    /// Where to resume the current track: the saved position until something has played, 0
    /// after a track ended by itself, otherwise the live position.
    fn position(&self, core: &Core) -> u64 {
        match self.engine.state().0 {
            State::Idle => core.restore_ms,
            State::Ended => 0,
            _ => self.engine.position_ms(),
        }
    }

    fn save(&self, core: &Core, position_ms: u64) {
        let (Some(dir), Some(track)) = (core.dir.as_deref(), core.current.as_ref()) else {
            return;
        };
        write_last(dir, track, position_ms);
    }
}

fn write_last(dir: &Path, track: &Track, position_ms: u64) {
    let last = Last {
        uri: track.uri.clone(),
        position_ms,
        title: track.title.clone(),
        artist: track.artist.clone(),
    };
    let _ = storage::save(dir, &last);
}

/// `id<TAB>title<TAB>artist` per line (see `Platform::query_tracks`).
fn parse_tracks(text: &str) -> Vec<Track> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.splitn(3, '\t');
            let id = fields.next()?.trim();
            if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let title = fields.next().unwrap_or("").trim().to_string();
            let mut artist = fields.next().unwrap_or("").trim().to_string();
            if artist == "<unknown>" {
                artist.clear();
            }
            Some(Track {
                uri: format!("content://media/external/audio/media/{}", id),
                title,
                artist,
            })
        })
        .collect()
}

fn strip_extension(name: &str) -> String {
    match name.rfind('.') {
        Some(dot) if dot > 0 => name[..dot].to_string(),
        _ => name.to_string(),
    }
}
