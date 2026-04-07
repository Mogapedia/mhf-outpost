# mhf-iel (vendored)

This is a vendored, frozen copy of [`rockisch/mhf-iel`](https://github.com/rockisch/mhf-iel),
imported into `mhf-outpost` as the **authoritative source** for `mhf-iel-cli.exe`.

The upstream repository is no longer maintained. All future fixes and changes
to the launcher code happen here, not upstream.

## Layout

- `src/` — the `mhf-iel` library: builds the in-process `DataZZ`/`DataF5` struct
  and calls `mhDLL_Main` from `mhfo-hd.dll` / `mhfo.dll`.
- `mhf-iel-cli/` — thin `clap` wrapper that reads `config.json` and invokes
  `mhf_iel::run`. This is the only crate that produces a binary.
- `Cargo.toml` — standalone Cargo workspace, **excluded** from the parent
  `mhf-outpost` workspace (see `mhf-outpost/Cargo.toml` `workspace.exclude`).
  Running `cargo build` here is independent of building `mhf-outpost` itself.

The `mhf-iel-auth` crate from upstream has been deleted: authentication is
implemented in pure Rust in `mhf-outpost/src/auth.rs` and no longer needs a
separate Windows binary.

## Rebuilding `mhf-iel-cli.exe`

The output of this crate is checked in at `mhf-outpost/resources/mhf-iel-cli.exe`
so that ordinary `cargo build` of `mhf-outpost` requires no extra toolchain.
**Do not edit that binary by hand.** To regenerate it after changing the source,
run from the `mhf-outpost` root:

```sh
./scripts/rebuild-launcher.sh
```

That script cross-compiles to `i686-pc-windows-msvc` via
[`cargo-xwin`](https://github.com/rust-cross/cargo-xwin) (works on Linux,
macOS, and Windows hosts) and copies the output into `resources/`.

## Why this can't be a normal workspace member of `mhf-outpost`

`mhf-iel-cli` must be built for **32-bit Windows** (`i686-pc-windows-msvc`)
because it loads the 32-bit `mhfo-hd.dll` into its own process and calls
`mhDLL_Main` with in-process `HMODULE` / `HGLOBAL` / `HKL` handles. The
`mhf-outpost` binary, by contrast, is built for the host's native target
(typically `x86_64`). They are two different `cargo build` invocations against
two different targets, so they live in two different workspaces.
