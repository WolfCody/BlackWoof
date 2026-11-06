//! Symphonia-based decoder. It takes ownership of the open file (so the descriptor is
//! closed when the decoder is dropped, whatever the path) and produces interleaved i16 PCM.
//!
//! Failures are `Result` values. Damaged packets are skipped; only
//! `MAX_CONSECUTIVE_ERRORS` bad packets in a row end the track with `Corrupt`.

use std::fs::File;
use std::io::ErrorKind as IoKind;

use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::codecs::CodecParameters;
use symphonia::core::common::Limit;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::default::{get_codecs, get_probe};

use crate::error::{Error, ErrorKind};

/// About 6 seconds of MP3 frames; enough to cross a damaged region, finite on garbage.
const MAX_CONSECUTIVE_ERRORS: u32 = 256;
const MIN_RATE: u32 = 8_000;
const MAX_RATE: u32 = 192_000;
/// Stage 2 supports mono and stereo; anything else is reported as `Unsupported`.
const MAX_CHANNELS: u32 = 2;

/// Format of the decoded stream, taken from the first decoded packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamInfo {
    pub sample_rate: u32,
    pub channels: u32,
}

enum Step {
    /// `scratch` holds one packet of interleaved samples.
    Samples,
    /// The stream is over.
    End,
}

pub struct Decoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    info: StreamInfo,
    /// Last decoded packet (interleaved).
    scratch: Vec<i16>,
    /// `scratch` holds a packet nobody has taken yet (the one decoded by `open`).
    scratch_ready: bool,
    ended: bool,
}

impl Decoder {
    /// Probes `file`, picks the first audio track and decodes its first packet so that
    /// the real stream format is known before the audio output is opened.
    /// `file` is dropped (descriptor closed) on every error path.
    pub fn open(file: File) -> Result<Decoder, Error> {
        let mss = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());
        // Cover art is never wanted. The metadata log lives as long as the format reader, so
        // without a limit a picture would stay in memory for the whole track. A limit of one
        // byte makes the readers skip every picture instead of loading it.
        let meta_opts = MetadataOptions::default().limit_visual_bytes(Limit::Maximum(1));
        let format = get_probe()
            .probe(&Hint::new(), mss, FormatOptions::default(), meta_opts)
            .map_err(|e| match e {
                SymError::IoError(_) => Error::new(ErrorKind::Io),
                SymError::Unsupported(_) => Error::new(ErrorKind::UnknownFormat),
                _ => Error::new(ErrorKind::Corrupt),
            })?;

        let (track_id, decoder) = {
            let track = format
                .tracks()
                .iter()
                .find(|t| t.codec_params.as_ref().is_some_and(CodecParameters::is_audio))
                .ok_or(Error::new(ErrorKind::NoAudio))?;
            let params = track
                .codec_params
                .as_ref()
                .and_then(CodecParameters::audio)
                .ok_or(Error::new(ErrorKind::NoAudio))?;
            let decoder = get_codecs()
                .make_audio_decoder(params, &AudioDecoderOptions::default())
                .map_err(|e| match e {
                    SymError::Unsupported(_) => Error::new(ErrorKind::Unsupported),
                    _ => Error::new(ErrorKind::Corrupt),
                })?;
            (track.id, decoder)
        };

