//! What the system media controls (notification, lock screen, headset buttons) show.
//! Rust decides the content and which actions exist; the Java service only applies it.
//! Nothing here is sent more often than the state changes.

use crate::audio_engine::State;

/// Bits of the `actions` mask sent to Java.
pub const ACTION_PLAY_PAUSE: i32 = 1;
pub const ACTION_PREVIOUS: i32 = 2;
pub const ACTION_NEXT: i32 = 4;

/// Same numbers as `android.media.session.PlaybackState`.
pub const STATE_STOPPED: i32 = 1;
pub const STATE_PAUSED: i32 = 2;
pub const STATE_PLAYING: i32 = 3;

/// Previous and Next are offered only when the queue can really move.
pub fn actions(queue_len: usize) -> i32 {
    if queue_len > 1 {
        ACTION_PLAY_PAUSE | ACTION_PREVIOUS | ACTION_NEXT
    } else {
        ACTION_PLAY_PAUSE
    }
}

pub fn playback_state(state: State) -> i32 {
    match state {
        State::Playing => STATE_PLAYING,
        State::Paused => STATE_PAUSED,
        _ => STATE_STOPPED,
    }
}
