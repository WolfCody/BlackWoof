//! Minimal AAudio binding (NDK, API 28+) and the `Sink` built on it.
//!
//! AAudio streams are not thread-safe, so a stream is created, used and closed on the
//! playback thread only. Output is blocking writes, so the thread sleeps while the system
//! drains the buffer. No logging: failures come back as `Error` values.

use std::os::raw::c_void;
use std::ptr;

use crate::audio_engine::Sink;
use crate::decoder::StreamInfo;
use crate::error::{Error, ErrorKind};

#[repr(C)]
struct AAudioStreamBuilder {
    _private: [u8; 0],
}

#[repr(C)]
struct AAudioStream {
    _private: [u8; 0],
}

// Values from <aaudio/AAudio.h>.
const OK: i32 = 0;
const DIRECTION_OUTPUT: i32 = 0;
const SHARING_SHARED: i32 = 1;
const FORMAT_PCM_I16: i32 = 1;
const PERFORMANCE_POWER_SAVING: i32 = 11;
const USAGE_MEDIA: i32 = 1;
const CONTENT_TYPE_MUSIC: i32 = 2;
const STATE_PAUSING: i32 = 5;
const STATE_STOPPING: i32 = 9;

/// Upper bound for one blocking write. The legacy (AudioTrack) path ignores it and blocks
/// until there is room; the MMAP path honours it. Either way `write_all` copes.
const WRITE_TIMEOUT_NS: i64 = 200_000_000;
const PAUSE_WAIT_NS: i64 = 100_000_000;
const DRAIN_SLICE_NS: i64 = 100_000_000;
/// 30 slices of 100 ms: the longest wait for the tail of the buffer to play out.
const DRAIN_MAX_SLICES: u32 = 30;

#[link(name = "aaudio")]
extern "C" {
    fn AAudio_createStreamBuilder(builder: *mut *mut AAudioStreamBuilder) -> i32;
    fn AAudioStreamBuilder_setDirection(builder: *mut AAudioStreamBuilder, direction: i32);
    fn AAudioStreamBuilder_setSharingMode(builder: *mut AAudioStreamBuilder, mode: i32);
    fn AAudioStreamBuilder_setSampleRate(builder: *mut AAudioStreamBuilder, rate: i32);
    fn AAudioStreamBuilder_setChannelCount(builder: *mut AAudioStreamBuilder, count: i32);
    fn AAudioStreamBuilder_setFormat(builder: *mut AAudioStreamBuilder, format: i32);
    fn AAudioStreamBuilder_setPerformanceMode(builder: *mut AAudioStreamBuilder, mode: i32);
    // API 28:
    fn AAudioStreamBuilder_setUsage(builder: *mut AAudioStreamBuilder, usage: i32);
    fn AAudioStreamBuilder_setContentType(builder: *mut AAudioStreamBuilder, content: i32);
    fn AAudioStreamBuilder_openStream(
        builder: *mut AAudioStreamBuilder,
        stream: *mut *mut AAudioStream,
    ) -> i32;
    fn AAudioStreamBuilder_delete(builder: *mut AAudioStreamBuilder) -> i32;

    fn AAudioStream_close(stream: *mut AAudioStream) -> i32;
    fn AAudioStream_requestStart(stream: *mut AAudioStream) -> i32;
    fn AAudioStream_requestPause(stream: *mut AAudioStream) -> i32;
    fn AAudioStream_requestStop(stream: *mut AAudioStream) -> i32;
    fn AAudioStream_getState(stream: *mut AAudioStream) -> i32;
    fn AAudioStream_waitForStateChange(
        stream: *mut AAudioStream,
        input_state: i32,
        next_state: *mut i32,
        timeout_ns: i64,
    ) -> i32;
    fn AAudioStream_write(
        stream: *mut AAudioStream,
        buffer: *const c_void,
        num_frames: i32,
        timeout_ns: i64,
    ) -> i32;
    fn AAudioStream_getSampleRate(stream: *mut AAudioStream) -> i32;
    fn AAudioStream_getChannelCount(stream: *mut AAudioStream) -> i32;
    fn AAudioStream_getFormat(stream: *mut AAudioStream) -> i32;
    fn AAudioStream_getBufferCapacityInFrames(stream: *mut AAudioStream) -> i32;
    fn AAudioStream_setBufferSizeInFrames(stream: *mut AAudioStream, frames: i32) -> i32;
}

pub struct AAudioSink {
    stream: *mut AAudioStream,
    channels: usize,
    /// `requestStart` succeeded and the stream has not been stopped since.
    started: bool,
}

impl AAudioSink {
    pub fn new() -> AAudioSink {
        AAudioSink { stream: ptr::null_mut(), channels: 0, started: false }
    }
}

