use anyhow::{bail, Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use serde::Deserialize;
use std::io::Write;
use std::path::{Path, PathBuf};

/// We maintain mhf-iel-cli ourselves; only the CLI launcher binary is needed.
/// Authentication (previously mhf-iel-auth) is now built into mhf-outpost.
const GITHUB_API: &str = "https://api.github.com/repos/rockisch/mhf-iel/releases/latest";

// ── GitHub release fetching ───────────────────────────────────────────────────

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
}

/// Download mhf-iel-cli.exe and mhf-iel-auth.exe into `dest`.
pub fn fetch_launcher(dest: &Path) -> Result<()> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("mhf-outpost/0.1")
        .build()?;

    println!("Fetching latest mhf-iel release from GitHub…");
    let release: Release = client
        .get(GITHUB_API)
        .send()
        .context("failed to reach GitHub API")?
        .json()
        .context("failed to parse GitHub release JSON")?;

    println!("Latest release: {}", release.tag_name);

    // Only the CLI launcher is needed; authentication is now built into mhf-outpost.
    let wanted = ["mhf-iel-cli.exe"];
    let mut found = 0u32;

    std::fs::create_dir_all(dest)
        .with_context(|| format!("cannot create '{}'", dest.display()))?;

    for name in wanted {
        let asset = release.assets.iter().find(|a| a.name == name);
        match asset {
            None => println!("  ⚠ {name} not found in release assets — skipping"),
            Some(a) => {
                let out = dest.join(name);
                download_asset(&client, a, &out)?;
                found += 1;
            }
        }
    }

    if found == 0 {
        bail!("mhf-iel-cli.exe not found in release {}", release.tag_name);
    }

    println!("\nPlace mhf-iel-cli.exe in your MHF game folder, then authenticate via the launcher UI.");
    Ok(())
}

fn download_asset(
    client: &reqwest::blocking::Client,
    asset: &Asset,
    dest: &Path,
) -> Result<()> {
    use std::io::Read;

    // Skip if already present and correct size.
    if dest.metadata().map(|m| m.len()).unwrap_or(0) == asset.size {
        println!("  {} already up to date", asset.name);
        return Ok(());
    }

    let pb = ProgressBar::new(asset.size);
    pb.set_style(
        ProgressStyle::with_template(&format!(
            "  {{spinner:.cyan}} {{bar:30.cyan/blue}} {{bytes}}/{{total_bytes}}  {}",
            asset.name
        ))
        .unwrap()
        .progress_chars("=>-"),
    );

    let mut resp = client
        .get(&asset.browser_download_url)
        .send()
        .with_context(|| format!("failed to download {}", asset.name))?;

    let mut file = std::fs::File::create(dest)
        .with_context(|| format!("cannot create '{}'", dest.display()))?;

    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        pb.inc(n as u64);
    }
    pb.finish_and_clear();
    println!("  ✓ {}", asset.name);
    Ok(())
}

// ── Launch ────────────────────────────────────────────────────────────────────

/// Launch the game. If `auth_first` is true, or config.json is missing/stale,
/// run mhf-iel-auth to (re-)authenticate before launching.
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
             Run: mhf-outpost fetch-launcher --path {}",
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

// ── helpers ───────────────────────────────────────────────────────────────────

pub fn launcher_paths(game_dir: &Path) -> (PathBuf, PathBuf) {
    (
        game_dir.join("mhf-iel-cli.exe"),
        game_dir.join("mhf-iel-auth.exe"),
    )
}
