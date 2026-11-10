//! Headless front end to the Speachy engine.
//!
//! Everything here goes through the same [`Session`] and the same commands
//! as the desktop app, so behaviour seen here is the app's behaviour.

use clap::{Parser, Subcommand};
use serde_json::{json, Value};
use speachy_core::audio::{AudioBackend, CaptureAudio, SystemAudio};
use speachy_core::pipeline::{Outcome, SpeechEvent};
use speachy_core::session::{Command as EngineCommand, Event, Paths, Session};
use speachy_core::wav;
use std::io::{BufRead, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(
    name = "speachy-cli",
    version,
    about = "Drive the Speachy text-to-speech engine without the app",
    after_help = "By default this uses the same models and settings as the desktop app.\n\
                  Set --home (or SPEACHY_HOME) to work in a separate directory."
)]
struct Cli {
    /// Keep models and settings in this directory instead of the app's.
    #[arg(long, global = true, value_name = "DIR")]
    home: Option<PathBuf>,
    /// Print results (and events) as JSON.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List the available models and whether each is installed.
    Models,
    /// List voices, of one model or of every installed model.
    Voices {
        /// Model id (see `models`).
        model: Option<String>,
    },
    /// Download and install a model.
    Install { model: String },
    /// Remove an installed model.
    Remove { model: String },
    /// Show the settings, or change some: `settings speed=1.2 language=de`.
    ///
    /// Values are JSON; anything that does not parse as JSON is taken as a
    /// string. Nested fields use a dot: `shortcuts.stop=F9`.
    Settings {
        #[arg(value_name = "KEY=VALUE")]
        changes: Vec<String>,
    },
    /// Show how a text would be spoken: its chunks, and the language and
    /// voice of each. Nothing is synthesized.
    Plan {
        #[command(flatten)]
        request: Request,
    },
    /// Show which language the detector sees in a text.
    Detect {
        /// The text; read from standard input if omitted.
        text: Vec<String>,
    },
    /// Speak a text.
    Speak {
        #[command(flatten)]
        request: Request,
        /// Write the speech to a WAV file instead of playing it.
        #[arg(long, value_name = "FILE.wav")]
        out: Option<PathBuf>,
        /// Do not play or keep the audio (for timing and testing).
        #[arg(long, conflicts_with = "out")]
        no_audio: bool,
        /// With --out or --no-audio: consume audio at playback pace rather
        /// than instantly, to see the timing a listener would get.
        #[arg(long)]
        realtime: bool,
    },
    /// Run engine commands given as JSON, exactly as the app sends them:
    /// `run '{"cmd":"snapshot"}'`.
    Run {
        #[arg(value_name = "JSON", required = true)]
        commands: Vec<String>,
    },
    /// Serve commands over standard input and output, one JSON object per
    /// line, until input ends. Results and events are written as JSON lines.
    Serve {
        /// Use a silent sound card that plays at real-time pace.
        #[arg(long)]
        no_audio: bool,
    },
    /// Print where models and settings are kept.
    Paths,
}

#[derive(clap::Args)]
struct Request {
    /// The text; read from standard input if omitted or `-`.
    text: Vec<String>,
    /// Voice to use, as `model/voice` or just `voice`.
    #[arg(long, value_name = "VOICE")]
    voice: Option<String>,
    /// Read the text as this language (ISO 639-1) instead of detecting it.
    #[arg(long, value_name = "CODE")]
    lang: Option<String>,
}

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn read_text(words: &[String]) -> Result<String, String> {
    let from_args = words.join(" ");
    if !from_args.is_empty() && from_args != "-" {
        return Ok(from_args);
    }
    if from_args.is_empty() && std::io::stdin().is_terminal() {
        return Err("no text given (pass it as an argument or on standard input)".into());
    }
    let mut text = String::new();
    std::io::stdin()
        .read_to_string(&mut text)
        .map_err(|err| err.to_string())?;
    Ok(text)
}

