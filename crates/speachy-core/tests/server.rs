//! Models that run in a speech server, end to end: a session with the real
//! engine cache and a virtual sound card, talking to a small HTTP server in
//! this process that answers the way mlx-audio and OpenAI's speech API do.

use serde_json::{json, Value};
use speachy_core::audio::CaptureAudio;
use speachy_core::engine::Audio;
use speachy_core::pipeline::{Outcome, SpeechEvent};
use speachy_core::session::{Event, Paths, Session};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const RATE: u32 = 8000;

/// How the server answers a speech request.
#[derive(Clone)]
enum Answer {
    /// This many seconds of audio, after a delay.
    Audio {
        secs: f32,
        after: Duration,
    },
    Refuse {
        status: u16,
        body: &'static str,
    },
    /// Audio, except for text that holds this word.
    RefuseText(&'static str),
    /// "200 OK", then the connection closes: what mlx-audio does when the
    /// model fails.
    BreakOff,
    NotAudio,
}

struct Server {
    endpoint: String,
    /// Every request: its first line, and its body as JSON.
    requests: Arc<Mutex<Vec<(String, Value)>>>,
    answer: Arc<Mutex<Answer>>,
}

fn respond(stream: &mut TcpStream, status: u16, kind: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
}

fn serve(mut stream: TcpStream, requests: &Mutex<Vec<(String, Value)>>, answer: &Mutex<Answer>) {
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    let (head_end, length) = loop {
        let n = stream.read(&mut buf).unwrap_or(0);
        if n == 0 {
            return;
        }
        data.extend_from_slice(&buf[..n]);
        if let Some(at) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&data[..at]).to_lowercase();
            let length = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .map_or(0, |v| v.trim().parse::<usize>().unwrap());
            break (at + 4, length);
        }
    };
    while data.len() < head_end + length {
        let n = stream.read(&mut buf).unwrap_or(0);
        if n == 0 {
            return;
        }
        data.extend_from_slice(&buf[..n]);
    }
    let first_line = String::from_utf8_lossy(&data[..head_end])
        .lines()
        .next()
        .unwrap_or_default()
        .replace(" HTTP/1.1", "");
    let body: Value = serde_json::from_slice(&data[head_end..]).unwrap_or(Value::Null);
    let input = body["input"].as_str().unwrap_or_default().to_string();
    requests.lock().unwrap().push((first_line.clone(), body));

    if first_line == "GET /v1/models" {
        let body = json!({ "object": "list", "data": [{ "id": "org/tts-large" }, { "id": "org/tts-small" }] });
        respond(
            &mut stream,
            200,
            "application/json",
            body.to_string().as_bytes(),
        );
    } else if first_line == "GET /v1/audio/voices?model=org%2Ftts-large" {
        let body =
            json!({ "data": [{ "id": "anna", "name": "anna" }, { "id": "ivan", "name": "ivan" }] });
        respond(
            &mut stream,
            200,
            "application/json",
            body.to_string().as_bytes(),
        );
    } else if first_line == "POST /v1/audio/speech" {
        let answer = match answer.lock().unwrap().clone() {
            Answer::RefuseText(word) if input.contains(word) => Answer::Refuse {
                status: 500,
                body: r#"{"detail":"out of memory"}"#,
            },
            Answer::RefuseText(_) => Answer::Audio {
                secs: 0.5,
                after: Duration::ZERO,
            },
            other => other,
        };
        match answer {
            Answer::Audio { secs, after } => {
                std::thread::sleep(after);
                let audio = Audio {
                    samples: vec![0.25; (secs * RATE as f32) as usize],
                    sample_rate: RATE,
                };
                respond(
                    &mut stream,
                    200,
                    "audio/wav",
                    &speachy_core::wav::encode(&audio),
                );
            }
            Answer::Refuse { status, body } => {
                respond(&mut stream, status, "application/json", body.as_bytes())
            }
            Answer::RefuseText(_) => unreachable!(),
            Answer::BreakOff => {
                let head = "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nTransfer-Encoding: chunked\r\n\r\n";
                let _ = stream.write_all(head.as_bytes());
            }
            Answer::NotAudio => respond(&mut stream, 200, "text/html", b"<html>hello</html>"),
        }
    } else {
        respond(
            &mut stream,
            404,
            "application/json",
            br#"{"detail":"Not Found"}"#,
        );
    }
}

