//! Test helpers (compiled for tests only).

use std::fs::File;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard};

/// Tests that open or inspect file descriptors hold this lock for their whole duration,
/// so a descriptor number cannot be recycled by a test running concurrently.
static FD_LOCK: Mutex<()> = Mutex::new(());

pub fn lock_fd() -> MutexGuard<'static, ()> {
    FD_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Deterministic test signal: a ramp, slightly different on every channel.
pub fn wav_sample(frame: u32, channel: u16) -> i16 {
    let ramp = (frame % 200) as i32 - 100;
    (ramp * 100 + (channel as i32) * 7) as i16
}

/// A 16-bit PCM WAV file held in memory.
pub fn wav_bytes(rate: u32, channels: u16, frames: u32) -> Vec<u8> {
    let data_len = frames * (channels as u32) * 2;
    let mut v = Vec::with_capacity(44 + data_len as usize);
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_len).to_le_bytes());
    v.extend_from_slice(b"WAVE");
    v.extend_from_slice(b"fmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes()); // PCM
    v.extend_from_slice(&channels.to_le_bytes());
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&(rate * (channels as u32) * 2).to_le_bytes()); // byte rate
    v.extend_from_slice(&(channels * 2).to_le_bytes()); // block align
    v.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_len.to_le_bytes());
    for frame in 0..frames {
        for channel in 0..channels {
            v.extend_from_slice(&wav_sample(frame, channel).to_le_bytes());
        }
    }
    v
}

/// Writes `bytes` to a temp file, opens it for reading and unlinks it: the returned
/// `File` is the only thing keeping the data alive.
pub fn open_temp(bytes: &[u8]) -> File {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let mut path = std::env::temp_dir();
    path.push(format!(
        "blackwoof-test-{}-{}.bin",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::write(&path, bytes).expect("write temp file");
    let file = File::open(&path).expect("open temp file");
    let _ = std::fs::remove_file(&path);
    file
}