/// Resolves `model/voice` or a bare voice id against the installed models.
fn voice_ref(session: &Session, spec: &str) -> Result<Value, String> {
    let installed = session.installed();
    let found = match spec.split_once('/') {
        Some((model, voice)) => installed
            .iter()
            .find(|m| m.id == model)
            .ok_or_else(|| format!("model {model} is not installed"))?
            .voice(voice)
            .map(|v| (model.to_string(), v.id.clone())),
        None => installed
            .iter()
            .find_map(|m| m.voice(spec).map(|v| (m.id.clone(), v.id.clone()))),
    };
    let (model_id, voice_id) =
        found.ok_or_else(|| format!("no installed voice matches {spec} (see `voices`)"))?;
    Ok(json!({ "model_id": model_id, "voice_id": voice_id }))
}

fn request_command(session: &Session, cmd: &str, request: &Request) -> Result<Value, String> {
    let voice = match &request.voice {
        Some(spec) => voice_ref(session, spec)?,
        None => Value::Null,
    };
    Ok(
        json!({ "cmd": cmd, "text": read_text(&request.text)?, "voice": voice, "lang": request.lang }),
    )
}

/// Writes one line to standard output, whole, even with events arriving
/// from other threads.
fn print_line(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// Describes an event for a person. Returns `None` for events that would
/// only be noise.
fn describe(event: &Event) -> Option<String> {
    Some(match event {
        Event::Speech(SpeechEvent::State { state }) => format!("state: {state:?}"),
        Event::Speech(SpeechEvent::Started { chunks, .. }) => {
            format!("speaking {} chunk(s)", chunks.len())
        }
        Event::Speech(SpeechEvent::ChunkReady {
            index,
            audio_secs,
            synth_secs,
            ..
        }) => {
            format!("chunk {index} ready: {audio_secs:.2} s of audio in {synth_secs:.2} s")
        }
        Event::Speech(SpeechEvent::ChunkPlaying { index, .. }) => format!("chunk {index} playing"),
        Event::Speech(SpeechEvent::Finished {
            outcome, message, ..
        }) => {
            format!(
                "finished: {outcome:?} {}",
                message.as_deref().unwrap_or_default()
            )
        }
        Event::Download(d) => match (&d.progress, d.finished) {
            (_, true) => format!("{}: {}", d.id, d.error.as_deref().unwrap_or("done")),
            (Some(p), _) => format!(
                "{}: {:?} {} / {} MB",
                d.id,
                p.phase,
                p.done_bytes / 1_000_000,
                p.total_bytes / 1_000_000
            ),
            (None, _) => return None,
        },
        Event::Settings(_) => return None,
        Event::Notice(message) => format!("notice: {message}"),
    })
}

fn run(cli: Cli) -> Result<(), String> {
    let paths = match &cli.home {
        Some(home) => Paths::under(home),
        None => Paths::resolve(),
    };
    let json_output = cli.json;

    // Which sound card, and what to do with events, depends on the command.
    let capture = match &cli.command {
        Cmd::Speak {
            out,
            no_audio,
            realtime,
            ..
        } if out.is_some() || *no_audio => {
            let pace = if *realtime { 1.0 } else { f32::INFINITY };
            Some(if out.is_some() {
                CaptureAudio::new(pace)
            } else {
                CaptureAudio::discarding(pace)
            })
        }
        Cmd::Serve { no_audio: true } => Some(CaptureAudio::discarding(1.0)),
        _ => None,
    };
    let audio: Arc<dyn AudioBackend> = match &capture {
        Some(capture) => Arc::new(capture.clone()),
        None => Arc::new(SystemAudio),
    };
    let show_events = matches!(
        cli.command,
        Cmd::Speak { .. } | Cmd::Install { .. } | Cmd::Serve { .. }
    );
    let serving = matches!(cli.command, Cmd::Serve { .. });
    let started = Instant::now();
    let failure: Arc<Mutex<Option<String>>> = Arc::default();
    let failed = failure.clone();
    let last_progress = Mutex::new(Instant::now());
    let session = Session::open(paths, audio, move |event| {
        if let Event::Speech(SpeechEvent::Finished {
            outcome: Outcome::Failed,
            message,
            ..
        }) = event
        {
            *failed.lock().unwrap() =
                Some(message.clone().unwrap_or_else(|| "speech failed".into()));
        }
        if !show_events {
            return;
        }
        if json_output || serving {
            if let Ok(line) = serde_json::to_string(event) {
                print_line(&line);
            }
            return;
        }
        // Download progress arrives ten times a second: thin it out.
        if let Event::Download(d) = event {
            let mut last = last_progress.lock().unwrap();
            if !d.finished && last.elapsed().as_millis() < 500 {
                return;
            }
            *last = Instant::now();
        }
        if let Some(text) = describe(event) {
            eprintln!("[{:7.2}s] {text}", started.elapsed().as_secs_f32());
        }
    });
    let result = run_command(cli.command, &session, json_output, capture, &failure);
    // Leaving while a thread is still inside a model would abort.
    if !session.shutdown(Duration::from_secs(30)) {
        eprintln!("warning: the engine did not shut down in time");
    }
    result
}

fn run_command(
    command: Cmd,
    session: &Session,
    json_output: bool,
    capture: Option<CaptureAudio>,
    failure: &Mutex<Option<String>>,
) -> Result<(), String> {
    let dispatch = |command: Value| session.dispatch_json(command);
    let print_json =
        |value: &Value| print_line(&serde_json::to_string_pretty(value).unwrap_or_default());

    match command {
        Cmd::Models => {
            let snapshot = dispatch(json!({ "cmd": "snapshot" }))?;
            if json_output {
                print_json(&snapshot["models"]);
            } else {
                for m in snapshot["models"].as_array().into_iter().flatten() {
                    let state = if m["installed"] == true {
                        "installed"
                    } else {
                        "-"
                    };
                    println!(
                        "{:<20} {:<10} {:>5} MB  {:<28} {}",
                        m["id"].as_str().unwrap_or_default(),
                        state,
                        m["size_bytes"].as_u64().unwrap_or(0) / 1_000_000,
                        m["name"].as_str().unwrap_or_default(),
                        m["languages"].as_array().map_or(0, Vec::len).to_string() + " language(s)",
                    );
                }
            }
        }
        Cmd::Voices { model } => {
            let snapshot = dispatch(json!({ "cmd": "snapshot" }))?;
            let models: Vec<&Value> = snapshot["models"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|m| match &model {
                    Some(id) => m["id"] == id.as_str(),
                    None => m["installed"] == true,
                })
                .collect();
            if models.is_empty() {
                return Err(match model {
                    Some(id) => format!("unknown model {id} (see `models`)"),
                    None => "no model is installed (see `models`, then `install`)".into(),
                });
            }
            if json_output {
                let voices: Vec<Value> = models
                    .iter()
                    .map(|m| json!({ "model": m["id"], "voices": m["voices"] }))
                    .collect();
                print_json(&json!(voices));
            } else {
                for m in models {
                    for v in m["voices"].as_array().into_iter().flatten() {
                        println!(
                            "{}/{:<18} {:<10} {:<7} {:<9} {}",
                            m["id"].as_str().unwrap_or_default(),
                            v["id"].as_str().unwrap_or_default(),
                            v["name"].as_str().unwrap_or_default(),
                            v["gender"].as_str().unwrap_or_default(),
                            v["accent"].as_str().unwrap_or("-"),
                            v["languages"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join(","),
                        );
                    }
                }
            }
        }
        Cmd::Install { model } => {
            dispatch(json!({ "cmd": "install_model", "id": model, "wait": true }))?;
            if json_output {
                print_json(&json!({ "installed": model }));
            } else {
                println!("installed {model}");
            }
        }
        Cmd::Remove { model } => {
            dispatch(json!({ "cmd": "remove_model", "id": model }))?;
            if json_output {
                print_json(&json!({ "removed": model }));
            } else {
                println!("removed {model}");
            }
        }
        Cmd::Settings { changes } => {
            let settings = if changes.is_empty() {
                dispatch(json!({ "cmd": "snapshot" }))?["settings"].take()
            } else {
                let mut patch = serde_json::Map::new();
                for change in &changes {
                    let (key, raw) = change
                        .split_once('=')
                        .ok_or_else(|| format!("expected KEY=VALUE, got {change}"))?;
                    let value = serde_json::from_str(raw)
                        .unwrap_or_else(|_| Value::String(raw.to_string()));
                    match key.split_once('.') {
                        Some((group, field)) => {
                            let group = patch.entry(group).or_insert_with(|| json!({}));
                            group
                                .as_object_mut()
                                .ok_or_else(|| format!("{key}: not a group of settings"))?
                                .insert(field.into(), value);
                        }
                        None => {
                            patch.insert(key.into(), value);
                        }
                    }
                }
                dispatch(json!({ "cmd": "update_settings", "patch": patch }))?["settings"].take()
            };
            print_json(&settings);
        }
        Cmd::Plan { request } => {
            let plan = dispatch(request_command(session, "plan", &request)?)?;
            if json_output {
                print_json(&plan["chunks"]);
            } else {
                for (i, c) in plan["chunks"].as_array().into_iter().flatten().enumerate() {
                    println!(
                        "{i:>3}  {}  {}/{}  [{}]  {}",
                        c["lang"].as_str().unwrap_or_default(),
                        c["model_id"].as_str().unwrap_or_default(),
                        c["voice_id"].as_str().unwrap_or_default(),
                        c["boundary"].as_str().unwrap_or_default(),
                        c["text"].as_str().unwrap_or_default(),
                    );
                }
            }
        }
        Cmd::Detect { text } => {
            let detected =
                dispatch(json!({ "cmd": "detect_language", "text": read_text(&text)? }))?;
            if json_output {
                print_json(&detected);
            } else {
                match detected["language"].as_str() {
                    Some(code) => println!(
                        "{code} ({}){}",
                        detected["name"].as_str().unwrap_or_default(),
                        if detected["reliable"] == true {
                            ""
                        } else {
                            ", not confident"
                        }
                    ),
                    None => println!("undetermined: the default language would be used"),
                }
            }
        }
        Cmd::Speak { request, out, .. } => {
            dispatch(request_command(session, "speak", &request)?)?;
            session.wait_idle();
            if let Some(message) = failure.lock().unwrap().take() {
                return Err(message);
            }
            if let (Some(path), Some(capture)) = (out, capture) {
                let audio = capture.take();
                wav::write(&path, &audio)
                    .map_err(|err| format!("could not write {}: {err}", path.display()))?;
                let summary = json!({ "file": path, "seconds": audio.seconds(), "sample_rate": audio.sample_rate });
                if json_output {
                    print_line(&summary.to_string());
                } else {
                    println!(
                        "wrote {} ({:.2} s at {} Hz)",
                        path.display(),
                        audio.seconds(),
                        audio.sample_rate
                    );
                }
            }
        }
        Cmd::Run { commands } => {
            for raw in commands {
                let command: Value =
                    serde_json::from_str(&raw).map_err(|err| format!("not JSON: {raw} ({err})"))?;
                print_json(&dispatch(command)?);
            }
        }
        Cmd::Serve { .. } => serve(session)?,
        Cmd::Paths => {
            let paths = session.paths();
            if json_output {
                print_json(&json!(paths));
            } else {
                println!("models    {}", paths.models_dir.display());
                println!("settings  {}", paths.settings_file.display());
            }
        }
    }
    Ok(())
}

/// The JSON-lines channel. Each input line is a command object, optionally
/// with an `id` that is echoed in the reply:
///
/// ```text
/// → {"id": 1, "cmd": "speak", "text": "Hello."}
/// ← {"id": 1, "ok": true, "result": {"session": 1}}
/// ← {"event": "speech", "data": {"type": "chunk_playing", "session": 1, "index": 0}}
/// ```
fn serve(session: &Session) -> Result<(), String> {
    for line in std::io::stdin().lock().lines() {
        let line = line.map_err(|err| err.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(mut command) => {
                let id = command
                    .as_object_mut()
                    .and_then(|object| object.remove("id"))
                    .unwrap_or(Value::Null);
                // Checked here as well, for a clearer message than serde's.
                match serde_json::from_value::<EngineCommand>(command)
                    .map_err(|err| format!("invalid command: {err}"))
                    .and_then(|command| session.dispatch(command))
                {
                    Ok(result) => json!({ "id": id, "ok": true, "result": result }),
                    Err(error) => json!({ "id": id, "ok": false, "error": error }),
                }
            }
            Err(err) => json!({ "id": null, "ok": false, "error": format!("not JSON: {err}") }),
        };
        print_line(&reply.to_string());
    }
    // Input ended: let what is being spoken finish before leaving.
    session.wait_idle();
    Ok(())
}
