//! Minimal WAV (16-bit PCM, mono) writer.

use crate::engine::Audio;
use std::io::{self, Write};
use std::path::Path;

pub fn write(path: &Path, audio: &Audio) -> io::Result<()> {
    let data_len = (audio.samples.len() * 2) as u32;
    let mut out = io::BufWriter::new(std::fs::File::create(path)?);
    out.write_all(b"RIFF")?;
    out.write_all(&(36 + data_len).to_le_bytes())?;
    out.write_all(b"WAVEfmt ")?;
    out.write_all(&16u32.to_le_bytes())?;
    out.write_all(&1u16.to_le_bytes())?; // PCM
    out.write_all(&1u16.to_le_bytes())?; // mono
    out.write_all(&audio.sample_rate.to_le_bytes())?;
    out.write_all(&(audio.sample_rate * 2).to_le_bytes())?; // byte rate
    out.write_all(&2u16.to_le_bytes())?; // block align
    out.write_all(&16u16.to_le_bytes())?; // bits per sample
    out.write_all(b"data")?;
    out.write_all(&data_len.to_le_bytes())?;
    for &sample in &audio.samples {
        let value = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        out.write_all(&value.to_le_bytes())?;
    }
    out.flush()
}
