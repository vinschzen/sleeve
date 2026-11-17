# Sleeve

A local music library for FLAC/MP3 collectors. Tauri 2 + Rust, phone-first UI.

```
crates/core   Rust library: scanner, SQLite index, play log, stats, listen-together server
src-tauri     Tauri 2 shell (desktop, Android, iOS). One command forwards to the core.
ui            Front end: plain HTML/CSS/JS, no build step, no framework
```

## What works

- **Library**: scans folders (FLAC, DSD as DSF/DFF, MP3, ALAC/AAC, WAV, AIFF, OGG, Opus, APE, WavPack), reads tags and
  audio properties, grades each file (hi-res / lossless / lossy 256+ / lossy). Rescans only read changed files.
- **Browse and search**: tracks, albums, artists; prefix search over title, artist and album (SQLite FTS5).
- **Playback**: queue follows the list you started from, lock-screen controls.
- **Now playing**: album cover (embedded art, or a cover/folder image beside the file, or a generated sleeve),
  kHz / bit / kbps plate, a real waveform you drag to seek, live sample counter and level, and a spec sheet:
  codec, bandwidth, theoretical SNR, size vs raw PCM, file size, path, play history.
- **Editing**: press and hold a track (or tap (More) on the now-playing screen) to edit title, artist, album,
  year, track number and genre, or move it to another album. Edits are written into the file's own tags and the
  index. If a file can't be written (read-only storage), the edit is kept in Sleeve only. Albums can be renamed
  from the album view, which rewrites every track on them.
- **Albums**: an album is whatever its tracks' tags say: tracks sharing an album title and album artist. The
  album page is gathered from those tracks (years, length, size, formats, genres) and the album list shows cover
  thumbnails. Edit an album's title, artist, year and genre, or change its cover; changes are written to every
  track's tags (the cover is embedded as front-cover art, scaled to at most 1400 px) and remembered by Sleeve.
- **DSD**: DSF and DFF files (DSD64 to DSD512) are indexed with their ID3 tags and embedded cover. No webview can
  play DSD, so the core converts it to 24-bit PCM at 88.2 kHz (96 kHz for the 48 kHz family) while streaming:
  a 90 dB Kaiser low-pass, +6 dB SACD reference gain, random access for seeking, nothing written to disk. The
  waveform and level measurements run on the same conversion. Multichannel files play their front pair.
- **Playlists**: your own ordered lists, separate from albums. Create, rename, delete, add and remove tracks.
- **Measured from the audio**: the file is decoded once (symphonia) for sample peak, RMS, crest factor,
  clipped samples and a DR figure. Results are cached. Opus has no decoder here, so it shows n/a.
- **Touch feedback**: haptics through `tauri-plugin-haptics` on phones (Vibration API in browsers), press and
  release animations on every control, a tick every 2% while scrubbing.
- **Stats**: plays and hours; by hour, weekday, month, wet/dry season or four seasons; a day x hour dot graph;
  top tracks, artists, genres (tap one to see when you play it); a genre sphere for the collection.
- **Similar**: other artists' tracks that share genre tags, closest in year first.
- **Listen together**: a timed, token-protected link that streams what you are playing to a friend's browser
  and keeps them in step with you. Same network only (or a private network such as Tailscale).

## Run it

