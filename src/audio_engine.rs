//! Playback engine. One thread per track decodes and writes PCM to a `Sink`.
//!
//! No logging and no counters. Control is two flags behind one mutex; while paused the
//! thread sleeps on a condvar, so a paused or idle engine costs no CPU.
//!
//! Descriptor ownership: `Engine::play` takes the `File` by value. From that moment the
//! playback thread (or, if the thread cannot start, the failed `spawn`) owns it, and it is
//! closed exactly once, before the final state becomes visible to anyone.

use std::fs::File;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::decoder::{Decoder, StreamInfo};
use crate::error::{Error, ErrorKind};

/// Frames decoded and written per round. Larger means fewer wake-ups; to be tuned by
/// measurement in Stage 7.
pub const CHUNK_FRAMES: usize = 4096;
/// Output re-opens allowed per track (device switch, disconnect).
const MAX_REOPEN: u32 = 3;
/// Consecutive zero-progress writes before the output is re-opened.
const MAX_IDLE_WRITES: u32 = 50;
const IDLE_SLEEP: Duration = Duration::from_millis(10);

/// Audio output. Created, used and dropped on the playback thread only.
pub trait Sink {
    /// Opens the output for `info`. May be called again after `close` (re-open).
    fn open(&mut self, info: StreamInfo) -> Result<(), Error>;
    fn start(&mut self) -> Result<(), Error>;
    fn pause(&mut self) -> Result<(), Error>;
    /// Blocking write of interleaved samples. Returns the number of FRAMES accepted,
    /// which may be fewer than offered.
    fn write(&mut self, samples: &[i16]) -> Result<usize, Error>;
    /// Lets buffered audio play out, then stops. Gives up early if `should_abort()` is true.
    fn drain(&mut self, should_abort: &dyn Fn() -> bool);
    /// Stops at once and releases the output. Safe to call repeatedly.
    fn close(&mut self);
}

/// Creates the sink. Called on the playback thread, once per track.
pub type SinkFactory = Arc<dyn Fn() -> Box<dyn Sink> + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum State {
    Idle = 0,
    Opening = 1,
    Playing = 2,
    Paused = 3,
    Ended = 4,
    Stopped = 5,
    Failed = 6,
}

impl State {
    pub fn is_final(self) -> bool {
        matches!(self, State::Ended | State::Stopped | State::Failed)
    }
}

struct Control {
    pause: bool,
    stop: bool,
    state: State,
    error: Option<ErrorKind>,
}

struct Shared {
    ctl: Mutex<Control>,
    cv: Condvar,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    fn new() -> Shared {
        Shared {
            ctl: Mutex::new(Control {
                pause: false,
                stop: false,
                state: State::Opening,
                error: None,
            }),
            cv: Condvar::new(),
        }
    }

    /// `(stop, pause)`.
    fn flags(&self) -> (bool, bool) {
        let c = lock(&self.ctl);
        (c.stop, c.pause)
    }

    fn set_pause(&self, value: bool) {
        let mut c = lock(&self.ctl);
        c.pause = value;
        self.cv.notify_all();
    }

    fn set_stop(&self) {
        let mut c = lock(&self.ctl);
        c.stop = true;
        self.cv.notify_all();
    }

    fn set_state(&self, state: State) {
        let mut c = lock(&self.ctl);
        if !c.state.is_final() {
            c.state = state;
        }
        self.cv.notify_all();
    }

    fn finish(&self, state: State, error: Option<ErrorKind>) {
        let mut c = lock(&self.ctl);
        c.state = state;
        c.error = error;
        self.cv.notify_all();
    }

    /// Sleeps until playback is resumed or stopped. No polling.
    fn wait_while_paused(&self) {
        let mut c = lock(&self.ctl);
        while c.pause && !c.stop {
            c = self.cv.wait(c).unwrap_or_else(|e| e.into_inner());
        }
    }
}