fn server() -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let answer = Arc::new(Mutex::new(Answer::Audio {
        secs: 0.5,
        after: Duration::ZERO,
    }));
    let (seen, how) = (requests.clone(), answer.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (seen, how) = (seen.clone(), how.clone());
            std::thread::spawn(move || serve(stream, &seen, &how));
        }
    });
    Server {
        endpoint,
        requests,
        answer,
    }
}

impl Server {
    fn speech_requests(&self) -> Vec<Value> {
        let requests = self.requests.lock().unwrap();
        requests
            .iter()
            .filter(|(line, _)| line == "POST /v1/audio/speech")
            .map(|(_, body)| body.clone())
            .collect()
    }

    /// A model definition that points at this server.
    fn model(&self) -> Value {
        json!({
            "id": "big-voice",
            "name": "Big Voice",
            "runtime": {
                "kind": "server",
                "endpoint": self.endpoint,
                "model": "org/tts-large",
                "language_param": "lang_code",
                "language_values": { "en": "a" },
                "options": { "temperature": 0.5, "model": "ignored", "response_format": "mp3" },
            },
            "languages": ["en", "ru"],
            "voices": [{ "id": "anna" }, { "id": "ivan" }],
            "imitates_recordings": true,
        })
    }
}

struct Rig {
    session: Session,
    audio: CaptureAudio,
    events: Receiver<Event>,
    home: tempfile::TempDir,
}

fn rig() -> Rig {
    let home = tempfile::tempdir().unwrap();
    let audio = CaptureAudio::new(f32::INFINITY);
    let (tx, events) = channel();
    let tx = Mutex::new(tx);
    let session = Session::open(
        Paths::under(home.path()),
        Arc::new(audio.clone()),
        move |event| {
            let _ = tx.lock().unwrap().send(event.clone());
        },
    );
    Rig {
        session,
        audio,
        events,
        home,
    }
}

impl Rig {
    fn run(&self, command: Value) -> Result<Value, String> {
        self.session.dispatch_json(command)
    }

    /// Speaks `text` and returns how it ended, with the notices shown.
    fn speak(&self, text: &str) -> (Outcome, Vec<String>) {
        let id = self.run(json!({ "cmd": "speak", "text": text })).unwrap()["session"]
            .as_u64()
            .unwrap();
        let mut notices = Vec::new();
        loop {
            match self
                .events
                .recv_timeout(Duration::from_secs(20))
                .expect("no event")
            {
                Event::Speech(SpeechEvent::Finished {
                    session,
                    outcome,
                    message,
                }) if session == id => {
                    // What went wrong is followed by its notice.
                    if outcome == Outcome::Failed || message.is_some() {
                        while let Ok(event) = self.events.recv_timeout(Duration::from_millis(500)) {
                            if let Event::Notice(text) = event {
                                notices.push(text);
                                break;
                            }
                        }
                    }
                    return (outcome, notices);
                }
                Event::Notice(text) => notices.push(text),
                _ => {}
            }
        }
    }
}