impl Sink for AAudioSink {
    fn open(&mut self, info: StreamInfo) -> Result<(), Error> {
        self.close();
        let failed = Error::new(ErrorKind::OutputOpen);
        // SAFETY: plain NDK calls. `builder` is deleted before leaving the block and the
        // stream pointer is stored only after the format was verified.
        unsafe {
            let mut builder: *mut AAudioStreamBuilder = ptr::null_mut();
            if AAudio_createStreamBuilder(&mut builder) != OK || builder.is_null() {
                return Err(failed);
            }
            AAudioStreamBuilder_setDirection(builder, DIRECTION_OUTPUT);
            AAudioStreamBuilder_setSharingMode(builder, SHARING_SHARED);
            // Music, not games: battery over latency (the system picks the buffer sizes).
            AAudioStreamBuilder_setPerformanceMode(builder, PERFORMANCE_POWER_SAVING);
            AAudioStreamBuilder_setUsage(builder, USAGE_MEDIA);
            AAudioStreamBuilder_setContentType(builder, CONTENT_TYPE_MUSIC);
            AAudioStreamBuilder_setFormat(builder, FORMAT_PCM_I16);
            AAudioStreamBuilder_setSampleRate(builder, info.sample_rate as i32);
            AAudioStreamBuilder_setChannelCount(builder, info.channels as i32);

            let mut stream: *mut AAudioStream = ptr::null_mut();
            let result = AAudioStreamBuilder_openStream(builder, &mut stream);
            AAudioStreamBuilder_delete(builder);
            if result != OK || stream.is_null() {
                return Err(failed);
            }

            // Refuse a stream that does not match the data: a different rate would play
            // at the wrong speed. The error shows up on the first device test.
            let same = AAudioStream_getSampleRate(stream) == info.sample_rate as i32
                && AAudioStream_getChannelCount(stream) == info.channels as i32
                && AAudioStream_getFormat(stream) == FORMAT_PCM_I16;
            if !same {
                AAudioStream_close(stream);
                return Err(failed);
            }

            // Use the whole buffer: fewer wake-ups, more tolerance to a busy CPU.
            let capacity = AAudioStream_getBufferCapacityInFrames(stream);
            if capacity > 0 {
                AAudioStream_setBufferSizeInFrames(stream, capacity);
            }
            self.stream = stream;
        }
        self.channels = info.channels as usize;
        self.started = false;
        Ok(())
    }

    fn start(&mut self) -> Result<(), Error> {
        let failed = Error::new(ErrorKind::OutputControl);
        if self.stream.is_null() {
            return Err(failed);
        }
        // SAFETY: `stream` is a live stream owned by this sink.
        if unsafe { AAudioStream_requestStart(self.stream) } != OK {
            return Err(failed);
        }
        self.started = true;
        Ok(())
    }

    fn pause(&mut self) -> Result<(), Error> {
        let failed = Error::new(ErrorKind::OutputControl);
        if self.stream.is_null() {
            return Err(failed);
        }
        // SAFETY: `stream` is a live stream owned by this sink.
        if unsafe { AAudioStream_requestPause(self.stream) } != OK {
            return Err(failed);
        }
        Ok(())
    }

    fn write(&mut self, samples: &[i16]) -> Result<usize, Error> {
        if self.stream.is_null() || self.channels == 0 {
            return Err(Error::new(ErrorKind::OutputWrite));
        }
        let frames = (samples.len() / self.channels).min(i32::MAX as usize) as i32;
        if frames == 0 {
            return Ok(0);
        }
        // SAFETY: `samples` holds at least `frames * channels` i16 values and outlives the
        // call; the stream was opened as 16-bit PCM with `channels` channels.
        let written = unsafe {
            AAudioStream_write(
                self.stream,
                samples.as_ptr() as *const c_void,
                frames,
                WRITE_TIMEOUT_NS,
            )
        };
        if written < 0 {
            Err(Error::new(ErrorKind::OutputWrite))
        } else {
            Ok(written as usize)
        }
    }

    fn drain(&mut self, should_abort: &dyn Fn() -> bool) {
        if self.stream.is_null() || !self.started {
            return;
        }
        // SAFETY: `stream` is a live stream owned by this sink. For an output stream,
        // requestStop lets the data already in the buffer play out before stopping.
        unsafe {
            if AAudioStream_requestStop(self.stream) != OK {
                return;
            }
            self.started = false;
            let mut state = AAudioStream_getState(self.stream);
            let mut slices = 0u32;
            while state == STATE_STOPPING && slices < DRAIN_MAX_SLICES && !should_abort() {
                let mut next: i32 = state;
                if AAudioStream_waitForStateChange(self.stream, state, &mut next, DRAIN_SLICE_NS)
                    == OK
                {
                    state = next;
                } else {
                    slices += 1;
                    state = AAudioStream_getState(self.stream);
                }
            }
        }
    }

    fn close(&mut self) {
        if self.stream.is_null() {
            return;
        }
        // SAFETY: `stream` is a live stream owned by this sink and is not used after close.
        unsafe {
            if self.started && AAudioStream_requestPause(self.stream) == OK {
                // Let the pause ramp finish so the cut is not abrupt (bounded wait).
                let state = AAudioStream_getState(self.stream);
                if state == STATE_PAUSING {
                    let mut next: i32 = state;
                    let _ = AAudioStream_waitForStateChange(
                        self.stream,
                        state,
                        &mut next,
                        PAUSE_WAIT_NS,
                    );
                }
            }
            let _ = AAudioStream_close(self.stream);
        }
        self.stream = ptr::null_mut();
        self.started = false;
    }
}

impl Drop for AAudioSink {
    fn drop(&mut self) {
        self.close();
    }
}