struct Session {
    shared: Arc<Shared>,
    handle: JoinHandle<()>,
}

impl Session {
    fn stop_and_join(self) {
        self.shared.set_stop();
        let _ = self.handle.join();
    }
}

pub struct Engine {
    factory: SinkFactory,
    /// Serialises `play` and `shutdown` (they join threads).
    serial: Mutex<()>,
    session: Mutex<Option<Session>>,
}

impl Engine {
    pub fn new(factory: SinkFactory) -> Engine {
        Engine { factory, serial: Mutex::new(()), session: Mutex::new(None) }
    }

    /// Starts playing `file`, replacing whatever is playing. Takes ownership of the file:
    /// it is closed on every path (success, error, replaced, stopped, panic).
    pub fn play(&self, file: File) -> Result<(), Error> {
        let _serial = lock(&self.serial);
        let previous = lock(&self.session).take();
        if let Some(previous) = previous {
            previous.stop_and_join();
        }
        let shared = Arc::new(Shared::new());
        let thread_shared = Arc::clone(&shared);
        let factory = Arc::clone(&self.factory);
        let handle = thread::Builder::new()
            .name("bw-audio".to_string())
            .spawn(move || thread_main(thread_shared, file, factory))
            .map_err(|_| Error::new(ErrorKind::Internal))?;
        *lock(&self.session) = Some(Session { shared, handle });
        Ok(())
    }

    pub fn pause(&self) {
        if let Some(s) = lock(&self.session).as_ref() {
            s.shared.set_pause(true);
        }
    }

    pub fn resume(&self) {
        if let Some(s) = lock(&self.session).as_ref() {
            s.shared.set_pause(false);
        }
    }

    /// Requests a stop and returns at once; the state becomes `Stopped` when the thread
    /// has released the output and the file.
    pub fn stop(&self) {
        if let Some(s) = lock(&self.session).as_ref() {
            s.shared.set_stop();
        }
    }

    /// Current state and, after `Failed`, the reason. Overwritten by the next `play`;
    /// it is a state, not a history.
    pub fn state(&self) -> (State, Option<ErrorKind>) {
        match lock(&self.session).as_ref() {
            Some(s) => {
                let c = lock(&s.shared.ctl);
                (c.state, c.error)
            }
            None => (State::Idle, None),
        }
    }

    /// Stops playback and waits for the thread. For process teardown.
    pub fn shutdown(&self) {
        let _serial = lock(&self.serial);
        let previous = lock(&self.session).take();
        if let Some(previous) = previous {
            previous.stop_and_join();
        }
    }
}

fn thread_main(shared: Arc<Shared>, file: File, factory: SinkFactory) {
    let outcome = catch_unwind(AssertUnwindSafe(|| run(&shared, file, &*factory)));
    if outcome.is_err() {
        // The unwind has already dropped the decoder (file) and the sink.
        shared.finish(State::Failed, Some(ErrorKind::Internal));
    }
}

fn run(shared: &Shared, file: File, factory: &(dyn Fn() -> Box<dyn Sink> + Send + Sync)) {
    // `play_track` owns the file and the sink; both are gone when it returns,
    // so the final state is published only after the descriptor is closed.
    let (state, error) = match play_track(shared, file, factory) {
        Ok(state) => (state, None),
        Err(e) => (State::Failed, Some(e.kind)),
    };
    shared.finish(state, error);
}

