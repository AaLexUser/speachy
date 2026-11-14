# AGENTS.md: working on Speachy

Speachy reads selected text aloud with local TTS models. This file is for
anyone, human or agent, changing the code. The short version: **the engine
is separate from the interface, and everything the app does is an engine
command you can run from a terminal.** You do not need the window, a
screen or a sound card to build, debug or verify engine work.

## How changes land

- `main` is the branch releases are cut from. Do not push to it.
- Do each piece of work on its own branch and open a pull request against
  `main`. CI (the **Build** workflow) runs on every pull request; it must
  pass.
- Merge only after the pull request has been approved, and merge by
  **squash**, so `main` gets one commit per change.
- Releases: set the version in `Cargo.toml` through a pull request, then run
  the **Release** workflow on `main` (it leaves a draft) and **Publish
  release** once the draft has been checked. See the README.

## Layers

| Crate | What it is | May depend on |
| --- | --- | --- |
| `crates/speachy-core` | The engine: models, text, language, planning, synthesis, the speech pipeline, settings. Front door: `session::Session`. | no UI toolkit, no front end |
| `crates/speachy-cli` | Headless front end: subcommands, raw JSON commands, the `serve` channel. | `speachy-core` |
| `src-tauri` + `ui/` | The desktop shell: windows, menu-bar icon, global shortcuts, reading the selection. | `speachy-core`, Tauri |

`scripts/check-layers.sh` (run in CI) fails if the engine or the CLI gains a
dependency on a UI crate.

What belongs where:

- **Engine** (`speachy-core`): anything that would still make sense with no
  window. If the CLI could use it, it goes here.
- **Shell** (`src-tauri`): only what needs the desktop — windows, tray,
  shortcuts, the Accessibility API and clipboard (`selection.rs`), the login
  item. The shell holds a `Session` and forwards to it; it has no speech
  logic of its own.
- **UI** (`ui/`): plain HTML, CSS and JavaScript, no build step. It renders a
  snapshot and sends engine commands through one Tauri command, `engine`.

## Everything is a command

`Session::dispatch` takes a `Command` (`crates/speachy-core/src/session.rs`)
and returns JSON. The app's window, the CLI and `serve` all send the same
commands, as `{"cmd": "<name>", …}`:

| Command | Arguments | Result |
| --- | --- | --- |
| `snapshot` | | models with install state, settings, speech state, paths |
| `install_model` | `id`, `wait` (default false) | `{}`; progress as `download` events |
| `cancel_install` | `id` | `{"cancelled": bool}` |
| `remove_model` | `id` | `{}` |
| `update_settings` | `patch` (only the fields to change) | `{"settings": …}` |
| `speak` | `text`, `voice` (`{model_id, voice_id}`), `lang` | `{"session": n}` |
| `plan` | same as `speak` | `{"chunks": […]}`; nothing is synthesized |
| `detect_language` | `text` | `{"language", "name", "reliable"}` |
| `stop` | | `{"stopped": bool}` |
| `pause`, `resume`, `toggle_pause` | | `{}` |
| `wait` | | `{}` once nothing is being spoken |
| `preload` | | `{"model": id}` once the default voice is loaded |

Events (`session::Event`) are `{"event": "<kind>", "data": …}` with kinds
`speech` (state changes, chunk ready / playing, finished), `download`,
`settings` and `notice`.

**Adding behaviour:** add a method to `Session`, a `Command` variant that
calls it, and a test in `crates/speachy-core/tests/session.rs`. Expose it in
the CLI if a person would want it. Only then wire up the UI. Behaviour that
exists only in the shell cannot be tested headlessly, so keep that to what
truly needs the desktop.

## Driving the engine from a terminal

```sh
cargo build -p speachy-cli            # target/debug/speachy-cli
speachy-cli --help                    # every subcommand explains itself

speachy-cli models                    # what can be installed, what is
speachy-cli install supertonic-3      # download a model (about 130 MB)
speachy-cli voices
speachy-cli plan "Hello. Привет, как дела?"      # chunks, language and voice per chunk
speachy-cli detect "Dies ist ein Test."
speachy-cli speak --no-audio "Timing only."       # synthesize, discard the audio
speachy-cli speak --out out.wav "Keep the audio." # write a WAV instead of playing
speachy-cli settings speed=1.2 shortcuts.stop=F9
speachy-cli run '{"cmd":"snapshot"}'  # any engine command, as the app sends it
```

- Add `--json` for machine-readable output. With `speak`, events are then
  printed as JSON lines.
