//! Finding and opening the sound output device.
//!
//! Output devices differ in what they accept. Bluetooth headsets are the
//! awkward ones: they change sample rate and channel count when their
//! microphone goes on or off, allow only certain buffer sizes, and refuse
//! everything for a moment while they switch. So the default device is
//! offered several configurations in turn, and the whole sequence is
//! repeated a few times before giving up.

use anyhow::{anyhow, Result};
use rodio::cpal::traits::HostTrait;
use rodio::cpal::{self, BufferSize};
use rodio::{DeviceSinkBuilder, DeviceTrait, MixerDeviceSink};
use serde::Serialize;
use std::time::Duration;

/// How many times the sequence of configurations is tried, and how long to
/// wait in between for a device that is busy switching modes.
const ROUNDS: usize = 3;
const PAUSE: Duration = Duration::from_millis(250);

/// Runs `attempts` in order, and the whole list up to `rounds` times with
/// `pause` in between, until one succeeds. If none does, returns what went
/// wrong, each distinct message once, in the order first seen.
fn first_success<T>(
    rounds: usize,
    pause: Duration,
    attempts: &[&dyn Fn() -> Result<T, String>],
) -> Result<T, Vec<String>> {
    let mut problems: Vec<String> = Vec::new();
    for round in 0..rounds {
        if round > 0 {
            std::thread::sleep(pause);
        }
        for attempt in attempts {
            match attempt() {
                Ok(value) => return Ok(value),
                Err(problem) => {
                    if !problems.contains(&problem) {
                        problems.push(problem);
                    }
                }
            }
        }
    }
    Err(problems)
}

/// An error with its causes, on one line.
fn describe(err: impl std::error::Error + Send + Sync + 'static) -> String {
    format!("{:#}", anyhow::Error::from(err))
}

fn default_device() -> Result<cpal::Device, String> {
    cpal::default_host()
        .default_output_device()
        .ok_or_else(|| "no sound output device was found".to_string())
}

fn device_name(device: &cpal::Device) -> Option<String> {
    device
        .description()
        .ok()
        .map(|description| description.name().to_string())
}

/// The device's preferred format, with the buffer size the audio library
/// suggests (about 50 ms).
fn open_preferred() -> Result<MixerDeviceSink, String> {
    DeviceSinkBuilder::from_device(default_device()?)
        .and_then(DeviceSinkBuilder::open_stream)
        .map_err(describe)
}

/// The device's preferred format, with whatever buffer size the system
/// picks. Some devices do not allow the suggested one.
fn open_with_system_buffer() -> Result<MixerDeviceSink, String> {
    DeviceSinkBuilder::from_device(default_device()?)
        .map(|builder| builder.with_buffer_size(BufferSize::Default))
        .and_then(DeviceSinkBuilder::open_stream)
        .map_err(describe)
}

/// Every other format the device says it supports, best first.
fn open_any_supported() -> Result<MixerDeviceSink, String> {
    let device = default_device()?;
    let mut last = None;
    for config in rodio::stream::supported_output_configs(&device).map_err(describe)? {
        let opened = DeviceSinkBuilder::default()
            .with_device(device.clone())
            .with_supported_config(&config)
            .with_buffer_size(BufferSize::Default)
            .open_stream();
        match opened {
            Ok(sink) => return Ok(sink),
            Err(err) => last = Some(describe(err)),
        }
    }
    Err(last.unwrap_or_else(|| "the device reports no supported formats".to_string()))
}

/// Opens the system's default output device.
pub fn open_default() -> Result<MixerDeviceSink> {
    first_success(
        ROUNDS,
        PAUSE,
        &[
            &open_preferred,
            &open_with_system_buffer,
            &open_any_supported,
        ],
    )
    .map_err(|problems| {
        let device = default_device()
            .ok()
            .and_then(|device| device_name(&device));
        let through = match device {
            Some(name) => format!("through \u{201c}{name}\u{201d}"),
            None => "any sound".to_string(),
        };
        anyhow!("could not play {through}: {}", problems.join("; "))
    })
}

