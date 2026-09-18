# Changelog

All notable changes to mhf-outpost are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Patch server sync** (`sync` command, "Update game files" button): install
  or update a game directory from an MHF patch server — the same per-file
  CRC32 mechanism the original `mhl.dll` launcher uses (ZeruLight "MHF Patch
  Server API" layout: `mhf_file.php?key=` manifest + `mhfdat/{exe,dat}` tree).
  Files are compared by size then CRC32, only differences are downloaded
  (4 parallel connections, `.part` files verified before being renamed into
  place, 3 retries). Works on an empty folder, so a server that advertises a
  patch server no longer needs an archive.org source or a 5 GB zip. The
  request shapes were recovered from `mhl.dll`, where they are stored
  obfuscated (each byte offset by 0x10).
- `authenticate` now returns the server's `patchServer`; the GUI shows the
  update button in the session card whenever the server advertises one.
- **Server-first welcome flow.** First run now asks for a server address,
  reads `/v2/server/info` to learn which client version it runs, signs the
  user in, installs the game from the server's patch tree into a chosen
  folder, and offers Play — four steps, no version picker, no download
  source to choose. Servers without a patch server get a "use an existing
  folder" path instead. `get_server_info` Tauri command added.

### Changed

- Library: when signed in to a server that provides files, "Install from
  server" is the primary action; the preservation-archive download is
  demoted to "From archive (advanced)". The "No archive source yet" banner
  only appears when neither route is available.
- README, crate and CLI descriptions reworded: mhf-outpost is a launcher for
  Erupe servers and contains no game data; the archive path is for
  archivists and operators.

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