- By default the CLI uses **the app's own models and settings** (see
  `speachy-cli paths`), which is what you want when reproducing something a
  user saw. Use `--home DIR` or `SPEACHY_HOME=DIR` for a scratch directory.
- No sound card is needed: `--no-audio` and `--out` use a virtual one. Add
  `--realtime` to make it consume audio at playback pace, which shows the
  timing a listener would get (gaps appear as `state: Preparing`).

### A long-lived session: `serve`

`speachy-cli serve` reads one JSON command per line on standard input and
writes replies and events as JSON lines on standard output. The model stays
loaded between commands, and you can interrupt speech, which a one-shot
command cannot show:

```text
→ {"id": 1, "cmd": "speak", "text": "A long text …"}
← {"event":"speech","data":{"type":"started","session":1,"chunks":[…]}}
← {"id":1,"ok":true,"result":{"session":1}}
← {"event":"speech","data":{"type":"chunk_playing","session":1,"index":0}}
→ {"id": 2, "cmd": "stop"}
← {"id":2,"ok":true,"result":{"stopped":true}}
← {"event":"speech","data":{"type":"finished","session":1,"outcome":"stopped","message":null}}
```

`id` is optional and echoed back. Errors come back as
`{"id": …, "ok": false, "error": "…"}`. Use `serve --no-audio` where there
is no sound card; it plays at real-time pace. Commands are handled one at a
time, so `wait` blocks the channel until speech ends.

## Checks

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace          # or: -p speachy-core -p speachy-cli without the GUI libraries
scripts/check-layers.sh
```

- `crates/speachy-core/tests/pipeline.rs` tests the pipeline with a fake
  synthesizer and a virtual sound card: ordering, gapless output, the
  look-ahead bound, pause, stop, replacement, shutdown.
- `crates/speachy-core/tests/session.rs` tests the command surface.
- `crates/speachy-cli/tests/cli.rs` runs the built binary.
- None of these download models. To check real synthesis, install a model
  into a scratch `--home` and use `speak --no-audio` or `--out`.

Timing-sensitive tests must hold on a slow, loaded machine: derive waits
from events, not from sleeps, and leave wide margins.

## Things to know

- **Building.** The first build downloads a prebuilt sherpa-onnx library
  from the sherpa-onnx GitHub releases. Where that download is not possible,
  fetch the archive named in the error by other means and point
  `SHERPA_ONNX_ARCHIVE_DIR` at its directory. Building only the engine and
  CLI (`-p speachy-core -p speachy-cli`) needs no GUI libraries; on Linux it
  needs `libasound2-dev` and `libssl-dev`.
- **Shutting down.** A front end must call `Session::shutdown` before the
  process exits. Exiting while a thread is still synthesizing aborts inside
  ONNX Runtime.
- **Event listeners** run on engine threads that may hold engine locks. A
  listener must not call back into the session on the same thread; hand the
  work to another thread (see `on_engine_event` in `src-tauri/src/lib.rs`).
- **English for Kokoro** is phonemized in `english.rs` (misaki-rs), not by
  sherpa-onnx's lexicon lookup, which drops unknown words.
- **Never panic on input.** Text, settings files, command JSON and model
  archives are untrusted: return an error message.

## What does need the desktop

Reading the selection, global shortcuts, the menu-bar icon, the overlay pill
and the Accessibility permission flow live in `src-tauri` and can only be
exercised in the running app; the macOS parts only on a Mac. Keep these thin.

Three scripts exercise the built app on a Mac, and run in CI on every push:
`scripts/smoke-app.sh` (launch), `scripts/e2e-macos.sh` (another app in full
screen, the shortcut with and without a selection, screenshots of the pill)
and `scripts/e2e-update-macos.sh` (self-update from a local server, and
refusal of a tampered update). Their screenshots and logs are uploaded as
workflow artifacts; look at them before claiming a desktop change works.

**Updates** (`src-tauri/src/updates.rs`) are the shell's: the app replaces
its own bundle. The feed is `latest.json` on the newest GitHub release;
archives are verified against `plugins.updater.pubkey`. Never commit the
private key, and never weaken verification to make a test pass —
`SPEACHY_UPDATE_ENDPOINT` redirects the feed for testing, but the signature
is still checked.

For interface work without a Mac, the app runs on Linux: `cargo run -p
speachy` (under `xvfb-run` where there is no display). The window scripts in
`ui/` can also be opened in a browser with `window.__TAURI__` stubbed, which
is how the screenshots in `docs/` were made.
