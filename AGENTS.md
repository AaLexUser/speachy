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
| `change_speed` | `by` (for example `0.1` or `-0.1`) | `{"speed": 1.1}`; also while speaking, heard at once |
| `reset_speed` | | `{"speed": 1.0}`; at 1.0, back to the speed before the last reset |
| `speak` | `text`, `voice` (`{model_id, voice_id}`), `lang` | `{"session": n}` |
| `plan` | same as `speak` | `{"chunks": […]}`, each with the text and, as `spoken`, the words the voice is given when they differ from it; nothing is synthesized |
| `detect_language` | `text` | `{"language", "name", "reliable"}` |
| `stop` | | `{"stopped": bool}` |
| `pause`, `resume`, `toggle_pause` | | `{}` |
| `wait` | | `{}` once nothing is being spoken |
| `preload` | | `{"model": id}` once the default voice is loaded |

Events (`session::Event`) are `{"event": "<kind>", "data": …}` with kinds
`speech` (state changes, chunk ready / playing, word playing, finished),
`download`, `settings` and `notice`. A `chunk_ready` event carries the
chunk's loudness over time (`levels`, one value from 0 to 100 for every 40 ms
of audio), which is what the pill's bars draw, and its `words` for captions
(`text` as written, `start_secs` at natural pace, `line`; see
`captions.rs`). `word_playing` (`index`, `word`) is sent by the audio thread
when playback reaches a word, so it follows speed, pause and resume. A
`started` event carries the planned chunks, `spoken` included. `spoken` is
made once, in `plan.rs`, and `plan`, `started` and the synthesizer are all
given that one value, so `plan` shows what the voice will be given. (Kokoro
makes its English phonemes from the text itself; `spoken` is then the words
behind them.)

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
speachy-cli plan "Hello. Привет, как дела?"      # chunks, language and voice per chunk, and what the voice is given where that differs
speachy-cli detect "Dies ist ein Test."
speachy-cli speak --no-audio "Timing only."       # synthesize, discard the audio
speachy-cli speak --out out.wav "Keep the audio." # write a WAV instead of playing
speachy-cli speak --json --no-audio "3,5 млрд рублей." | grep word_playing   # the words, as they are read
speachy-cli settings speed=1.2 shortcuts.stop=F9
speachy-cli audio                     # the sound output device and whether it can be used
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
python3 scripts/stress-eval.py marks   # Russian stress marks against the gold: a second, no models
                                       # (`heard` and `compare` of that script are run by hand, see below)
