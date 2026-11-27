<!-- The body of each GitHub release. Rewrite "What's new" in the pull request that sets the version. -->
Select text anywhere, press ⌃⌥S, and hear it read aloud by a text-to-speech model that runs on your computer.

## What's new

- **Captions.** While Speachy reads, the line being read can float above the pill, with the word you are hearing highlighted and slightly larger. Numbers are shown as written: "3,5 млрд" stays highlighted while "три с половиной миллиарда" is read. The captions let clicks through to the app under them, and they follow speed changes and pauses. They are off by default: turn on **Show captions** under Settings. With captions on, the pill shrinks to its controls.
- **KittenTTS no longer ends each sentence with noise.** Its chunks ended with a hiss, sometimes a murmur, and a click that were not in the text. They are gone, and the gaps between sentences are shorter. Pauses inside a sentence now have the length the voice gives them.

## Downloads

| File | For |
| --- | --- |
| `Speachy-{{version}}-macos-arm64.dmg` | The app, for Macs with Apple Silicon (M1 and later) |
| `Speachy-{{version}}-macos-x64.dmg` | The app, for Intel Macs |
| `speachy-cli-{{version}}-macos-arm64.tar.gz`, `…-macos-x64.tar.gz` | The command-line tool for macOS |
| `speachy-cli-{{version}}-linux-x64.tar.gz` | The command-line tool for Linux (x86-64, glibc 2.35 or later; needs `libasound2` and `libssl3`) |
| `Speachy-{{version}}-macos-….app.tar.gz`, `latest.json` | Used by the app to update itself; no need to download these |
| `SHA256SUMS.txt` | Checksums of the files above |

Requires macOS 11 or later. The desktop app is not packaged for Linux or Windows; on Linux it can be built from source.

## Installing on macOS

1. Open the disk image and drag **Speachy** to Applications.
2. The app is not notarized by Apple, so macOS will refuse to open it the first time. Either open **System Settings → Privacy & Security**, scroll to the message about Speachy and choose **Open Anyway**, or run this once in Terminal:

   ```sh
   xattr -dr com.apple.quarantine /Applications/Speachy.app
   ```

3. Open the **Models** tab and download a model. Supertonic 3 is a good first choice.
4. Grant **Accessibility** access when asked (System Settings → Privacy & Security → Accessibility). Speachy needs it to read the text you have selected in other apps.

## Updating

Speachy looks for new versions by itself and offers them under **Settings → Updates** (also: **Check for Updates…** in the menu-bar menu). Accessibility access carries over from one version to the next, starting with 0.1.4; an update from an earlier version asks once more.

The command-line tool is a single file: unpack it and run `./speachy-cli --help`. If macOS blocks it, remove the quarantine flag the same way: `xattr -d com.apple.quarantine speachy-cli`.
