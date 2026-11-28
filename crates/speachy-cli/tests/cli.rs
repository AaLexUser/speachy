//! Runs the built `speachy-cli` binary the way a script or an agent would.
//! No model is installed in these tests, so nothing is synthesized, except
//! by the tests that `SPEACHY_TEST_MODELS` turns on: set it to a models
//! directory with `supertonic-3` installed.

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
fn speed_commands_answer_with_the_new_speed() {
    let home = tempfile::tempdir().unwrap();
    let faster = cli(
        home.path(),
        &["run", r#"{"cmd":"change_speed","by":0.1}"#],
        None,
    );
    assert_eq!(json(&faster), serde_json::json!({ "speed": 1.1 }));

    let fresh = tempfile::tempdir().unwrap();
    let reset = cli(fresh.path(), &["run", r#"{"cmd":"reset_speed"}"#], None);
    assert_eq!(json(&reset), serde_json::json!({ "speed": 1.0 }));
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

/// A scratch home that uses the models in `SPEACHY_TEST_MODELS`, if set.
#[cfg(unix)]
fn home_with_models() -> Option<tempfile::TempDir> {
    let models = std::env::var_os("SPEACHY_TEST_MODELS")?;
    let home = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(models, home.path().join("models")).unwrap();
    Some(home)
}

#[cfg(unix)]
#[test]
fn speaking_reports_every_word_as_it_is_read() {
    let Some(home) = home_with_models() else {
        eprintln!("SPEACHY_TEST_MODELS is not set: nothing to synthesize with");
        return;
    };
    let text = "Выручка выросла на 15% и достигла 3,5 млрд рублей.";
    let args = [
        "speak",
        "--json",
        "--no-audio",
        "--voice",
        "supertonic-3/F1",
        "--lang",
        "ru",
        text,
    ];
    let output = cli(home.path(), &args, None);
    assert!(output.status.success(), "{}", stdout(&output));
    let lines: Vec<Value> = stdout(&output)
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let words: Vec<String> = lines
        .iter()
        .filter(|l| l["data"]["type"] == "chunk_ready")
        .flat_map(|l| l["data"]["words"].as_array().unwrap().clone())
        .map(|w| w["text"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        words,
        [
            "Выручка",
            "выросла",
            "на",
            "15%",
            "и",
            "достигла",
            "3,5 млрд",
            "рублей."
        ]
    );
    let played: Vec<(u64, u64)> = lines
        .iter()
        .filter(|l| l["data"]["type"] == "word_playing")
        .map(|l| {
            (
                l["data"]["index"].as_u64().unwrap(),
                l["data"]["word"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(played, (0..8).map(|w| (0, w)).collect::<Vec<_>>());
}

#[cfg(unix)]
#[test]
fn plan_shows_what_the_voice_is_given_under_its_chunk() {
    let Some(home) = home_with_models() else {
        eprintln!("SPEACHY_TEST_MODELS is not set: no model to plan with");
        return;
    };
    let text = "Вода стоит 15 рублей.";
    let output = cli(home.path(), &["plan", "--lang", "ru", text], None);
    assert!(output.status.success(), "{}", stdout(&output));
    let lines: Vec<String> = stdout(&output).lines().map(str::to_string).collect();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[0].ends_with(text), "{}", lines[0]);
    assert!(
        lines[1].ends_with("Вода\u{301} стоит пятна\u{301}дцать рубле\u{301}й."),
        "{}",
        lines[1]
    );

    let output = cli(home.path(), &["plan", "--json", "--lang", "ru", text], None);
    let chunks = json(&output);
    assert_eq!(chunks[0]["text"], text);
    assert_eq!(
        chunks[0]["spoken"],
        "Вода\u{301} стоит пятна\u{301}дцать рубле\u{301}й."
    );

    // Nothing is printed under a chunk the voice reads as written.
    let output = cli(home.path(), &["plan", "--lang", "en", "Hello there."], None);
    assert!(output.status.success(), "{}", stdout(&output));
    assert_eq!(stdout(&output).lines().count(), 1);
}
