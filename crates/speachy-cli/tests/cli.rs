//! Runs the built `speachy-cli` binary the way a script or an agent would.
//! No model is installed in these tests, so nothing is synthesized.

use serde_json::Value;
use std::io::Write;
use std::process::{Command, Output, Stdio};

fn cli(home: &std::path::Path, args: &[&str], stdin: Option<&str>) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_speachy-cli"))
        .arg("--home")
        .arg(home)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if let Some(input) = stdin {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    }
    drop(child.stdin.take());
    child.wait_with_output().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|err| panic!("not JSON ({err}): {}", stdout(output)))
}

#[test]
fn models_and_paths_are_listed() {
    let home = tempfile::tempdir().unwrap();
    let models = cli(home.path(), &["models", "--json"], None);
    assert!(models.status.success());
    let models = json(&models);
    assert!(models
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["id"] == "supertonic-3" && m["installed"] == false));

    let table = stdout(&cli(home.path(), &["models"], None));
    assert!(
        table.contains("supertonic-3") && table.contains("Kokoro"),
        "{table}"
    );

    let paths = json(&cli(home.path(), &["paths", "--json"], None));
    assert!(paths["models_dir"]
        .as_str()
        .unwrap()
        .starts_with(home.path().to_str().unwrap()));
}

#[test]
fn settings_can_be_read_and_changed() {
    let home = tempfile::tempdir().unwrap();
    assert_eq!(json(&cli(home.path(), &["settings"], None))["speed"], 1.0);
    let changed = json(&cli(
        home.path(),
        &[
            "settings",
            "speed=1.4",
            "language=de",
            "shortcuts.stop=F9",
            "show_overlay=false",
        ],
        None,
    ));
    assert_eq!(
        changed["speed"].as_f64().map(|v| (v * 10.0).round()),
        Some(14.0)
    );
    assert_eq!(changed["language"], "de");
    assert_eq!(changed["shortcuts"]["stop"], "F9");
    assert_eq!(changed["show_overlay"], false);
    // Saved: a later invocation sees it.
    assert_eq!(
        json(&cli(home.path(), &["settings"], None))["language"],
        "de"
    );

    let bad = cli(home.path(), &["settings", "nope=1"], None);
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("unknown setting"));
}

#[test]
fn detect_reads_arguments_or_standard_input() {
    let home = tempfile::tempdir().unwrap();
    let from_args = stdout(&cli(
        home.path(),
        &[
            "detect",
            "Dies",
            "ist",
            "ein",
            "Test",
            "der",
            "Sprachsynthese",
            "und",
            "das",
            "Wetter",
            "ist",
            "schön.",
        ],
        None,
    ));
    assert!(from_args.starts_with("de (German)"), "{from_args}");
    let from_stdin = json(&cli(
        home.path(),
        &["detect", "--json"],
        Some("Привет! Это проверка синтеза речи."),
    ));
    assert_eq!(from_stdin["language"], "ru");
}

#[test]
fn speaking_without_a_model_fails_with_a_message() {
    let home = tempfile::tempdir().unwrap();
    for args in [
        &["speak", "--no-audio", "Hello."][..],
        &["plan", "Hello."],
        &["voices"],
    ] {
        let output = cli(home.path(), args, None);
        assert!(!output.status.success(), "{args:?}");
        let err = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(
            err.contains("no voice model") || err.contains("no model is installed"),
            "{args:?}: {err}"
        );
    }
}

