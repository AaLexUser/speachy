<!-- The body of each GitHub release. Rewrite "What's new" in the pull request that sets the version. -->
Select text anywhere, press ⌃⌥S, and hear it read aloud by a text-to-speech model that runs on your computer.

## What's new

- **Change the speed while it reads.** ⌃⌥= is faster and ⌃⌥- slower, in steps of 0.1. The sentence being read finishes, and the next one has the new speed. ⌃⌥0 goes back to normal speed; pressed again, it goes back to yours. The pill shows the speed, and clicking it does the same as ⌃⌥0. The speed is remembered, and the three shortcuts can be changed under Settings.
- **Russian reads better with Supertonic 3.** Numbers, units and abbreviations are read as words: "3,5 млрд рублей", "−20 °C", "и т. д.". Each word gets its stress from a dictionary, and "ё" is put back where the text prints "е". Words that can be read two ways, such as "замок", are read as before.
- **Accessibility access survives updates.** macOS remembers the permission for an app's signature, and until now every release had a signature of its own, so each update had to be allowed again. From this version on all releases share one. Updating to this version from an earlier one asks one last time: press **Open settings** in Speachy and switch it on.
- **The pill moves with the voice.** Its bars now show how loud the speech is as you hear it, instead of a fixed animation, in a plainer style.
- **Pocket TTS**, a new English model (104 MB) with eight voices that each sound like a particular person.

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
