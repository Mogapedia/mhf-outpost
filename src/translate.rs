use anyhow::{bail, Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use serde::Deserialize;
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Default GitHub repository for MHFrontier translations (owner/repo).
pub const DEFAULT_REPO: &str = "mogapedia/MHFrontier-Translation";

// ── GitHub API types ──────────────────────────────────────────────────────────

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

// ── Public entry point ────────────────────────────────────────────────────────

pub struct TranslateOptions {
    /// Game directory where translated files will be written.
    pub dest: std::path::PathBuf,
    /// Language code to fetch (e.g. "fr", "en").
    pub lang: String,
    /// GitHub repository slug (e.g. "mogapedia/MHFrontier-Translation").
    pub repo: String,
}

/// Fetch the latest translated game files from a GitHub release and apply
/// them to the game directory.
///
/// The release is expected to contain assets named `<lang>-<file>.bin`
/// (e.g. `fr-mhfdat.bin`, `fr-mhfpac.bin`). Each asset is written directly
/// into `dest`, replacing the stock file.
pub fn run(opts: TranslateOptions) -> Result<()> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("mhf-outpost/0.1")
        .timeout(REQUEST_TIMEOUT)
        .build()?;

    let api_url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        opts.repo.trim_matches('/')
    );

    println!("Fetching latest translation release from {}…", opts.repo);
    let release: Release = client
        .get(&api_url)
        .send()
        .with_context(|| format!("failed to reach GitHub API at {api_url}"))?
        .json()
        .context("failed to parse GitHub release JSON")?;

    println!("Release: {}", release.tag_name);

    // Filter to assets matching the requested language prefix.
    let prefix = format!("{}-", opts.lang);
    let matching: Vec<&Asset> = release
        .assets
        .iter()
        .filter(|a| a.name.starts_with(&prefix) && a.name.ends_with(".bin"))
        .collect();

    if matching.is_empty() {
        bail!(
            "no translated .bin files found for language '{}' in release {} of {}\n\
             Available assets: {}",
            opts.lang,
            release.tag_name,
            opts.repo,
            release
                .assets
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    std::fs::create_dir_all(&opts.dest)
        .with_context(|| format!("cannot create '{}'", opts.dest.display()))?;

    let mut applied = 0usize;
    for asset in &matching {
        // Strip the language prefix to get the target filename (e.g. "mhfdat.bin").
        let target_name = &asset.name[prefix.len()..];
        let dest_path = opts.dest.join(target_name);

        // Warn if the target file doesn't exist in the game directory —
        // the translation is still written so the user can place it manually.
        if !dest_path.exists() {
            println!(
                "  ⚠ {} not found in game directory — writing anyway",
                target_name
            );
        }

        download_asset(&client, asset, &dest_path)?;
        applied += 1;
    }

    println!(
        "\n✓ Applied {} translated file(s) from {} ({})",
        applied, opts.repo, release.tag_name
    );
    println!(
        "  Files modified in: {}",
        opts.dest.display()
    );
    Ok(())
}

// ── Server info ───────────────────────────────────────────────────────────────

/// Response from Erupe's GET /v2/server/info endpoint.
#[derive(Deserialize)]
pub struct ServerInfoResponse {
    #[serde(rename = "clientMode")]
    pub client_mode: String,
    #[serde(rename = "manifestId")]
    pub manifest_id: String,
    pub name: String,
}

/// Fetch server info from an Erupe instance and print a compatibility summary.
///
/// `local_version` is the mhf-outpost manifest ID the user has installed
/// (e.g. "zz", "gg"). Pass `None` to skip the compatibility check.
pub fn server_info(server: &str, local_version: Option<&str>) -> Result<()> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("mhf-outpost/0.1")
        .timeout(REQUEST_TIMEOUT)
        .build()?;

    let url = format!("{}/v2/server/info", server.trim_end_matches('/'));
    let resp = client
        .get(&url)
        .send()
        .with_context(|| format!("failed to connect to {url}"))?;

    if !resp.status().is_success() {
        bail!(
            "server returned {}: {}",
            resp.status(),
            resp.text().unwrap_or_default()
        );
    }

    let info: ServerInfoResponse = resp
        .json()
        .context("failed to parse server info response")?;

    println!("Server:      {}", server);
    println!("Software:    {}", info.name);
    println!("Client mode: {} (manifest ID: {})", info.client_mode, info.manifest_id);

    if let Some(local) = local_version {
        if local.to_ascii_lowercase() == info.manifest_id {
            println!("Compatibility: ✓ your game version ({local}) matches the server");
        } else {
            println!(
                "Compatibility: ⚠ server requires '{}', but you specified '{local}'\n\
                 Run `mhf-outpost download --version {}` to get the correct version.",
                info.manifest_id, info.manifest_id
            );
        }
    }

    Ok(())
}

// ── HTTP download helper ───────────────────────────────────────────────────────

fn download_asset(client: &reqwest::blocking::Client, asset: &Asset, dest: &Path) -> Result<()> {
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
    println!("  ✓ {} → {}", asset.name, dest.display());
    Ok(())
}