Prerequisites: Rust, and the [Tauri 2 prerequisites](https://tauri.app/start/prerequisites/) for your platform.

```sh
cargo install tauri-cli --version "^2"
cargo tauri icon src-tauri/app-icon.png     # generates the full icon set, once

cargo tauri dev                             # desktop
cargo tauri android init && cargo tauri android dev
cargo tauri ios init && cargo tauri ios dev # needs a Mac
```

Without Tauri, in any browser, against the real Rust core:

```sh
cargo run -p sleeve-core --bin sleeve-dev -- sleeve.db ui 4717   # then open http://127.0.0.1:4717
```

Opening `ui/index.html` directly shows the UI with an invented demo library.

### Android APK

Easiest: put this folder in a GitHub repository, open the **Actions** tab, pick **Android APK** and press
**Run workflow**. About 20 minutes later the run's **Artifacts** holds `Sleeve.apk`; download it on the phone and
install it (allow "install unknown apps" for your browser or file manager). The workflow is
`.github/workflows/android.yml`. It has not been run yet, so expect to fix a step or two on the first try.

On your own computer (Android Studio with SDK + NDK installed, `ANDROID_HOME` and `NDK_HOME` set):

```sh
rustup target add aarch64-linux-android armv7-linux-androideabi
cargo install tauri-cli --version "^2"
cargo tauri icon src-tauri/app-icon.png
cargo tauri android init
python3 scripts/android-setup.py            # permissions + first-launch permission prompt
cargo tauri android build --apk --target aarch64
```

For builds that install over each other, create one signing key and add it to the repository's secrets as
`ANDROID_KEYSTORE_B64` (the file, base64) and `ANDROID_KEYSTORE_PASSWORD`, with key alias `sleeve`:
`keytool -genkeypair -keystore sleeve.jks -alias sleeve -keyalg RSA -keysize 2048 -validity 10000`.

### Phone setup

- **Android**: `scripts/android-setup.py` adds the storage permissions and asks for them on first launch. If a
  scan finds nothing, allow "Music and audio" for Sleeve in system settings. Editing tags on shared storage
  needs Android's "All files access", which Sleeve does not request; without it edits are kept in Sleeve only. The default scan path is
  `/storage/emulated/0/Music`. The listen-together link also needs `android.permission.INTERNET` (Tauri adds it).
- **iOS**: apps can only read their own sandbox. Enable file sharing (`UIFileSharingEnabled`,
  `LSSupportsOpeningDocumentsInPlace`) and copy music into Sleeve's folder in the Files app, then scan that folder.

## What has and hasn't been tested

Tested here: core unit tests; a scan of 40 generated FLAC/MP3 files; search, stats, similar; the session link
(page, state, ranged audio, expiry, bad token); real playback through `sleeve-dev` in headless Chrome at phone size;
`cargo check` of the Tauri shell on Linux;
tag editing (FLAC and MP3, checked with ffprobe), album rename, playlists, cover replacement
(embedded art confirmed with ffprobe) and thumbnails;
DSD conversion of generated DSD64 files in both DSF and DFF (tone frequency and level exact, cross-checked
with ffmpeg's decoder); cover extraction and audio analysis on generated FLAC/MP3 files (a sine reads -18.1 dBFS peak, -21.1 RMS as expected).

Not tested: the app running on an Android or iOS device, haptics on real hardware, and a library of real size. Expect to adjust
permissions and paths on the first phone build.

## Performance notes

- Every list query walks an index and returns 100 rows; the UI keeps about 25 rows in the DOM whatever the library size.
- The scanner parses tags on all cores and writes in batched transactions; cover art is skipped.
- SQLite runs in WAL mode, so browsing stays responsive during a scan.
- Charts are plain DOM; the sphere is one canvas that only animates while it is on screen.

## Known limits and next steps

- Audio goes through the system webview's decoder. It plays FLAC, but it is not bit-perfect and has no exclusive
  DAC mode. A native output path (symphonia + cpal/Oboe) is the next audio milestone.
- Cover thumbnails appear in the album list and album page, not yet in the track list.
- Choosing a cover image uses the webview's file picker, which has not been tried on Android or iOS.
- The DR figure follows the published TT DR method but has not been checked against the official meter.
- Peak is a sample peak, not an oversampled true peak.
- "Similar" uses genre tags and year only. Audio-based similarity needs an embedding model.
- DSD is always converted to PCM; there is no native DSD or DoP output to a DAC. DST-compressed DFF files and SACD
  disc images (.iso) are not supported: extract an ISO to DSF first (for example with `sacd_extract`). Tags in
  DSF/DFF files can't be rewritten yet, so edits to them are kept in Sleeve only.
- Playlists can't be reordered yet, and there is no M3U import or export.
- An edit kept in Sleeve only is replaced by the file's tags if that file later changes on disk.
- No sync between phone and desktop yet. The play log is append-only with unique ids so two logs can be merged later.
- Android playback of files outside the app's own storage depends on the asset protocol reaching that path;
  if a track won't play, that is the first place to look.
