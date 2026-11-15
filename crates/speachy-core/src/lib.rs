//! Speachy core: local text-to-speech with a streaming, gapless pipeline.
//!
//! * [`catalog`] — the models and their voices: built in, and the user's own
//! * [`models`] — downloading and installing models
//! * [`text`] — splitting text into speakable chunks
//! * [`lang`] — language detection
//! * [`plan`] — choosing a voice for every chunk
//! * [`engine`] — synthesis inside Speachy (sherpa-onnx), and the engines in use
//! * [`server`] — synthesis by a speech server
//! * [`audio`] — gapless audio output
//! * [`output`] — finding and opening the sound output device
//! * [`pipeline`] — the synthesize-ahead speech pipeline
//! * [`session`] — the front door: one object, driven by commands, that
//!   every front end (app, CLI, JSON channel) uses

pub mod audio;
pub mod catalog;
pub mod engine;
pub mod english;
pub mod lang;
pub mod models;
pub mod numbers;
pub mod output;
pub mod pipeline;
pub mod plan;
pub mod server;
pub mod session;
pub mod settings;
pub mod text;
pub mod wav;
