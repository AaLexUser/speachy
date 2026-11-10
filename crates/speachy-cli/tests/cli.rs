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