```

- `crates/speachy-core/tests/pipeline.rs` tests the pipeline with a fake
  synthesizer and a virtual sound card: ordering, gapless output, the
  look-ahead bound, pause, stop, replacement, shutdown.
- `crates/speachy-core/tests/session.rs` tests the command surface.
- `crates/speachy-core/tests/stress.rs` holds `tests/fixtures/stress-spoken.txt`
  to what the engine gives the voice for the sentences of
  `tests/fixtures/stress-gold.txt`.
- `crates/speachy-core/tests/captions.rs` holds word timing to its measured
  accuracy on recordings in `tests/fixtures/captions.json`
  (`scripts/make-caption-fixtures.py` makes them; it needs the models).
- `crates/speachy-cli/tests/cli.rs` runs the built binary. Set
  `SPEACHY_TEST_MODELS` to a models directory with `supertonic-3` to also
  run its tests that synthesize; one of them checks that the `chunk_seconds` of
  `speak --out --json`, which `heard` cuts the audio by, add up to the file.
- None of these download models. To check real synthesis, install a model
  into a scratch `--home` and use `speak --no-audio` or `--out`.

Timing-sensitive tests must hold on a slow, loaded machine: derive waits
from events, not from sleeps, and leave wide margins.

## Russian stress: the eval

`crates/speachy-core/tests/fixtures/stress-gold.txt` holds Russian sentences
with the stress of every word marked; its header gives the rules and says who
placed the marks and that no native speaker has checked them.
`scripts/stress-eval.py` measures Speachy against it, in two halves.

`marks` is exact: text against text, no model, standard library only, and it
runs in CI. For every word of two or more vowels it says whether Speachy marked
the gold vowel, marked another vowel, left the word to the voice, read it as
another language, or rewrote it. A word marked on another vowel is an error in
the stress table or in the gold; a person settles each and fixes it where it is.

`heard` is estimated, and is run by hand: it is not in CI. It has the built CLI
read the gold as one document (`speak --out`, language detection on, one
process per reading, two readings), and a Russian phoneme recognizer says which
vowel of each word carried the stress. The report is how often a word was heard
off its gold vowel, per class of `marks`, with a 95 % range from resampling
sentences. It stops if the binary does not give the voice what
`stress-spoken.txt` says, so build the CLI from the checkout first. It refuses a
speed other than 1.0. Runs are saved in `target/stress-eval/NAME.json`, with the
audio beside them:

```sh
cargo build -p speachy-cli
uv run scripts/stress-eval.py heard mine --home DIR    # about 17 minutes; DIR has supertonic-3
uv run scripts/stress-eval.py compare main mine        # after the same on a build of main
```

`heard` downloads the recognizer, `omogr/wav2vec2-lv-60-ru-ipa` at the revision
pinned in the script (1.26 GB, once, to the Hugging Face cache), and installs
the Python packages pinned at the top of the script (`uv run` does; torch alone
is a few hundred MB). Its licence is CC BY-NC-SA 4.0, so nothing of it may enter
this repository, the app or a release: it is a development tool, fetched when
`heard` runs. Its weights are a pickle and are read with
`torch.load(weights_only=True)`; keep it that way.

What the numbers mean. A word "heard off the gold vowel" is not a word stressed
wrongly: the same recognizer hears people off the gold vowel in 1.9 % of words,
and it misses about one wrong stress in five on other voices. The share is a
rate over many words, not a verdict on one, and two readings of one text differ
on a few words in a hundred. Judge a change with `compare`, not by eye: it pairs
two runs word by word, refuses runs with another voice, model, speed or
recognizer revision, and prints three lines. "Words Speachy marks differently"
is where a change can show; "words Speachy marks the same" is the control, and
should say "within noise". It is a control for noise, not a proof: those words
share chunks with words whose marks did change, so once in a while it says
"real" (it did, by 4.7 points, between a build and one that marks nothing). "Real" means the 95 % range of the difference excludes
zero. A change of under about 1.5 points overall, or about 9 points in a group
of 100 words, cannot be seen this way; `marks` shows it exactly.

The script reads `tests/fixtures/stress-spoken.txt`, which is what the engine
gives the voice for the gold read as one document. `tests/stress.rs` fails
until that file equals the code, so a change cannot land without it. After a
change to `stress.rs`, `russian.rs`, the stress table or chunking:

```sh
SPEACHY_BLESS=1 cargo test -p speachy-core --test stress   # rewrite stress-spoken.txt
python3 scripts/stress-eval.py marks --since main          # what the change did, word by word
```

Put the output in the pull request. Never edit `stress-spoken.txt` by hand.

To add a sentence, append it to the gold with no marks, run the two commands
above in that order, and the script refuses it with a draft: the sentence as
Speachy marks it, and the words Speachy left alone. Correct the draft against
the meaning of the sentence, put it in the gold, and run the commands again.
A sentence joins before anyone has heard Speachy read it, never because a word
in it was heard wrong, or the result stops being a rate.

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
- **Russian for Supertonic** is prepared in `russian.rs` (numbers, units,
  abbreviations) and `stress.rs` (stress marks, ё); the stress table is built by
  `scripts/make-stress-dict.py`.
- **Never panic on input.** Text, settings files, command JSON and model
  archives are untrusted: return an error message.

## What does need the desktop

Reading the selection, global shortcuts, the menu-bar icon, the overlay pill
and the Accessibility permission flow live in `src-tauri` and can only be
exercised in the running app; the macOS parts only on a Mac. Keep these thin.

Four scripts exercise the built app on a Mac, and run in CI on every pull
request: `scripts/smoke-app.sh` (launch), `scripts/e2e-macos.sh` (another
app in full screen, the shortcut with and without a selection, screenshots
of the pill), `scripts/e2e-update-macos.sh` (self-update from a local
server, and refusal of a tampered update) and
`scripts/e2e-permission-macos.sh` (Accessibility access given to one build
still holds for the build it updates to). Their screenshots and logs are uploaded as
workflow artifacts; look at them before claiming a desktop change works.

**Updates** (`src-tauri/src/updates.rs`) are the shell's: the app replaces
its own bundle. The feed is `latest.json` on the newest GitHub release;
archives are verified against `plugins.updater.pubkey`. Never commit the
private key, and never weaken verification to make a test pass —
`SPEACHY_UPDATE_ENDPOINT` redirects the feed for testing, but the signature
is still checked.

**Signing** (`scripts/macos-signing.sh`). macOS remembers Accessibility
access for an app's signature, so every release is signed with the same
self-signed certificate: the public half is `src-tauri/signing-certificate.pem`,
the private half the `MACOS_SIGNING_IDENTITY` secret. Never commit the
private key, and do not replace the certificate: a release signed with
another one loses every user's access. CI builds use a certificate made for
that run.

For interface work without a Mac, the app runs on Linux: `cargo run -p
speachy` (under `xvfb-run` where there is no display). The window scripts in
`ui/` can also be opened in a browser with `window.__TAURI__` stubbed, which
is how the screenshots in `docs/` were made.