struct Playback {
    /// The output has been started and not paused.
    started: bool,
    /// Pause is in effect (the thread is, or is about to be, asleep).
    paused: bool,
    reopen_left: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Flow {
    Go,
    Stop,
}

fn play_track(
    shared: &Shared,
    file: File,
    factory: &(dyn Fn() -> Box<dyn Sink> + Send + Sync),
) -> Result<State, Error> {
    let mut dec = Decoder::open(file)?;
    if shared.flags().0 {
        return Ok(State::Stopped);
    }
    let info = dec.info();
    let mut sink = factory();
    sink.open(info)?;
    let mut buf: Vec<i16> = Vec::with_capacity((CHUNK_FRAMES + 2048) * info.channels as usize);
    let mut pb = Playback { started: false, paused: false, reopen_left: MAX_REOPEN };
    let result = pump(shared, &mut dec, &mut *sink, &mut buf, info, &mut pb);
    if matches!(result, Ok(State::Ended)) {
        // Natural end: let the buffered tail play out (a stop request cuts it short).
        sink.drain(&|| shared.flags().0);
    }
    sink.close();
    result
}

fn pump(
    shared: &Shared,
    dec: &mut Decoder,
    sink: &mut dyn Sink,
    buf: &mut Vec<i16>,
    info: StreamInfo,
    pb: &mut Playback,
) -> Result<State, Error> {
    shared.set_state(State::Playing);
    loop {
        buf.clear();
        let read = dec.read(buf, CHUNK_FRAMES);
        // Play what was decoded even if the read then ended or failed.
        if !buf.is_empty() && write_all(shared, sink, buf, info, pb)? == Flow::Stop {
            return Ok(State::Stopped);
        }
        match read {
            Ok(true) => {}
            Ok(false) => return Ok(State::Ended),
            Err(e) => return Err(e),
        }
    }
}

/// Handles stop and pause between writes. Blocks (asleep) while paused.
fn gate(
    shared: &Shared,
    sink: &mut dyn Sink,
    info: StreamInfo,
    pb: &mut Playback,
) -> Result<Flow, Error> {
    loop {
        let (stop, pause) = shared.flags();
        if stop {
            return Ok(Flow::Stop);
        }
        if !pause {
            if pb.paused {
                pb.paused = false;
                if pb.started && sink.start().is_err() {
                    reopen(sink, info, pb, ErrorKind::OutputControl)?;
                }
                shared.set_state(State::Playing);
            }
            return Ok(Flow::Go);
        }
        if !pb.paused {
            pb.paused = true;
            if pb.started {
                // A failing pause (output already gone) is dealt with on resume.
                let _ = sink.pause();
            }
            shared.set_state(State::Paused);
        }
        shared.wait_while_paused();
    }
}

fn write_all(
    shared: &Shared,
    sink: &mut dyn Sink,
    samples: &[i16],
    info: StreamInfo,
    pb: &mut Playback,
) -> Result<Flow, Error> {
    let channels = info.channels as usize;
    let mut offset = 0usize;
    let mut idle = 0u32;
    while offset < samples.len() {
        if gate(shared, sink, info, pb)? == Flow::Stop {
            return Ok(Flow::Stop);
        }
        if !pb.started {
            if sink.start().is_err() {
                reopen(sink, info, pb, ErrorKind::OutputControl)?;
                continue;
            }
            pb.started = true;
        }
        match sink.write(&samples[offset..]) {
            Ok(0) => {
                idle += 1;
                if idle >= MAX_IDLE_WRITES {
                    idle = 0;
                    reopen(sink, info, pb, ErrorKind::OutputWrite)?;
                } else {
                    thread::sleep(IDLE_SLEEP);
                }
            }
            Ok(frames) => {
                offset = (offset + frames * channels).min(samples.len());
                idle = 0;
            }
            Err(_) => reopen(sink, info, pb, ErrorKind::OutputWrite)?,
        }
    }
    Ok(Flow::Go)
}

/// Closes and re-opens the output (device switch, disconnect). Bounded per track.
/// The caller starts it again on its next round (`started` is reset).
fn reopen(
    sink: &mut dyn Sink,
    info: StreamInfo,
    pb: &mut Playback,
    kind: ErrorKind,
) -> Result<(), Error> {
    if pb.reopen_left == 0 {
        return Err(Error::new(kind));
    }
    pb.reopen_left -= 1;
    sink.close();
    sink.open(info)?;
    pb.started = false;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fd::is_open;
    use crate::testutil::{lock_fd, open_temp, wav_bytes};
    use std::os::fd::AsRawFd;
    use std::time::Instant;

    #[derive(Default)]
    struct Rec {
        opens: Vec<StreamInfo>,
        starts: u32,
        pauses: u32,
        closes: u32,
        frames: usize,
        drained: bool,
    }

    #[derive(Clone, Copy, Default)]
    struct Cfg {
        delay_ms: u64,
        fail_open: bool,
        panic_open: bool,
        fail_all_writes: bool,
        /// 1-based index of one write call that fails (0 = none).
        fail_write_call: u32,
    }

    struct FakeSink {
        rec: Arc<Mutex<Rec>>,
        cfg: Cfg,
        channels: usize,
        calls: u32,
    }

    impl Sink for FakeSink {
        fn open(&mut self, info: StreamInfo) -> Result<(), Error> {
            if self.cfg.panic_open {
                panic!("deliberate test panic in Sink::open");
            }
            if self.cfg.fail_open {
                return Err(Error::new(ErrorKind::OutputOpen));
            }
            self.channels = info.channels as usize;
            lock(&self.rec).opens.push(info);
            Ok(())
        }

        fn start(&mut self) -> Result<(), Error> {
            lock(&self.rec).starts += 1;
            Ok(())
        }

        fn pause(&mut self) -> Result<(), Error> {
            lock(&self.rec).pauses += 1;
            Ok(())
        }

        fn write(&mut self, samples: &[i16]) -> Result<usize, Error> {
            self.calls += 1;
            if self.cfg.fail_all_writes || self.cfg.fail_write_call == self.calls {
                return Err(Error::new(ErrorKind::OutputWrite));
            }
            if self.cfg.delay_ms > 0 {
                thread::sleep(Duration::from_millis(self.cfg.delay_ms));
            }
            let frames = samples.len() / self.channels;
            lock(&self.rec).frames += frames;
            Ok(frames)
        }

        fn drain(&mut self, _should_abort: &dyn Fn() -> bool) {
            lock(&self.rec).drained = true;
        }

        fn close(&mut self) {
            lock(&self.rec).closes += 1;
        }
    }

    fn engine(rec: &Arc<Mutex<Rec>>, cfg: Cfg) -> Engine {
        let rec = Arc::clone(rec);
        Engine::new(Arc::new(move || -> Box<dyn Sink> {
            Box::new(FakeSink { rec: Arc::clone(&rec), cfg, channels: 1, calls: 0 })
        }))
    }

    /// Waits for a final state; fails the test instead of hanging.
    fn wait_final(e: &Engine, secs: u64) -> (State, Option<ErrorKind>) {
        let shared = match lock(&e.session).as_ref() {
            Some(s) => Arc::clone(&s.shared),
            None => return (State::Idle, None),
        };
        let deadline = Instant::now() + Duration::from_secs(secs);
        let mut c = lock(&shared.ctl);
        while !c.state.is_final() {
            let now = Instant::now();
            assert!(now < deadline, "engine did not finish in time (state {:?})", c.state);
            c = shared.cv.wait_timeout(c, deadline - now).unwrap_or_else(|e| e.into_inner()).0;
        }
        (c.state, c.error)
    }

    fn wait_until<F: Fn() -> bool>(secs: u64, f: F) {
        let end = Instant::now() + Duration::from_secs(secs);
        while !f() {
            assert!(Instant::now() < end, "condition not reached in time");
            thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn plays_to_the_end_drains_and_closes_the_fd() {
        let _g = lock_fd();
        let rec = Arc::new(Mutex::new(Rec::default()));
        let e = engine(&rec, Cfg::default());
        let file = open_temp(&wav_bytes(44_100, 2, 44_100));
        let raw = file.as_raw_fd();
        e.play(file).expect("play");
        assert_eq!(wait_final(&e, 20), (State::Ended, None));
        {
            let r = lock(&rec);
            assert_eq!(r.frames, 44_100);
            assert_eq!(r.opens, vec![StreamInfo { sample_rate: 44_100, channels: 2 }]);
            assert_eq!(r.starts, 1);
            assert!(r.drained);
            assert!(r.closes >= 1);
        }
        assert!(!is_open(raw), "descriptor must be closed after playback");
        e.shutdown();
    }

    #[test]
    fn undecodable_file_never_opens_the_output() {
        let _g = lock_fd();
        let rec = Arc::new(Mutex::new(Rec::default()));
        let e = engine(&rec, Cfg::default());
        let file = open_temp(&b"This is not an audio file.\n".repeat(2000));
        let raw = file.as_raw_fd();
        e.play(file).expect("play");
        let (state, error) = wait_final(&e, 20);
        assert_eq!(state, State::Failed);
        assert!(error.is_some());
        assert!(lock(&rec).opens.is_empty(), "output must stay closed for a bad file");
        assert!(!is_open(raw));
        e.shutdown();
    }

    #[test]
    fn stop_interrupts_playback_and_closes_the_fd() {
        let _g = lock_fd();
        let rec = Arc::new(Mutex::new(Rec::default()));
        let e = engine(&rec, Cfg { delay_ms: 20, ..Cfg::default() });
        let file = open_temp(&wav_bytes(44_100, 2, 441_000));
        let raw = file.as_raw_fd();
        e.play(file).expect("play");
        wait_until(5, || lock(&rec).frames > 0);
        e.stop();
        assert_eq!(wait_final(&e, 10), (State::Stopped, None));
        assert!(lock(&rec).frames < 441_000);
        assert!(!lock(&rec).drained, "a stop must not wait for the buffer to drain");
        assert!(!is_open(raw));
        e.shutdown();
    }

    #[test]
    fn pause_stops_writes_and_resume_continues() {
        let _g = lock_fd();
        let rec = Arc::new(Mutex::new(Rec::default()));
        let e = engine(&rec, Cfg { delay_ms: 20, ..Cfg::default() });
        e.play(open_temp(&wav_bytes(44_100, 2, 88_200))).expect("play");
        wait_until(5, || lock(&rec).frames > 0);
        e.pause();
        wait_until(5, || e.state().0 == State::Paused);
        let frozen = lock(&rec).frames;
        thread::sleep(Duration::from_millis(150));
        assert_eq!(lock(&rec).frames, frozen, "no writes while paused");
        assert_eq!(lock(&rec).pauses, 1);
        e.resume();
        assert_eq!(wait_final(&e, 20), (State::Ended, None));
        assert_eq!(lock(&rec).frames, 88_200);
        e.shutdown();
    }

    #[test]
    fn replacing_a_track_closes_the_previous_fd() {
        let _g = lock_fd();
        let rec = Arc::new(Mutex::new(Rec::default()));
        let e = engine(&rec, Cfg { delay_ms: 20, ..Cfg::default() });
        let first = open_temp(&wav_bytes(44_100, 2, 441_000));
        let first_fd = first.as_raw_fd();
        e.play(first).expect("play first");
        wait_until(5, || lock(&rec).frames > 0);
        e.play(open_temp(&wav_bytes(44_100, 2, 4_410))).expect("play second");
        assert!(!is_open(first_fd), "the replaced track's descriptor must be closed");
        assert_eq!(wait_final(&e, 20), (State::Ended, None));
        e.shutdown();
    }

    #[test]
    fn write_error_reopens_the_output_and_continues() {
        let _g = lock_fd();
        let rec = Arc::new(Mutex::new(Rec::default()));
        let e = engine(&rec, Cfg { fail_write_call: 3, ..Cfg::default() });
        e.play(open_temp(&wav_bytes(44_100, 2, 44_100))).expect("play");
        assert_eq!(wait_final(&e, 20), (State::Ended, None));
        let r = lock(&rec);
        assert_eq!(r.frames, 44_100, "the failed write must be retried, not skipped");
        assert_eq!(r.opens.len(), 2);
        assert_eq!(r.starts, 2);
        drop(r);
        e.shutdown();
    }

    #[test]
    fn persistent_write_errors_fail_after_bounded_reopens() {
        let _g = lock_fd();
        let rec = Arc::new(Mutex::new(Rec::default()));
        let e = engine(&rec, Cfg { fail_all_writes: true, ..Cfg::default() });
        e.play(open_temp(&wav_bytes(44_100, 2, 44_100))).expect("play");
        assert_eq!(wait_final(&e, 20), (State::Failed, Some(ErrorKind::OutputWrite)));
        assert_eq!(lock(&rec).opens.len() as u32, 1 + MAX_REOPEN);
        e.shutdown();
    }

    #[test]
    fn output_open_failure_is_reported_and_releases_the_fd() {
        let _g = lock_fd();
        let rec = Arc::new(Mutex::new(Rec::default()));
        let e = engine(&rec, Cfg { fail_open: true, ..Cfg::default() });
        let file = open_temp(&wav_bytes(44_100, 2, 4_410));
        let raw = file.as_raw_fd();
        e.play(file).expect("play");
        assert_eq!(wait_final(&e, 20), (State::Failed, Some(ErrorKind::OutputOpen)));
        assert!(!is_open(raw));
        e.shutdown();
    }

    #[test]
    fn a_panic_in_the_playback_thread_is_contained() {
        let _g = lock_fd();
        let rec = Arc::new(Mutex::new(Rec::default()));
        let e = engine(&rec, Cfg { panic_open: true, ..Cfg::default() });
        let file = open_temp(&wav_bytes(44_100, 2, 4_410));
        let raw = file.as_raw_fd();
        e.play(file).expect("play");
        assert_eq!(wait_final(&e, 20), (State::Failed, Some(ErrorKind::Internal)));
        assert!(!is_open(raw), "unwinding must close the descriptor");
        // The engine is still usable afterwards.
        let healthy = engine(&rec, Cfg::default());
        healthy.play(open_temp(&wav_bytes(44_100, 2, 4_410))).expect("play again");
        assert_eq!(wait_final(&healthy, 20), (State::Ended, None));
        healthy.shutdown();
        e.shutdown();
    }

    /// Runs only when `BW_FIXTURES` points at real MP3/FLAC/OGG/WAV files (see decoder tests).
    /// Truncated and damaged copies must always end in a final state, never hang or crash.
    #[test]
    fn damaged_fixtures_never_take_the_engine_down() {
        let dir = match std::env::var_os("BW_FIXTURES") {
            Some(d) => std::path::PathBuf::from(d),
            None => return,
        };
        let _g = lock_fd();
        let mut paths: Vec<_> = std::fs::read_dir(&dir)
            .expect("fixtures directory")
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .collect();
        paths.sort();
        assert!(!paths.is_empty());
        for path in paths {
            let bytes = std::fs::read(&path).expect("read fixture");
            let mut cut = bytes.clone();
            cut.truncate(bytes.len() * 6 / 10);
            let mut damaged = bytes.clone();
            let (from, to) = (bytes.len() * 3 / 10, bytes.len() * 5 / 10);
            for i in (from..to).step_by(7) {
                damaged[i] ^= 0xFF;
            }
            for data in [cut, damaged] {
                let rec = Arc::new(Mutex::new(Rec::default()));
                let e = engine(&rec, Cfg::default());
                e.play(open_temp(&data)).expect("play");
                let (state, _) = wait_final(&e, 60);
                assert!(state.is_final());
                e.shutdown();
            }
        }
    }
}