/// What the default output device is and whether it can be used. For
/// diagnosing sound problems on a particular machine.
#[derive(Debug, Clone, Serialize)]
pub struct OutputReport {
    /// Name of the default output device, if there is one.
    pub device: Option<String>,
    /// The format the device prefers.
    pub preferred_format: Option<String>,
    /// Every format the device says it supports.
    pub supported_formats: Vec<String>,
    /// Whether the device could be opened.
    pub usable: bool,
    /// The format it was opened with, or why it could not be opened.
    pub detail: String,
}

fn format_of(config: &cpal::SupportedStreamConfig) -> String {
    format!(
        "{} channel(s), {} Hz, {:?}, buffer {:?}",
        config.channels(),
        config.sample_rate(),
        config.sample_format(),
        config.buffer_size(),
    )
}

/// Examines the default output device and tries to open it, without
/// playing anything.
pub fn report() -> OutputReport {
    let device = default_device().ok();
    let preferred_format = device
        .as_ref()
        .and_then(|device| device.default_output_config().ok())
        .map(|config| format_of(&config));
    let supported_formats = device
        .as_ref()
        .and_then(|device| rodio::stream::supported_output_configs(device).ok())
        .map(|configs| configs.map(|config| format_of(&config)).collect())
        .unwrap_or_default();
    let (usable, detail) = match open_default() {
        Ok(mut sink) => {
            sink.log_on_drop(false);
            let config = sink.config();
            let detail = format!(
                "opened with {} channel(s), {} Hz, {:?}, buffer {:?}",
                config.channel_count(),
                config.sample_rate(),
                config.sample_format(),
                config.buffer_size(),
            );
            (true, detail)
        }
        Err(err) => (false, format!("{err:#}")),
    };
    OutputReport {
        device: device.as_ref().and_then(device_name),
        preferred_format,
        supported_formats,
        usable,
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn a_later_configuration_is_used_when_an_earlier_one_is_refused() {
        let refused = || Err::<u32, String>("buffer size not allowed".into());
        let accepted = || Ok::<u32, String>(7);
        let never = || -> Result<u32, String> { panic!("tried after one already worked") };
        let result = first_success(3, Duration::ZERO, &[&refused, &accepted, &never]);
        assert_eq!(result, Ok(7));
    }

    #[test]
    fn a_device_that_is_busy_at_first_is_tried_again() {
        let calls = Cell::new(0);
        // Refuses everything during the first round, as a headset does
        // while it switches modes.
        let busy_at_first = || {
            calls.set(calls.get() + 1);
            if calls.get() <= 2 {
                Err("device is busy".to_string())
            } else {
                Ok(calls.get())
            }
        };
        let result = first_success(3, Duration::ZERO, &[&busy_at_first, &busy_at_first]);
        assert_eq!(result, Ok(3));
    }

    #[test]
    fn every_distinct_problem_is_reported_once() {
        let first = || Err::<(), String>("buffer size not allowed".into());
        let second = || Err::<(), String>("format not supported".into());
        let result = first_success(3, Duration::ZERO, &[&first, &second, &first]);
        assert_eq!(
            result,
            Err(vec![
                "buffer size not allowed".to_string(),
                "format not supported".to_string()
            ])
        );
        let none: Result<(), _> = first_success(3, Duration::ZERO, &[]);
        assert_eq!(none, Err(vec![]));
    }

    #[test]
    fn the_report_never_fails_whatever_the_machine_has() {
        // Runs on machines with and without a sound card.
        let report = report();
        assert!(!report.detail.is_empty());
        assert_eq!(report.usable, report.detail.starts_with("opened with"));
        if report.device.is_none() {
            assert!(!report.usable);
        }
    }
}
