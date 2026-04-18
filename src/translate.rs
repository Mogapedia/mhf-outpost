use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use indicatif::{ProgressBar, ProgressStyle};
use serde::Deserialize;
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Default GitHub repository for MHFrontier translations (owner/repo).
pub const DEFAULT_REPO: &str = "mogapedia/MHFrontier-Translation";

/// Release asset filename template for per-language gzipped launcher payloads
/// (MHFrontier-Translation v0.2.0+). Substitute `{lang}` with the ISO code.
fn translated_asset_name(lang: &str) -> String {
    format!("translations-{lang}.json.gz")
}

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
    /// Game root directory (contains `dat/`, `mhf.exe`, …).
    pub dest: std::path::PathBuf,
    /// Language code to apply (e.g. "fr", "en").
    pub lang: String,
    /// GitHub repository slug (e.g. "mogapedia/MHFrontier-Translation").
    pub repo: String,
}

/// Download the latest per-language translation payload from GitHub and apply
/// it to the game directory.
///
/// Since MHFrontier-Translation v0.2.0 the release assets are per-language
/// gzipped JSON files (`translations-{lang}.json.gz`) that contain only the
/// translated strings — no original game data. The payload uses the index-
/// keyed format with `{j}` join markers and `{cNN}`/`{/c}` color codes.
/// Applying the patch requires the user's own game files.
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

    // Locate the per-language gzipped payload (v0.2.0 asset naming).
    let asset_name = translated_asset_name(&opts.lang);
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == asset_name)
        .ok_or_else(|| {
            let available = release
                .assets
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            anyhow::anyhow!(
                "asset '{}' not found in release {} of {}\nAvailable: {}",
                asset_name,
                release.tag_name,
                opts.repo,
                available,
            )
        })?;

    std::fs::create_dir_all(&opts.dest)
        .with_context(|| format!("cannot create '{}'", opts.dest.display()))?;

    let gz_path = opts.dest.join(&asset_name);
    download_asset(&client, asset, &gz_path)?;

    // Decompress the gzipped payload into the parallel .json file.
    let json_path = opts.dest.join(format!("translations-{}.json", opts.lang));
    decompress_gzip(&gz_path, &json_path)
        .with_context(|| format!("failed to decompress {}", gz_path.display()))?;
    println!("  Decompressed to {}", json_path.display());

    println!("\nApplying translations…");
    let results = crate::patch::apply_translations(&json_path, &opts.lang, &opts.dest, true, true)?;

    for r in &results {
        println!("  ✓ {} — {} string(s) patched", r.file, r.count);
    }
    if results.is_empty() {
        println!("  No translations found for language '{}'.", opts.lang);
    } else {
        let total: usize = results.iter().map(|r| r.count).sum();
        println!(
            "\nDone — {} string(s) applied to {} file(s).",
            total,
            results.len()
        );
    }

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
    println!(
        "Client mode: {} (manifest ID: {})",
        info.client_mode, info.manifest_id
    );

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

/// Stream-decompress a gzipped file to a plain file next to it.
fn decompress_gzip(src: &Path, dest: &Path) -> Result<()> {
    let input =
        std::fs::File::open(src).with_context(|| format!("cannot open {}", src.display()))?;
    let mut decoder = GzDecoder::new(input);
    let mut output =
        std::fs::File::create(dest).with_context(|| format!("cannot create {}", dest.display()))?;
    std::io::copy(&mut decoder, &mut output)?;
    Ok(())
}
