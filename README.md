<p align="center">
  <img src="src-tauri/icons/icon.png" alt="MHF Launcher" width="200">
</p>

# mhf-outpost

GUI installer and launcher for Monster Hunter Frontier, part of the [Mogapedia](https://mogapedia.fr) preservation ecosystem.

mhf-outpost downloads and verifies game archives, authenticates against an [Erupe](https://github.com/Mezeporta/Erupe) server, and launches the game via [mhf-iel](https://github.com/rockisch/mhf-iel). Authentication is handled entirely inside the launcher — the game only receives control once a valid session token has been written to `config.json`.

Built with [Tauri 2](https://tauri.app) (Rust backend) and Vue 3 (frontend).

## Features

- Download and verify game archives from archive.org (G10, GG, G91, G52, G2, G1, F5, F4, S6, Wii U)
- SHA-1 archive integrity check before extraction; SHA-256 per-file verification after
- Resume interrupted downloads
- Server tab independent of game version — authenticate once, play any installed version
- Login, register, and character selection against any Erupe-compatible server
- Bundles `mhf-iel-cli.exe` directly in the binary — no network fetch, no GitHub dependency
- System check: DirectX 9 / Wine + DXVK, Japanese fonts, game directory health
- Windows Defender exclusion helper
- Cross-platform: Windows (native), Linux (Wine)

## Architecture

```
mhf-outpost (this)
    │  authenticate → writes config.json
    ↓
mhf-iel-cli.exe  ──→  mhf.exe  ──→  mhfo-hd.dll / mhfo.dll
                                          (game takes over)

mhf-outpost ←──── Erupe server (auth API)
mhf.exe     ←──── Erupe server (game protocol)
```

## Prerequisites

### Build tools

| Tool | Version | Notes |
|------|---------|-------|
| Rust | stable | via [rustup](https://rustup.rs) |
| Node.js | 20+ | |
| npm | 10+ | bundled with Node |

### Linux system libraries (for Tauri)

```bash
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev libappindicator3-dev \
  librsvg2-dev patchelf libgtk-3-dev
```

### Windows

WebView2 runtime is required (pre-installed on Windows 10 1803+ and Windows 11).

## Development

```bash
npm install
npm run dev          # Vite dev server + Tauri dev window (hot reload)
```

## Building

```bash
npm run build        # Frontend only (dist/)
npm run tauri build  # Full Tauri release bundle (includes Rust backend)
```

The Rust core library and CLI can be built independently:

```bash
cargo build --release          # builds mhf-outpost CLI binary
cargo build --release --package mhf-launcher-app   # Tauri backend only
```

## CLI usage

The `mhf-outpost` binary exposes the full feature set without the GUI:

```bash
mhf-outpost list
mhf-outpost info G10
mhf-outpost download --version G10 --path ~/mhf
mhf-outpost verify  --version G10 --path ~/mhf
mhf-outpost fetch-launcher --path ~/mhf
mhf-outpost launch --path ~/mhf
mhf-outpost check
mhf-outpost hash-dir ~/mhf          # generate manifest hashes
```

## Manifests

Each supported game version has a TOML manifest in `manifests/` that records the archive source, expected SHA-1, and per-file SHA-256 hashes. File kinds control how verification failures are classified:

| Kind | Meaning |
|------|---------|
| `core` | Game executable / DLL — any modification is an error |
| `url` | Server URL lists — expected to differ for community servers |
| `translation` | Game data files — expected to differ for fan translations |
| `config` | User config / save data — always user-specific |

## Related projects

| Project | Role |
|---------|------|
| [Erupe](https://github.com/Mezeporta/Erupe) | Server emulator (sign, entrance, channel servers) |
| [mhf-iel](https://github.com/rockisch/mhf-iel) | Thin launcher that starts `mhf.exe` from `config.json` |

## License

[MIT](LICENSE)
