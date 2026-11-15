//! WAV files: written as 16-bit mono, read in the forms speech servers send.

use crate::engine::Audio;
use anyhow::{anyhow, bail, Result};
use std::io;
use std::path::Path;

/// `audio` as a 16-bit mono WAV file.
pub fn encode(audio: &Audio) -> Vec<u8> {
    let data_len = (audio.samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&audio.sample_rate.to_le_bytes());
    out.extend_from_slice(&(audio.sample_rate * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for &sample in &audio.samples {
        let value = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

pub fn write(path: &Path, audio: &Audio) -> io::Result<()> {
    std::fs::write(path, encode(audio))
}

const PCM: u16 = 1;
const FLOAT: u16 = 3;
const EXTENSIBLE: u16 = 0xFFFE;

/// Reads a WAV file: integer PCM of 8 to 32 bits or 32- and 64-bit float,
/// any number of channels, mixed down to mono.
///
/// A file written as a stream may not know its own length and say 0 or the
/// largest number instead; the audio then runs to the end of `bytes`.
pub fn decode(bytes: &[u8]) -> Result<Audio> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        bail!("not a WAV file");
    }
    let u16_at = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    let u32_at =
        |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    let mut format = None;
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let declared = u32_at(at + 4) as usize;
        let body = at + 8;
        let left = bytes.len() - body;
        if id == b"fmt " {
            if declared < 16 || left < declared {
                bail!("the WAV header is cut short");
            }
            let mut encoding = u16_at(body);
            let channels = u16_at(body + 2) as usize;
            let sample_rate = u32_at(body + 4);
            let bits = u16_at(body + 14) as usize;
            // The real encoding sits in the first two bytes of the
            // sub-format, 24 bytes into the chunk.
            if encoding == EXTENSIBLE && declared >= 26 {
                encoding = u16_at(body + 24);
            }
            format = Some((encoding, channels, sample_rate, bits));
        } else if id == b"data" {
            let (encoding, channels, sample_rate, bits) =
                format.ok_or_else(|| anyhow!("the WAV file has audio before its format"))?;
            if channels == 0 || sample_rate == 0 {
                bail!("the WAV file names no channels or no sample rate");
            }
            let length = if declared == 0 || declared > left {
                left
            } else {
                declared
            };
            let data = &bytes[body..body + length];
            let width = bits / 8;
            let sample: fn(&[u8]) -> f32 = match (encoding, bits) {
                (PCM, 8) => |b| (b[0] as f32 - 128.0) / 128.0,
                (PCM, 16) => |b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32_768.0,
                (PCM, 24) => |b| {
                    // Sign-extended through the top byte.
                    (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0
                },
                (PCM, 32) => {
                    |b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0
                }
                (FLOAT, 32) => |b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                (FLOAT, 64) => {
                    |b| f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as f32
                }
                _ => bail!("WAV audio of {bits} bits in encoding {encoding} is not supported"),
            };
            let samples = data
                .chunks_exact(width * channels)
                .map(|frame| frame.chunks_exact(width).map(sample).sum::<f32>() / channels as f32)
                .collect();
            return Ok(Audio {
                samples,
                sample_rate,
            });
        }
        // Chunks are padded to an even length.
        at = body.saturating_add(declared).saturating_add(declared & 1);
    }
    bail!("the WAV file holds no audio")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(encoding: u16, channels: u16, bits: u16, declared: u32, data: &[u8]) -> Vec<u8> {
        let mut out = b"RIFF\0\0\0\0WAVE".to_vec();
        // A chunk a reader has to step over, of odd length.
        out.extend_from_slice(b"LIST\x03\0\0\0abc\0");
        out.extend_from_slice(b"fmt \x10\0\0\0");
        out.extend_from_slice(&encoding.to_le_bytes());
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&8000u32.to_le_bytes());
        out.extend_from_slice(&[0; 6]);
        out.extend_from_slice(&bits.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&declared.to_le_bytes());
        out.extend_from_slice(data);
        out
    }

    #[test]
    fn what_is_written_reads_back() {
        let audio = Audio {
            samples: vec![0.0, 0.5, -0.5, 1.0, -1.0],
            sample_rate: 24_000,
        };
        let back = decode(&encode(&audio)).unwrap();
        assert_eq!(back.sample_rate, 24_000);
        for (a, b) in audio.samples.iter().zip(&back.samples) {
            assert!((a - b).abs() < 1e-4, "{a} became {b}");
        }
    }

    #[test]
    fn reads_the_forms_servers_send() {
        // Float, stereo: mixed down.
        let mut data = Vec::new();
        for value in [0.5f32, 0.25, -1.0, 0.0] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        let audio = decode(&wav(FLOAT, 2, 32, data.len() as u32, &data)).unwrap();
        assert_eq!(audio.samples, [0.375, -0.5]);
        assert_eq!(audio.sample_rate, 8000);
        // 24-bit, negative values included.
        let data = [0x00, 0x00, 0x40, 0x00, 0x00, 0xC0];
        let audio = decode(&wav(PCM, 1, 24, 6, &data)).unwrap();
        assert_eq!(audio.samples, [0.5, -0.5]);
        // A stream that did not know its length.
        let data = 16_384i16.to_le_bytes();
        for declared in [0, u32::MAX] {
            let audio = decode(&wav(PCM, 1, 16, declared, &data)).unwrap();
            assert_eq!(audio.samples, [0.5]);
        }
    }

    #[test]
    fn rejects_what_is_not_audio() {
        assert!(decode(b"").is_err());
        assert!(decode(b"<html>Internal Server Error</html>").is_err());
        assert!(decode(b"RIFF\0\0\0\0WAVE").is_err());
        // An encoding we do not read (A-law).
        assert!(decode(&wav(6, 1, 8, 1, &[0])).is_err());
        // Audio before the format, and a header cut short.
        assert!(decode(b"RIFF\0\0\0\0WAVEdata\x02\0\0\0\0\0").is_err());
        assert!(decode(b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0\x01\0").is_err());
    }
}
