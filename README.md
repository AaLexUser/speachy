# Speachy

Select text in any app, press a shortcut, and hear it read aloud — by a
text-to-speech model that runs entirely on your computer.

Speachy is the mirror image of dictation tools such as Wispr Flow or
[Handy](https://github.com/cjpais/Handy): they turn speech into text, Speachy
turns text into speech. It lives in the menu bar, and a small pill at the
bottom of the screen shows what is being read.

![The Home tab while reading: the sentence being spoken is highlighted](docs/home.png)

## What it does

- **Speak the selection.** Press <kbd>⌃</kbd><kbd>⌥</kbd><kbd>S</kbd> with
  text selected anywhere. Press it again to stop, or select something else and
  press it to switch. <kbd>⌃</kbd><kbd>⌥</kbd><kbd>P</kbd> pauses and resumes,
  <kbd>⌃</kbd><kbd>⌥</kbd><kbd>X</kbd> stops.
- **Change the speed while it reads.** <kbd>⌃</kbd><kbd>⌥</kbd><kbd>=</kbd>
  is faster, <kbd>⌃</kbd><kbd>⌥</kbd><kbd>-</kbd> slower, in steps of 0.1.
  The new speed is heard at once, and no word is skipped: the voice keeps
  its pitch.
  <kbd>⌃</kbd><kbd>⌥</kbd><kbd>0</kbd> goes back to normal speed, and pressed
  again, back to yours. The pill shows the speed; clicking it does the same.
  The speed is remembered. Every shortcut can be changed.
- **Starts quickly, never waits.** Text is split into chunks. The first one is
  short, so speech starts as soon as it is synthesized; the following ones are
  synthesized ahead of playback and queued back to back, so there are no gaps
  between them. See [How the pipeline works](#how-the-pipeline-works).
- **Captions, if you want them.** Turn on *Show captions* and the words being
  read float above the pill, the current one highlighted, as in short videos.
  They are off by default.
- **Choose and download models.** The Models tab lists the supported models
  with their size and languages; downloads resume if interrupted.
- **Choose voices.** Pick a default voice and, if you like, a different voice
  for specific languages. Every voice has a preview button.
- **Detects the language.** Each part of the text is read in its own language
  with a voice that can speak it. Mixed-language text works.
- **Private.** Nothing you read leaves the machine. The network is used only
  to download models.

| Models | Voices |
| --- | --- |
| ![Models tab](docs/models.png) | ![Voices tab](docs/voices.png) |

## Models

| Model | Download | Languages | Voices | Notes |
| --- | --- | --- | --- | --- |
| **Supertonic 3** (recommended) | 129 MB | 31 | 10 | Fast, 44.1 kHz. The one to use with language detection. |
| Kokoro 82M v1.0 | 350 MB | English (US, British), Chinese | 36 | Expressive voices; needs a faster machine to stay ahead of playback. |
| Kokoro 82M v1.0 (compact) | 132 MB | same | 36 | 8-bit quantized: smaller download, somewhat slower. |
| Pocket TTS | 104 MB | English | 8 | Imitates recorded voices, so each sounds like a particular person; natural intonation. |
| KittenTTS Nano 0.2 | 33 MB | English | 8 | Tiny and fast; skips words missing from its dictionary. |
| KittenTTS Mini 0.1 | 163 MB | English | 8 | Larger KittenTTS model, same limitation. |

All of them run through [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx)
on the CPU. Model weights are downloaded from the sherpa-onnx `tts-models`
release into the app's data directory and carry their own licences
(Supertonic: OpenRAIL-M; Kokoro and KittenTTS: Apache-2.0; Pocket TTS:
CC BY 4.0, with voice recordings from Kyutai's
[tts-voices](https://huggingface.co/kyutai/tts-voices) under CC BY 4.0 and
CC0). The Russian stress dictionary is derived from
[RUAccent](https://huggingface.co/ruaccent/accentuator) (MIT).

## Install

Download the disk image for your Mac from the
[Releases](https://github.com/AaLexUser/speachy/releases) page
(`macos-arm64` for Apple Silicon, `macos-x64` for Intel), open it and drag
Speachy to Applications. The app is not notarized by Apple, so the first
time macOS will refuse to open it: allow it under **System Settings →
Privacy & Security → Open Anyway**, or run
`xattr -dr com.apple.quarantine /Applications/Speachy.app` once.

To build it yourself instead:

```sh
# Prerequisites: Rust (https://rustup.rs) and the Xcode command line tools.
cargo install tauri-cli --version "^2" --locked

cargo tauri dev      # run from source
cargo tauri build    # produce target/release/bundle/macos/Speachy.app and a .dmg
```

The first build downloads the prebuilt sherpa-onnx library for your platform.
There is no JavaScript toolchain: the interface in `ui/` is plain HTML, CSS
and JavaScript.

First run:

1. Open the **Models** tab and download a model (Supertonic 3 is a good
   start).
2. On macOS, grant **Accessibility** access when asked (System Settings →
   Privacy & Security → Accessibility). Speachy needs it to read the text
   selected in other apps.
3. Select some text anywhere and press <kbd>⌃</kbd><kbd>⌥</kbd><kbd>S</kbd>.

Closing the window keeps Speachy running in the menu bar; quit from the menu
bar icon. *Launch at login* and *Start in the menu bar* are in Settings.

> An app you build yourself is ad-hoc signed, and macOS ties the Accessibility
> permission to the signature, so a rebuilt app has to be allowed again:
> press *Open settings* in the app's banner and switch Speachy on. To keep
> access across your own builds, sign them with a certificate of your own
> (`scripts/macos-signing.sh`).

## How the pipeline works

```
selection ──► clean up ──► chunks ──► language + voice per chunk
                                              │
                    ┌─────────────────────────┘
                    ▼
        synthesis worker ──► gapless audio queue ──► speakers
        (runs ahead of         (chunk, pause, chunk, …)
         playback)
```

1. **Reading the selection** (`src-tauri/src/selection.rs`). On macOS the
   Accessibility API is asked for the focused element's selected text. Apps
   that do not expose it (many browsers and Electron apps) get a simulated
   <kbd>⌘</kbd><kbd>C</kbd> instead, after which the previous clipboard
   contents are put back.
2. **Chunking** (`crates/speachy-core/src/text.rs`). Markdown markup, URLs and
   hard line wraps are cleaned up, then the text is cut at sentence
   boundaries (and at clause boundaries inside very long sentences, never
   inside a word). The first chunk is at most ~110 characters. Each later
   chunk may be as long as everything before it combined, up to 280
   characters: as long as synthesis is at least twice as fast as playback, a
   chunk is always ready before it is needed.
3. **Language and voice** (`lang.rs`, `plan.rs`). The language of the whole
   text and of each chunk is detected from its script, common words and
   letter statistics (a change of script always starts a new chunk). Each chunk then gets a voice: your
   choice for that language, else the default voice if it speaks the
   language, else any installed model that does.
4. **Synthesis ahead of playback** (`pipeline.rs`). A worker synthesizes the
   chunks in order and keeps going until a configurable amount of audio
   (20 s by default) is waiting, then idles until playback catches up — so
   stopping early wastes little work. The model is loaded once and kept warm;
   by default it is preloaded at launch.
5. **Gapless playback** (`audio.rs`). Finished chunks are appended to one
   audio queue and play back to back. Silence at the edges of each chunk is
   trimmed and replaced by a deliberate pause that depends on the boundary
   (clause, sentence, paragraph), so the rhythm does not depend on where the
   text happened to be cut. The model always speaks at its natural pace;
   speed is applied to the audio as it is played, by a time-stretch that
   keeps the pitch (`stretch.rs`), so a change is heard within 15 ms.

For English with Kokoro, words are converted to phonemes by
[misaki-rs](https://github.com/MicheleYin/misaki-rs), the Rust port of the
phonemizer Kokoro was trained with (`english.rs`); words it does not know are
sounded out by spelling rules rather than skipped.
For Russian with Supertonic, numbers, units and abbreviations are first
written out as words, in the case the surrounding words call for
(`russian.rs`). Then the stressed vowel of each word a dictionary knows is
marked so the voice stresses it there, and "ё" is put back where the text
prints "е" for it, as most Russian text does (`stress.rs`). Words that can be
read two ways, such as "замок" or "все", are left as written.
`speachy-cli plan` shows the result under each chunk it changes, so you can
see what the voice was given when a word is misread.
`scripts/stress-eval.py marks` checks those marks against Russian sentences
whose stress is marked (the file's header says who placed the marks), and says
which words Speachy marks on another vowel, leaves unmarked or rewrites; it
needs no model and runs on every pull request. It does not yet measure what the voice does with a mark.

## Engine and interface

The engine is a library with no user-interface dependencies; the desktop app
is a thin shell around it. Everything the app does is an engine command, and
the same commands are available from a terminal, which makes the engine easy
to script, debug and test without a window, a screen or a sound card.

```
crates/speachy-core/   The engine
  src/session.rs         the front door: commands in, events out
  src/catalog.rs         downloadable models and their voices
  src/models.rs          download (resumable), unpack, remove
  src/text.rs            cleaning and chunking
  src/lang.rs            language detection
  src/plan.rs            language and voice for each chunk
  src/engine.rs          sherpa-onnx synthesis, loaded-model cache
  src/english.rs         English phonemes for Kokoro
  src/pipeline.rs        synthesize-ahead pipeline, pause / stop
  src/audio.rs           gapless output queue
  src/captions.rs        when each word is heard, and caption lines
crates/speachy-cli/    Headless front end: subcommands and a JSON channel
src-tauri/             The desktop shell (Tauri 2): windows, menu-bar icon,
                       shortcuts, reading the selection
ui/                    Settings window, overlay pill and captions (no build step)
```

```sh
cargo build --release -p speachy-cli      # target/release/speachy-cli

speachy-cli models                         # what can be installed, what is
speachy-cli install supertonic-3
speachy-cli plan "Hello. Привет, как дела?"   # chunks, language and voice of each, and the words the voice is given
speachy-cli speak "Hello from Speachy."
speachy-cli speak --voice M2 --out hello.wav "Hello."
speachy-cli run '{"cmd":"snapshot"}'       # any engine command, as the app sends it
speachy-cli serve                          # JSON commands on stdin, replies and events on stdout
```

The CLI works on the same models and settings as the app unless `--home DIR`
is given. [AGENTS.md](AGENTS.md) lists the commands and events and describes
how to work on the engine headlessly.

## Development

```sh
cargo test --workspace                          # everything
cargo test -p speachy-core -p speachy-cli       # engine and CLI only; needs no GUI libraries
cargo clippy --workspace --all-targets -- -D warnings
```

The app also builds and runs on Linux (X11), where the shortcut reads the
primary selection. Building there needs the Tauri prerequisites plus
`libasound2-dev`, `libssl-dev` and `libxdo-dev`. Windows is untested.

### Updates

The app checks GitHub for a newer release shortly after launch and every
few hours (this can be turned off), and offers it under **Settings →
Updates**; nothing is installed without asking. `Speachy.app/Contents/MacOS/speachy
--update` does the same from a terminal. Only published, non-pre-release
versions are offered.

An update is an archive of the app signed with a key whose public half is
built into the app (`plugins.updater.pubkey` in `src-tauri/tauri.conf.json`).
The private half must be available to the Release workflow as the
repository secret `TAURI_SIGNING_PRIVATE_KEY`; without it releases are still
built, but installed apps cannot update to them. Keep that key safe: apps
only accept updates signed with it, so losing it means users have to
reinstall by hand.

macOS ties a permission such as Accessibility to the signature of the app
it was given to. Releases are therefore all signed with one certificate
(`src-tauri/signing-certificate.pem`; its private key is the repository
secret `MACOS_SIGNING_IDENTITY`), so access given once carries over to
every later update. Like the update key, that key must be kept: a release
signed with another one, or built without the secret, makes macOS ask every
user again. The certificate is self-signed, not issued by Apple, so it
does not change the *Open Anyway* step on first launch. Versions up to
0.1.2 were signed without it; updating from one of them asks for access one
last time.

### Releasing

Set the version in `Cargo.toml` on `main`, then push a tag `vX.Y.Z` that matches it.
The **Release** workflow builds the app for Apple Silicon and Intel and the
CLI for macOS and Linux, checks each build (the CLI installs a model and
synthesizes a sentence; the app is launched), and publishes a GitHub release
with the files, their checksums, and the signed update archives and
`latest.json` that installed apps look for. Running the workflow by
hand from the Actions tab does the same but leaves the release as a draft;
the **Publish release** workflow then makes it public.

## Status and known limitations

The engine (chunking, language detection, downloads, the pipeline, the
command surface) is covered by tests. The built app is also run on a Mac for
every pull request: it is launched, the shortcut is pressed in another app in
full screen with and without a selection, it updates itself from a test
server, and Accessibility access is checked to survive that update. What
those checks cannot judge is how the voices sound.

- Kokoro needs a reasonably fast CPU. If synthesis is slower than playback
  you will hear pauses between chunks (the pill shows *Preparing…*).
- KittenTTS skips words that are not in its dictionary.
- Piper voices are not included: sherpa-onnx 2.0 no longer bundles the
  phonemizer they need.
- Language detection on a handful of words is unreliable by nature; short
  selections fall back to the language set under Voices → Language.
- The simulated <kbd>⌘</kbd><kbd>C</kbd> uses the physical key position of
  "C" on a QWERTY layout.
- The pill follows the pointer to the display it is on; with several
  displays this has not been tried on real hardware.
- Captions estimate when each word starts from the text and the loudness of
  the audio. In Russian 94 % of words are within 0.15 s; English is less
  exact. Numbers and abbreviations are highlighted as written, for as long
  as they take to read.
