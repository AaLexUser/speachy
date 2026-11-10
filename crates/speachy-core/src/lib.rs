//! Speachy core: local text-to-speech with a streaming, gapless pipeline.
//!
//! * [`catalog`] — the downloadable models and their voices
//! * [`models`] — downloading and installing models
//! * [`text`] — splitting text into speakable chunks
//! * [`lang`] — language detection
//! * [`plan`] — choosing a voice for every chunk
//! * [`engine`] — synthesis (sherpa-onnx)
//! * [`audio`] — gapless audio output
//! * [`pipeline`] — the synthesize-ahead speech pipeline

pub mod audio;
pub mod catalog;
pub mod engine;
pub mod lang;
pub mod models;
pub mod numbers;
pub mod pipeline;
pub mod plan;
pub mod settings;
pub mod text;
pub mod wav;