#[test]
fn a_model_in_a_server_speaks_through_the_pipeline() {
    let server = server();
    let rig = rig();
    let added = rig
        .run(json!({ "cmd": "add_model", "model": server.model() }))
        .unwrap();
    // It needs no download, so it can speak at once and becomes the
    // default voice.
    assert_eq!(added["model"]["installed"], true);
    assert_eq!(added["model"]["custom"], true);
    let snapshot = rig.run(json!({ "cmd": "snapshot" })).unwrap();
    assert_eq!(
        snapshot["settings"]["default_voice"],
        json!({ "model_id": "big-voice", "voice_id": "anna" })
    );
    let listed = snapshot["models"].as_array().unwrap().last().unwrap();
    assert_eq!(listed["id"], "big-voice");
    assert_eq!(listed["runtime"]["kind"], "server");

    let text = "The first sentence is short. The second sentence follows it, and it is \
                a good deal longer than the first one was, so that the two of them do \
                not fit into one chunk.";
    let planned = rig.run(json!({ "cmd": "plan", "text": text })).unwrap();
    let chunks: Vec<&str> = planned["chunks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["text"].as_str().unwrap())
        .collect();
    assert!(chunks.len() >= 2, "{chunks:?}");
    let (outcome, notices) = rig.speak(text);
    assert_eq!((outcome, notices), (Outcome::Completed, vec![]));

    // One request for each chunk, in order, in the server's own terms.
    let requests = server.speech_requests();
    let asked: Vec<&str> = requests
        .iter()
        .map(|r| r["input"].as_str().unwrap())
        .collect();
    assert_eq!(asked, chunks);
    for request in &requests {
        assert_eq!(request["model"], "org/tts-large");
        assert_eq!(request["voice"], "anna");
        assert_eq!(request["response_format"], "wav");
        assert_eq!(request["speed"], 1.0);
        // English as this server names it, and the user's own option.
        assert_eq!(request["lang_code"], "a");
        assert_eq!(request["temperature"], 0.5);
        assert!(request.get("ref_audio").is_none());
    }
    // Every answer was played: half a second of audio for each chunk, at
    // the server's sample rate, with pauses between them.
    let played = rig.audio.take();
    assert_eq!(played.sample_rate, RATE);
    let heard = played.samples.iter().filter(|s| s.abs() > 0.2).count();
    let sent = chunks.len() * RATE as usize / 2;
    assert!(
        (sent - 400..=sent).contains(&heard),
        "{heard} of {sent} samples heard"
    );
    assert!(played.samples.len() > heard + 1000);
}

#[test]
fn the_language_and_voice_follow_the_text_and_the_request() {
    let server = server();
    let rig = rig();
    rig.run(json!({ "cmd": "add_model", "model": server.model() }))
        .unwrap();
    let command = json!({
        "cmd": "speak",
        "text": "Привет! Это проверка синтеза речи на русском языке.",
        "voice": { "model_id": "big-voice", "voice_id": "ivan" },
    });
    rig.run(command).unwrap();
    rig.run(json!({ "cmd": "wait" })).unwrap();
    let requests = server.speech_requests();
    assert!(!requests.is_empty());
    // A language the definition gives no name for is sent as its code.
    assert_eq!(requests[0]["lang_code"], "ru");
    assert_eq!(requests[0]["voice"], "ivan");
}

#[test]
fn what_goes_wrong_in_the_server_is_explained() {
    let server = server();
    let rig = rig();
    rig.run(json!({ "cmd": "add_model", "model": server.model() }))
        .unwrap();
    let cases = [
        (
            Answer::Refuse {
                status: 400,
                body: r#"{"detail":"Model org/tts-large is not a speech model"}"#,
            },
            "answered 400: Model org/tts-large is not a speech model",
        ),
        (Answer::BreakOff, "its own log will say why"),
        (Answer::NotAudio, "did not answer with WAV audio"),
    ];
    for (answer, expected) in cases {
        *server.answer.lock().unwrap() = answer;
        let (outcome, notices) = rig.speak("Hello there.");
        assert_eq!(outcome, Outcome::Failed);
        assert!(
            notices.iter().any(|n| n.contains(expected)),
            "{notices:?} does not mention {expected:?}"
        );
    }
    // The server recovers: so does speech.
    *server.answer.lock().unwrap() = Answer::Audio {
        secs: 0.2,
        after: Duration::ZERO,
    };
    assert_eq!(rig.speak("Hello again.").0, Outcome::Completed);
}

#[test]
fn a_chunk_the_server_refuses_is_not_dropped_in_silence() {
    let server = server();
    *server.answer.lock().unwrap() = Answer::RefuseText("second");
    let rig = rig();
    rig.run(json!({ "cmd": "add_model", "model": server.model() }))
        .unwrap();
    let text = "The first sentence is short. The second sentence follows it, and it is \
                a good deal longer than the first one was, so that the two of them do \
                not fit into one chunk.";
    let (outcome, notices) = rig.speak(text);
    // The rest was read, and the listener is told that something is missing.
    assert_eq!(outcome, Outcome::Completed);
    assert_eq!(
        notices,
        ["Part of the text was left out: the speech server answered 500: out of memory"]
    );
    assert!(!rig.audio.take().samples.is_empty());
}

#[test]
fn a_server_that_is_not_running_is_named() {
    // A port nothing listens on.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let rig = rig();
    let mut model = server().model();
    model["runtime"]["endpoint"] = json!(format!("http://127.0.0.1:{port}"));
    rig.run(json!({ "cmd": "add_model", "model": model }))
        .unwrap();
    let (outcome, notices) = rig.speak("Hello there.");
    assert_eq!(outcome, Outcome::Failed);
    let expected = format!("the speech server at http://127.0.0.1:{port} is not answering");
    assert!(notices.iter().any(|n| n.contains(&expected)), "{notices:?}");
    assert!(
        notices.iter().any(|n| n.contains("Is it running?")),
        "{notices:?}"
    );
    let asked =
        rig.run(json!({ "cmd": "server_offer", "endpoint": format!("http://127.0.0.1:{port}") }));
    assert!(asked.unwrap_err().contains("is not answering"));
}

#[test]
fn stopping_does_not_wait_for_a_slow_server() {
    let server = server();
    *server.answer.lock().unwrap() = Answer::Audio {
        secs: 0.5,
        after: Duration::from_secs(30),
    };
    let rig = rig();
    rig.run(json!({ "cmd": "add_model", "model": server.model() }))
        .unwrap();
    rig.run(json!({ "cmd": "speak", "text": "Hello there." }))
        .unwrap();
    // The request is on its way.
    let deadline = Instant::now() + Duration::from_secs(10);
    while server.speech_requests().is_empty() {
        assert!(Instant::now() < deadline, "the server was never asked");
        std::thread::sleep(Duration::from_millis(10));
    }
    let started = Instant::now();
    assert_eq!(rig.run(json!({ "cmd": "stop" })).unwrap()["stopped"], true);
    rig.run(json!({ "cmd": "wait" })).unwrap();
    // And nothing is left that would keep the process from leaving.
    assert!(rig.session.shutdown(Duration::from_secs(5)));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn a_voice_of_ones_own_is_sent_as_a_recording() {
    let server = server();
    let rig = rig();
    rig.run(json!({ "cmd": "add_model", "model": server.model() }))
        .unwrap();
    // A recording in a form Speachy has to convert: 32-bit float, stereo.
    let mut wav = b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0\x03\0\x02\0".to_vec();
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&[0, 0, 0, 0, 8, 0, 32, 0]);
    wav.extend_from_slice(b"data");
    let frames = 2 * RATE as usize;
    wav.extend_from_slice(&((frames * 8) as u32).to_le_bytes());
    for _ in 0..frames * 2 {
        wav.extend_from_slice(&0.5f32.to_le_bytes());
    }
    let recording = rig.home.path().join("me.wav");
    std::fs::write(&recording, wav).unwrap();

    let added = rig
        .run(json!({
            "cmd": "add_voice", "model": "big-voice", "name": "My Voice!",
            "recording": recording, "text": "This is what I say in it.",
        }))
        .unwrap();
    assert_eq!(added["voice"]["id"], "my-voice");
    assert_eq!(added["voice"]["name"], "My Voice!");
    assert_eq!(added["voice"]["custom"], true);
    // Speachy keeps its own copy, as 16-bit mono.
    let copy = std::path::PathBuf::from(added["voice"]["reference"].as_str().unwrap());
    assert!(copy.starts_with(rig.home.path().join("voices").join("big-voice")));
    let kept = speachy_core::wav::decode(&std::fs::read(&copy).unwrap()).unwrap();
    assert_eq!((kept.samples.len(), kept.sample_rate), (frames, RATE));
    assert!((kept.samples[0] - 0.5).abs() < 1e-3);

    let command = json!({
        "cmd": "speak", "text": "Hello there.",
        "voice": { "model_id": "big-voice", "voice_id": "my-voice" },
    });
    rig.run(command).unwrap();
    rig.run(json!({ "cmd": "wait" })).unwrap();
    let request = server.speech_requests().pop().unwrap();
    assert_eq!(request["ref_audio"], copy.to_string_lossy().as_ref());
    assert_eq!(request["ref_text"], "This is what I say in it.");
    assert!(request.get("voice").is_none());

    // Made the default, then removed: the default falls back, the copy goes.
    let patch = json!({ "default_voice": { "model_id": "big-voice", "voice_id": "my-voice" } });
    rig.run(json!({ "cmd": "update_settings", "patch": patch }))
        .unwrap();
    rig.run(json!({ "cmd": "remove_voice", "model": "big-voice", "voice": "my-voice" }))
        .unwrap();
    assert!(!copy.exists());
    let snapshot = rig.run(json!({ "cmd": "snapshot" })).unwrap();
    assert_eq!(snapshot["settings"]["default_voice"]["voice_id"], "anna");
    // The model's own voices cannot be removed, nor recordings that are not audio.
    assert!(rig
        .run(json!({ "cmd": "remove_voice", "model": "big-voice", "voice": "anna" }))
        .is_err());
    let text = rig.home.path().join("notes.txt");
    std::fs::write(&text, "not audio").unwrap();
    let refused = rig
        .run(json!({ "cmd": "add_voice", "model": "big-voice", "name": "x", "recording": text }));
    assert!(refused.unwrap_err().contains("not a WAV recording"));
}

#[test]
fn a_server_says_what_it_offers() {
    let server = server();
    let rig = rig();
    let offer = rig
        .run(json!({ "cmd": "server_offer", "endpoint": format!("{}/", server.endpoint), "model": "org/tts-large" }))
        .unwrap();
    assert_eq!(
        offer,
        json!({ "models": ["org/tts-large", "org/tts-small"], "voices": ["anna", "ivan"] })
    );
    // A model the server lists no voices for: none, not an error.
    let offer = rig
        .run(
            json!({ "cmd": "server_offer", "endpoint": server.endpoint, "model": "org/tts-small" }),
        )
        .unwrap();
    assert_eq!(offer["voices"], json!([]));
}

#[test]
fn models_of_ones_own_are_kept_changed_and_removed() {
    let server = server();
    let rig = rig();
    rig.run(json!({ "cmd": "add_model", "model": server.model() }))
        .unwrap();
    // Nothing to download, and nothing sensible to say about a definition
    // that is not one.
    let refused = rig.run(json!({ "cmd": "install_model", "id": "big-voice", "wait": true }));
    assert!(refused.unwrap_err().contains("nothing to download"));
    let refused = rig.run(json!({ "cmd": "add_model", "model": { "id": "x" } }));
    assert!(refused.unwrap_err().contains("not a model definition"));

    // The same id again replaces the definition; a voice that is gone
    // leaves the settings.
    let patch = json!({ "default_voice": { "model_id": "big-voice", "voice_id": "ivan" } });
    rig.run(json!({ "cmd": "update_settings", "patch": patch }))
        .unwrap();
    let mut changed = server.model();
    changed["voices"] = json!([{ "id": "anna" }, { "id": "olga" }]);
    rig.run(json!({ "cmd": "add_model", "model": changed }))
        .unwrap();
    let snapshot = rig.run(json!({ "cmd": "snapshot" })).unwrap();
    assert_eq!(snapshot["settings"]["default_voice"]["voice_id"], "anna");

    // A new session on the same files still has it.
    let again = Session::open(
        Paths::under(rig.home.path()),
        Arc::new(CaptureAudio::discarding(f32::INFINITY)),
        |_| {},
    );
    let model = again.catalog().find("big-voice").unwrap();
    assert_eq!(model.voices[1].id, "olga");

    rig.run(json!({ "cmd": "remove_model", "id": "big-voice" }))
        .unwrap();
    let snapshot = rig.run(json!({ "cmd": "snapshot" })).unwrap();
    assert!(snapshot["models"]
        .as_array()
        .unwrap()
        .iter()
        .all(|m| m["id"] != "big-voice"));
    assert_eq!(snapshot["settings"]["default_voice"], Value::Null);
    assert!(rig
        .run(json!({ "cmd": "remove_model", "id": "big-voice" }))
        .is_err());
}
