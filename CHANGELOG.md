# Changelog

All notable changes to mhf-outpost are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- **Japanese text was garbled on Linux.** The game's text uses the Japanese
  code page (CP932), which Wine only selects under a Japanese locale, and the
  launcher started Wine in the user's own language. The game now always runs
  under `ja_JP.UTF-8`; the launcher's own interface keeps your language. The
  system check reports an error if the `ja_JP.UTF-8` locale is not installed.

### Changed

- **The system check now recommends DXVK 2.7.1** instead of plain
  `winetricks dxvk`, which installs DXVK 3.x. DXVK 3.x needs Wine 10.1 or
  newer; on older Wine (such as the Wine 9.0 shipped by Ubuntu 24.04) it finds
  no graphics adapter and the game closes on launch.

## [0.1.2] — 2026-09-28

### Fixed

- **The game failed to launch with "VCRUNTIME140.dll not found"** ([#4]).
  The bundled launcher needed the Visual C++ Redistributable, which is
  missing on some Windows installs and in fresh Wine prefixes. The runtime
  is now built into the launcher, so nothing extra has to be installed.
  Players who installed the redistributable or ran `winetricks vcrun2022`
  as a workaround don't need to undo anything.

[#4]: https://github.com/Mogapedia/mhf-outpost/issues/4

## [0.1.1] — 2026-09-27

### Fixed

- **New characters were disconnected on first login** ([#1]). The launcher
  always told the game that the character already existed, so the game
  skipped character creation and asked for a save the server did not have
  yet, and the server closed the connection. A newly created character now
  opens character creation. Players whose first login failed just need to
  sign in again with this version: their unfinished character is picked up
  and creation starts normally.

[#1]: https://github.com/Mogapedia/mhf-outpost/issues/1

## [0.1.0] — 2026-09-19

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
  dependency on launch, no GameGuard hooks. The stub is refreshed on every
  launch, so upgrading mhf-outpost never leaves a stale one in the game
  folder. Quick-play button in the top bar for the last-played version.
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

[Unreleased]: https://github.com/Mogapedia/mhf-outpost/compare/v0.1.2...HEAD
[0.1.2]: https://github.com/Mogapedia/mhf-outpost/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/Mogapedia/mhf-outpost/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/Mogapedia/mhf-outpost/releases/tag/v0.1.0
