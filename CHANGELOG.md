# Changelog

All notable changes to mhf-outpost are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] — TBD

First public release of mhf-outpost. The launcher can now take a fresh user
from "no game files" to "in-game" without leaving the GUI.

### Added

- **Game library**: 29 documented MHF versions across the Season, Forward, G,
  and Z generations, plus a Wii U entry. 12 versions ship a verified
  archive.org source; the rest are stubs awaiting an upload.
- **Download & verify**: streams archives from archive.org with a resumable
  HTTP range request, checks the SHA-1 against the manifest, then extracts
  ZIP/RAR/7z. Per-file SHA-256 verification flags tampered or partial installs
  with a four-tier severity (`core` / `url` / `translation` / `config`).
- **In-app authentication**: login and registration against any
  Erupe-compatible server via the `/v2/login` and `/v2/register` endpoints.
  Character selection and creation happen before the game ever runs; no
  password is ever written to disk.
- **One-click launch**: writes a valid `config.json` and runs the bundled
  `mhf-iel-cli.exe`. The launcher binary is embedded in the Rust executable
  at compile time — no GitHub fetch, no network dependency on launch.
- **Quick-play top bar**: returning users see a Steam-style Play button for
  the most recently launched version, persisted across sessions.
- **Translations panel**: downloads `translations-translated.json` from the
  MHFrontier-Translation GitHub releases and patches `mhfdat.bin` /
  `mhfpac.bin` natively (decrypt → decompress → rewrite pointer tables →
  recompress → re-encrypt) without any external tooling.
- **System checks**: probes for DirectX 9 / Wine + DXVK, Japanese fonts, and
  game directory health, surfacing actionable fixes.
- **Windows Defender exclusion helper**: adds the game folder via PowerShell
  with automatic UAC elevation when not run as administrator.
- **CLI parity**: every GUI feature (`list`, `info`, `download`, `verify`,
  `fetch-launcher`, `launch`, `check`, `hash-dir`) is also reachable from the
  `mhf-outpost` binary for scripting.
- **Cross-platform packaging**: GitHub Actions builds `.deb`, `.rpm`,
  `.AppImage`, `.msi`, and `.exe` bundles on every `v*` tag.

### Known limitations

- The installer does not yet check whether the chosen install folder is empty
  before extracting; pick a fresh directory.
- 17 of the 29 manifests are documentation-only stubs without a downloadable
  archive (`f1`–`f3`, `g3`, `g6`–`g9`, `s1`–`s5`, `s7`–`s10`, `gg`).
- Authentication state is held in memory only; signing in again is required
  after restarting the launcher.
