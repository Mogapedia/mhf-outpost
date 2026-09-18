# Changelog

All notable changes to mhf-outpost are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] — TBD

First public release. mhf-outpost is a launcher for Erupe servers: it signs
in, keeps the game files in sync with the server, applies community
translations and boots the game without Internet Explorer. It contains no
game data.

### Added

- **Guided first run**: server address → `/v2/server/info` picks the client
  version → sign in → install from the server into a chosen folder → Play.
  Four steps, no version picker, no download source to choose. Servers
  without a patch server get a "use an existing folder" path. Reopenable any
  time from Settings.
- **Sign-in**: login and registration against any Erupe-compatible server
  (`/v2/login`, `/v2/register`); character selection and creation happen
  before the game runs; no password is ever written to disk. Addresses are
  normalised (bare host → `http://host:8080`, pasted paths dropped) and a
  wrong endpoint is reported as such rather than as a web server's 404 page.
- **Patch server sync** (`sync` command, "Install / update game files"): the
  per-file CRC32 update mechanism of the original `mhl.dll` launcher,
  reimplemented (ZeruLight "MHF Patch Server API" layout: `mhf_file.php?key=`
  manifest + `mhfdat/{exe,dat}` tree). Files are compared by size then CRC32
  and only differences are downloaded — 4 parallel connections, `.part`
  files verified before being renamed into place, 3 retries — so a first
  install and a routine update are the same button. The request shapes were
  recovered from `mhl.dll`, where they are stored with every byte offset by
  0x10.
- **Launch without IE**: writes a valid `config.json` and runs the bundled
  32-bit boot stub, embedded in the executable at compile time — no network
  dependency on launch, no GameGuard hooks. Quick-play button in the top bar
  for the last-played version.
- **Translations**: downloads the per-language `translations-<lang>.json.gz`
  payload from MHFrontier-Translation releases and patches `mhfdat.bin` /
  `mhfpac.bin` natively (decrypt → decompress → rewrite pointer tables →
  recompress → re-encrypt), no external tooling.
- **System checks**: DirectX 9 / Wine + DXVK, Japanese fonts, game directory
  health, with actionable fixes. Windows Defender exclusion helper (PowerShell
  with UAC elevation).
- **Verify**: per-file SHA-256 manifests for 29 documented client versions
  (Season, Forward, G, Z, Wii U), with a four-tier severity
  (`core` / `url` / `translation` / `config`) so a broken install is
  diagnosed rather than guessed at.
- **Advanced: preservation archives**: 12 manifests record a public archive
  source; `download` streams it with resumable range requests, checks the
  SHA-1, extracts ZIP/RAR/7z and refuses to extract over an unrecognised
  non-empty folder. Shown as "From archive (advanced)" in the Library when a
  server route exists.
- **CLI parity**: `login`, `sync`, `launch`, `check`, `server-info`, `verify`,
  `translate`, `list`, `info`, `download`, `hash-dir`.
- **Cross-platform packaging**: GitHub Actions builds `.deb`, `.rpm`,
  `.AppImage`, `.msi`, and `.exe` bundles on every `v*` tag.

### Known limitations

- The pre-launch sync reads every listed file to CRC it (about 5 GB); fast on
  an SSD or a warm cache, slow on a spinning disk.
- 17 of the 29 manifests are documentation-only stubs without a downloadable
  archive (`f1`–`f3`, `g3`, `g6`–`g9`, `s1`–`s5`, `s7`–`s10`, `gg`).
- Authentication state is held in memory only; signing in again is required
  after restarting the launcher.