#[test]
fn raw_commands_run_like_the_app_sends_them() {
    let home = tempfile::tempdir().unwrap();
    let output = cli(
        home.path(),
        &[
            "run",
            r#"{"cmd":"detect_language","text":"你好，今天天气很好。"}"#,
        ],
        None,
    );
    assert_eq!(json(&output)["language"], "zh");
    assert!(!cli(home.path(), &["run", r#"{"cmd":"fly"}"#], None)
        .status
        .success());
}

#[test]
fn serve_answers_each_line_and_echoes_ids() {
    let home = tempfile::tempdir().unwrap();
    let input = concat!(
        r#"{"id": 1, "cmd": "update_settings", "patch": {"speed": 2}}"#,
        "\n",
        r#"{"id": "two", "cmd": "speak", "text": "Hello."}"#,
        "\n",
        "this is not json\n",
        "\n",
        r#"{"id": 4, "cmd": "stop"}"#,
        "\n",
    );
    let output = cli(home.path(), &["serve", "--no-audio"], Some(input));
    assert!(output.status.success());
    let lines: Vec<Value> = stdout(&output)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let reply = |id: Value| {
        lines
            .iter()
            .find(|l| l["id"] == id && l.get("ok").is_some())
            .unwrap()
    };
    assert_eq!(reply(1.into())["result"]["settings"]["speed"], 2.0);
    assert_eq!(reply("two".into())["ok"], false);
    assert!(reply("two".into())["error"]
        .as_str()
        .unwrap()
        .contains("no voice model"));
    assert_eq!(reply(4.into())["result"]["stopped"], false);
    assert!(lines
        .iter()
        .any(|l| l["ok"] == false && l["error"].as_str().unwrap().contains("not JSON")));
    // The settings change was announced as an event on the same stream.
    assert!(lines
        .iter()
        .any(|l| l["event"] == "settings" && l["data"]["speed"] == 2.0));
}

/// A speech server that lists one model with two voices and answers every
/// speech request with a second of sound. Returns its address.
fn speech_server() -> String {
    use std::io::Read;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut data = Vec::new();
            let mut buf = [0u8; 4096];
            // Headers, then as much body as they announce.
            let wanted = loop {
                let n = stream.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    break 0;
                }
                data.extend_from_slice(&buf[..n]);
                if let Some(at) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&data[..at]).to_lowercase();
                    let length = head
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .map_or(0, |v| v.trim().parse::<usize>().unwrap());
                    break at + 4 + length;
                }
            };
            while data.len() < wanted {
                let n = stream.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    break;
                }
                data.extend_from_slice(&buf[..n]);
            }
            let request = String::from_utf8_lossy(&data).into_owned();
            let (kind, body) = if request.starts_with("GET /v1/models") {
                (
                    "application/json",
                    br#"{"data":[{"id":"org/Big-Voice_v2"}]}"#.to_vec(),
                )
            } else if request.starts_with("GET /v1/audio/voices") {
                (
                    "application/json",
                    br#"{"data":[{"id":"anna"},{"id":"ivan"}]}"#.to_vec(),
                )
            } else {
                let audio = speachy_core::engine::Audio {
                    samples: vec![0.25; 8000],
                    sample_rate: 8000,
                };
                ("audio/wav", speachy_core::wav::encode(&audio))
            };
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body);
        }
    });
    address
}

#[test]
fn a_model_in_a_speech_server_is_added_spoken_with_and_removed() {
    let home = tempfile::tempdir().unwrap();
    let server = speech_server();

    let offer = stdout(&cli(
        home.path(),
        &["server", &server, "org/Big-Voice_v2"],
        None,
    ));
    assert!(
        offer.contains("org/Big-Voice_v2") && offer.contains("anna, ivan"),
        "{offer}"
    );

    // The voices come from the server, the id from the model's name.
    let added = cli(
        home.path(),
        &[
            "add-model",
            "--server",
            &server,
            "--model",
            "org/Big-Voice_v2",
            "--languages",
            "en,ru",
        ],
        None,
    );
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    assert!(
        stdout(&added).contains("added big-voice-v2 (2 voice(s))"),
        "{}",
        stdout(&added)
    );
    let table = stdout(&cli(home.path(), &["models"], None));
    assert!(
        table
            .lines()
            .any(|l| l.starts_with("big-voice-v2") && l.contains("server")),
        "{table}"
    );
    let voices = stdout(&cli(home.path(), &["voices"], None));
    assert!(
        voices.contains("big-voice-v2/anna") && voices.contains("big-voice-v2/ivan"),
        "{voices}"
    );

    // It is the only model that can speak, so it is the one that does.
    let wav = home.path().join("out.wav");
    let spoken = cli(
        home.path(),
        &[
            "speak",
            "--out",
            wav.to_str().unwrap(),
            "Hello from the server.",
        ],
        None,
    );
    assert!(
        spoken.status.success(),
        "{}",
        String::from_utf8_lossy(&spoken.stderr)
    );
    let audio = speachy_core::wav::decode(&std::fs::read(&wav).unwrap()).unwrap();
    assert!(audio.seconds() > 0.9, "{} s", audio.seconds());

    assert!(cli(home.path(), &["remove", "big-voice-v2"], None)
        .status
        .success());
    let table = stdout(&cli(home.path(), &["models"], None));
    assert!(!table.contains("big-voice-v2"), "{table}");

    // Without a server at the address, the reason is given.
    let nobody = cli(home.path(), &["server", "http://127.0.0.1:9"], None);
    assert!(!nobody.status.success());
    assert!(String::from_utf8_lossy(&nobody.stderr).contains("is not answering"));
}
