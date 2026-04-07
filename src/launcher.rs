use anyhow::{bail, Context, Result};
use std::path::Path;

// ── Embedded launcher binary ──────────────────────────────────────────────────
//
// `mhf-iel-cli.exe` is a 32-bit Windows binary that loads `mhfo-hd.dll` into
// its own process and calls `mhDLL_Main` with a hand-built data struct of
// in-process Win32 handles. Because mhf-outpost is built for the host's
// native target (typically x86_64), it cannot host that DLL itself — a
// separate i686-pc-windows-msvc executable must exist on disk and be
// exec'd (under Wine on Linux).
//
// The authoritative source lives in `vendor/mhf-iel/` (a frozen, vendored
// copy of the upstream `rockisch/mhf-iel` repo — see its README for details).
// We bundle a prebuilt copy in `resources/` so that ordinary `cargo build`
// requires no Windows cross-compile toolchain. To regenerate the binary
// after editing `vendor/mhf-iel/`:
//
//     ./scripts/rebuild-launcher.sh
//
// Authentication (previously `mhf-iel-auth.exe`) is implemented in pure Rust
// in `auth.rs` and does not need a sidecar binary.
const MHF_IEL_CLI_EXE: &[u8] = include_bytes!("../resources/mhf-iel-cli.exe");

/// Write the bundled `mhf-iel-cli.exe` into `dest`. Skips the write if the
/// file is already present and byte-identical.
pub fn extract_launcher(dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest).with_context(|| format!("cannot create '{}'", dest.display()))?;

    let out = dest.join("mhf-iel-cli.exe");
    let up_to_date = std::fs::read(&out)
        .map(|existing| existing == MHF_IEL_CLI_EXE)
        .unwrap_or(false);

    if up_to_date {
        println!("  mhf-iel-cli.exe already up to date");
    } else {
        std::fs::write(&out, MHF_IEL_CLI_EXE)
            .with_context(|| format!("cannot write '{}'", out.display()))?;
        println!("  ✓ wrote mhf-iel-cli.exe ({} bytes)", MHF_IEL_CLI_EXE.len());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&out)?.permissions();
            perms.set_mode(0o755);
            let _ = std::fs::set_permissions(&out, perms);
        }
    }

    println!("\nAuthenticate via the launcher UI to generate config.json, then launch.");
    Ok(())
}

// ── Launch ────────────────────────────────────────────────────────────────────

/// Launch the game. If `auth_first` is true, or config.json is missing/stale,
/// re-authenticate via the in-app auth flow before launching.
pub fn launch(game_dir: &Path, auth_first: bool) -> Result<()> {
    let config_path = game_dir.join("config.json");
    let cli_exe = game_dir.join("mhf-iel-cli.exe");

    // Authentication is now handled by mhf-outpost itself (auth module).
    // If auth is needed, the caller should run auth::authenticate() first.
    if auth_first || !config_path.exists() || token_expired(&config_path) {
        bail!(
            "config.json is missing or has no valid token.\n\
             Use the launcher UI to authenticate, or run:\n  \
             mhf-outpost launch --path {} --auth",
            game_dir.display()
        );
    }

    if !cli_exe.exists() {
        bail!(
            "mhf-iel-cli.exe not found in '{}'\n\
             Run: mhf-outpost extract-launcher --path {}",
            game_dir.display(),
            game_dir.display()
        );
    }

    println!("Launching MHF…");
    let status = platform_exec(&cli_exe, game_dir)?;
    if !status.success() {
        bail!("mhf-iel-cli exited with {:?}", status.code());
    }
    Ok(())
}

/// True if config.json has an expired session token.
fn token_expired(config_path: &Path) -> bool {
    let Ok(src) = std::fs::read_to_string(config_path) else {
        return true;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&src) else {
        return true;
    };
    let token = v.get("user_token").and_then(|t| t.as_str()).unwrap_or("");
    // A valid token is exactly 16 characters; empty/short means not authenticated yet.
    token.len() != 16
}

/// Run a Windows .exe: natively on Windows, via Wine on Linux.
fn platform_exec(exe: &Path, cwd: &Path) -> Result<std::process::ExitStatus> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new(exe)
            .current_dir(cwd)
            .status()
            .with_context(|| format!("failed to run '{}'", exe.display()))
    }

    #[cfg(not(target_os = "windows"))]
    {
        // Try wine, then wine64.
        for bin in ["wine", "wine64"] {
            if let Ok(status) = std::process::Command::new(bin)
                .arg(exe)
                .current_dir(cwd)
                .status()
            {
                return Ok(status);
            }
        }
        bail!(
            "wine not found — install Wine to run MHF on Linux\n\
             Run `mhf-outpost check` for details"
        )
    }
}

// ── Windows Defender exclusion ────────────────────────────────────────────────

/// Add the game directory to Windows Defender exclusions.
/// No-op and returns Ok on non-Windows platforms.
pub fn av_exclude(game_dir: &Path) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        av_exclude_windows(game_dir)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = game_dir;
        println!("AV exclusion is only applicable on Windows.");
        Ok(())
    }
}

#[cfg(target_os = "windows")]
fn av_exclude_windows(game_dir: &Path) -> Result<()> {
    let path_str = game_dir
        .canonicalize()
        .unwrap_or_else(|_| game_dir.to_path_buf())
        .to_string_lossy()
        .to_string();

    println!("Adding Windows Defender exclusion for: {path_str}");

    // Try directly first (works if already admin).
    let direct = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!("Add-MpPreference -ExclusionPath '{path_str}'"),
        ])
        .status();

    match direct {
        Ok(s) if s.success() => {
            println!("✓ Exclusion added");
            return Ok(());
        }
        _ => {}
    }

    // Not admin — relaunch the PowerShell command elevated via Start-Process.
    println!("Requesting administrator elevation…");
    let ps_cmd = format!(
        "Start-Process powershell -Verb RunAs -ArgumentList \
         '-NoProfile -NonInteractive -Command \
         Add-MpPreference -ExclusionPath ''''{path_str}''''; \
         Read-Host ''Press Enter to close'''"
    );
    let status = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps_cmd])
        .status()
        .context("failed to launch elevated PowerShell")?;

    if status.success() {
        println!("✓ Elevation requested — accept the UAC prompt to apply the exclusion");
    } else {
        bail!(
            "could not add Defender exclusion automatically\n\
             Run manually in an admin PowerShell:\n  \
             Add-MpPreference -ExclusionPath '{path_str}'"
        );
    }
    Ok(())
}