        let mut dec = Decoder {
            format,
            decoder,
            track_id,
            info: StreamInfo { sample_rate: 0, channels: 0 },
            scratch: Vec::new(),
            scratch_ready: false,
            ended: false,
        };
        match dec.step()? {
            Step::Samples => {
                dec.scratch_ready = true;
                Ok(dec)
            }
            Step::End => Err(Error::new(ErrorKind::NoAudio)),
        }
    }

    /// Stream format. Valid after `open`.
    pub fn info(&self) -> StreamInfo {
        self.info
    }

    /// Appends about `target_frames` frames of interleaved samples to `out`.
    ///
    /// `Ok(true)`: more data may follow. `Ok(false)`: end of stream (`out` may still have
    /// received samples in this call). `Err`: fatal error (`out` keeps what was decoded
    /// before it, and the caller should play that first).
    pub fn read(&mut self, out: &mut Vec<i16>, target_frames: usize) -> Result<bool, Error> {
        let target = target_frames.saturating_mul(self.info.channels as usize);
        let start = out.len();
        if self.scratch_ready {
            out.extend_from_slice(&self.scratch);
            self.scratch_ready = false;
        }
        if self.ended {
            return Ok(false);
        }
        while out.len() - start < target {
            match self.step()? {
                Step::Samples => out.extend_from_slice(&self.scratch),
                Step::End => {
                    self.ended = true;
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    /// Decodes the next packet of the chosen track into `scratch`.
    fn step(&mut self) -> Result<Step, Error> {
        let mut failures: u32 = 0;
        loop {
            let packet = match self.format.next_packet() {
                Ok(Some(p)) => p,
                Ok(None) => return Ok(Step::End),
                Err(SymError::IoError(e)) => {
                    // A truncated file ends the track; any other read error is fatal.
                    return if e.kind() == IoKind::UnexpectedEof {
                        Ok(Step::End)
                    } else {
                        Err(Error::new(ErrorKind::Io))
                    };
                }
                // A chained stream with new parameters: end the track.
                Err(SymError::ResetRequired) => return Ok(Step::End),
                Err(SymError::DecodeError(_)) => {
                    failures += 1;
                    if failures > MAX_CONSECUTIVE_ERRORS {
                        return Err(Error::new(ErrorKind::Corrupt));
                    }
                    continue;
                }
                Err(SymError::Unsupported(_)) => return Err(Error::new(ErrorKind::Unsupported)),
                Err(_) => return Err(Error::new(ErrorKind::Corrupt)),
            };
            if packet.track_id != self.track_id {
                continue;
            }
            match self.decoder.decode(&packet) {
                Ok(buf) => {
                    if buf.frames() == 0 {
                        continue;
                    }
                    let spec = buf.spec();
                    let rate = spec.rate();
                    let channels = spec.channels().count() as u32;
                    if self.info.sample_rate == 0 {
                        let rate_ok = (MIN_RATE..=MAX_RATE).contains(&rate);
                        if !rate_ok || channels == 0 || channels > MAX_CHANNELS {
                            return Err(Error::new(ErrorKind::Unsupported));
                        }
                        self.info = StreamInfo { sample_rate: rate, channels };
                    } else if rate != self.info.sample_rate || channels != self.info.channels {
                        // The format changed in the middle of the stream: end the track.
                        return Ok(Step::End);
                    }
                    self.scratch.clear();
                    buf.copy_to_vec_interleaved(&mut self.scratch);
                    return Ok(Step::Samples);
                }
                Err(SymError::DecodeError(_)) | Err(SymError::ResetRequired) => {
                    failures += 1;
                    if failures > MAX_CONSECUTIVE_ERRORS {
                        return Err(Error::new(ErrorKind::Corrupt));
                    }
                }
                Err(SymError::IoError(_)) => return Err(Error::new(ErrorKind::Io)),
                Err(SymError::Unsupported(_)) => return Err(Error::new(ErrorKind::Unsupported)),
                Err(_) => return Err(Error::new(ErrorKind::Corrupt)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{lock_fd, open_temp, wav_bytes, wav_sample};

    /// Reads a decoder to the end. Fails the test if it never terminates.
    fn drain(dec: &mut Decoder) -> (Vec<i16>, Option<Error>) {
        let mut all = Vec::new();
        let mut buf = Vec::new();
        for _ in 0..100_000 {
            buf.clear();
            let r = dec.read(&mut buf, 4096);
            all.extend_from_slice(&buf);
            match r {
                Ok(true) => {}
                Ok(false) => return (all, None),
                Err(e) => return (all, Some(e)),
            }
        }
        panic!("decoder did not terminate");
    }

    fn check_ramp(samples: &[i16], channels: usize) {
        for (i, s) in samples.iter().enumerate() {
            let want = wav_sample((i / channels) as u32, (i % channels) as u16);
            assert!((*s as i32 - want as i32).abs() <= 1, "sample {i}: got {s}, want {want}");
        }
    }

    #[test]
    fn stereo_wav_roundtrip() {
        let _g = lock_fd();
        let mut dec = Decoder::open(open_temp(&wav_bytes(44_100, 2, 10_000))).expect("open");
        assert_eq!(dec.info(), StreamInfo { sample_rate: 44_100, channels: 2 });
        let (samples, err) = drain(&mut dec);
        assert!(err.is_none(), "unexpected error {err:?}");
        assert_eq!(samples.len(), 10_000 * 2);
        check_ramp(&samples, 2);
    }

    #[test]
    fn mono_22k_wav_roundtrip() {
        let _g = lock_fd();
        let mut dec = Decoder::open(open_temp(&wav_bytes(22_050, 1, 5_000))).expect("open");
        assert_eq!(dec.info(), StreamInfo { sample_rate: 22_050, channels: 1 });
        let (samples, err) = drain(&mut dec);
        assert!(err.is_none(), "unexpected error {err:?}");
        assert_eq!(samples.len(), 5_000);
        check_ramp(&samples, 1);
    }

    #[test]
    fn more_than_two_channels_is_rejected() {
        // Either Symphonia refuses the header or our own channel limit does; both are errors.
        let _g = lock_fd();
        assert!(Decoder::open(open_temp(&wav_bytes(44_100, 6, 2_000))).is_err());
    }

    #[test]
    fn text_is_not_audio() {
        let _g = lock_fd();
        let bytes = b"This is not an audio file.\n".repeat(2000);
        assert!(Decoder::open(open_temp(&bytes)).is_err());
    }

    #[test]
    fn empty_file_is_rejected() {
        let _g = lock_fd();
        assert!(Decoder::open(open_temp(&[])).is_err());
    }

    #[test]
    fn truncated_wav_terminates_without_panic() {
        let _g = lock_fd();
        let mut bytes = wav_bytes(44_100, 2, 50_000);
        bytes.truncate(44 + 20_000 * 4);
        if let Ok(mut dec) = Decoder::open(open_temp(&bytes)) {
            let (samples, _err) = drain(&mut dec);
            assert!(samples.len() <= 20_000 * 2);
        }
    }

    /// Runs only when `BW_FIXTURES` points at a directory with real MP3/FLAC/OGG/WAV files
    /// (CI generates them with ffmpeg; ffmpeg is not part of the app).
    #[test]
    fn real_fixtures_when_available() {
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
        assert!(!paths.is_empty(), "no fixtures in {dir:?}");
        for path in paths {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string();
            let (rate, channels) =
                if name.starts_with("mono22k") { (22_050u32, 1u32) } else { (44_100u32, 2u32) };
            let file = File::open(&path).expect("open fixture");
            let mut dec = match Decoder::open(file) {
                Ok(d) => d,
                Err(e) => panic!("{name}: open failed: {e}"),
            };
            assert_eq!(dec.info(), StreamInfo { sample_rate: rate, channels }, "{name}: format");
            let (samples, err) = drain(&mut dec);
            assert!(err.is_none(), "{name}: decode error {err:?}");
            let frames = samples.len() / channels as usize;
            let want = rate as usize * 3;
            let slack = rate as usize / 5;
            assert!(
                frames + slack >= want && frames <= want + slack,
                "{name}: {frames} frames, expected about {want}"
            );
        }
    }
}
