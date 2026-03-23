use crate::manifest::{ArchiveSource, Manifest};
use crate::verify;
use anyhow::{bail, Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

const CHUNK: usize = 64 * 1024; // 64 KiB

// ── Public entry point ────────────────────────────────────────────────────────

pub struct DownloadOptions {
    /// Destination directory for the extracted game files.
    pub dest: PathBuf,
    /// Where to store the downloaded archive (default: dest / filename).
    pub archive_path: Option<PathBuf>,
    /// Skip the copyright disclaimer prompt.
    pub yes: bool,
    /// Keep the archive after successful extraction.
    pub keep_archive: bool,
}

pub fn run(manifest: &Manifest, opts: DownloadOptions) -> Result<()> {
    let archive = manifest.archive.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "no archive.org source recorded for '{}' — \
             obtain the files manually and use `verify` to check them",
            manifest.version.id
        )
    })?;

    if !opts.yes {
        prompt_disclaimer(&manifest.version.name)?;
    }

    std::fs::create_dir_all(&opts.dest)
        .with_context(|| format!("cannot create '{}'", opts.dest.display()))?;

    let archive_path = opts
        .archive_path
        .unwrap_or_else(|| opts.dest.join(&archive.filename));

    // ── 1. Download ───────────────────────────────────────────────────────────
    download_file(archive, &archive_path)?;

    // ── 2. Verify archive integrity ───────────────────────────────────────────
    println!("\nVerifying archive integrity…");
    let check = verify::verify_archive(archive, &archive_path)?;
    if !check.size_ok() {
        bail!(
            "size mismatch after download (expected {} B, got {} B)",
            check.expected_size,
            check.actual_size
        );
    }
    if check.sha1_ok() {
        println!("✓ SHA-1 OK");
    } else {
        bail!(
            "SHA-1 mismatch — archive may be corrupted\n  expected: {}\n  actual:   {}",
            check.expected_sha1,
            check.actual_sha1
        );
    }

    // ── 3. Extract ────────────────────────────────────────────────────────────
    println!("\nExtracting to {}…", opts.dest.display());
    let count = extract(archive, &archive_path, &opts.dest)?;
    println!("✓ Extracted {count} file(s)");

    if !opts.keep_archive {
        let _ = std::fs::remove_file(&archive_path);
    }

    // Handle double-wrapped archives (e.g. ZIP containing a 7z).
    maybe_extract_inner(&opts.dest)?;

    println!(
        "\nDone. Run `mhf-outpost verify --version {} --path {}` to confirm.",
        manifest.version.id.to_ascii_lowercase(),
        opts.dest.display()
    );
    Ok(())
}

// ── Disclaimer ────────────────────────────────────────────────────────────────

fn prompt_disclaimer(name: &str) -> Result<()> {
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  Copyright notice");
    println!();
    println!("  {name} is copyrighted software © Capcom Co., Ltd.");
    println!("  Monster Hunter Frontier Online was shut down on 2019-12-18 and");
    println!("  is no longer commercially available from any official source.");
    println!();
    println!("  This download is provided solely for game preservation.");
    println!("  You are responsible for compliance with the laws of your country.");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();
    print!("  Continue? [y/N] ");
    std::io::stdout().flush()?;

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    if input.trim().eq_ignore_ascii_case("y") {
        Ok(())
    } else {
        bail!("download cancelled");
    }
}

// ── HTTP download (with resume) ───────────────────────────────────────────────

fn download_file(archive: &ArchiveSource, dest: &Path) -> Result<()> {
    let existing = dest.metadata().map(|m| m.len()).unwrap_or(0);

    if existing == archive.size {
        println!("Archive already present and correct size — skipping download.");
        return Ok(());
    }

    let client = reqwest::blocking::Client::builder()
        .user_agent("mhf-outpost/0.1")
        .build()?;

    let mut req = client.get(archive.download_url());
    if existing > 0 {
        println!("Resuming download from {} B…", existing);
        req = req.header("Range", format!("bytes={existing}-"));
    }

    let mut resp = req
        .send()
        .with_context(|| format!("failed to connect to {}", archive.download_url()))?;

    let status = resp.status();
    // 200 OK = fresh download, 206 Partial Content = resume accepted
    if !status.is_success() {
        bail!("server returned {status}");
    }
    let resumed = status == reqwest::StatusCode::PARTIAL_CONTENT;

    let content_len = resp.content_length().unwrap_or(0);
    let total = if resumed {
        existing + content_len
    } else {
        content_len.max(archive.size)
    };

    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.cyan} [{bar:40.cyan/blue}] {bytes}/{total_bytes} \
             ({binary_bytes_per_sec}, {eta})",
        )
        .unwrap()
        .progress_chars("=>-"),
    );
    pb.set_position(existing);

    let mut file = if resumed {
        OpenOptions::new()
            .append(true)
            .open(dest)
            .with_context(|| format!("cannot open '{}'", dest.display()))?
    } else {
        File::create(dest).with_context(|| format!("cannot create '{}'", dest.display()))?
    };

    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        pb.inc(n as u64);
    }
    pb.finish_and_clear();
    println!("✓ Download complete ({})", dest.display());
    Ok(())
}

// ── Extraction ────────────────────────────────────────────────────────────────

fn extract(archive: &ArchiveSource, src: &Path, dest: &Path) -> Result<usize> {
    match archive.format.as_str() {
        "ZIP" => extract_zip(src, dest),
        "RAR" => extract_with_tool(src, dest, &["unrar", "x", "-o+"], "unrar"),
        "7z" => extract_7z(src, dest),
        fmt => bail!("unsupported archive format '{fmt}' — extract manually"),
    }
}

/// ZIP extraction via the `zip` crate (pure Rust, no system tool needed).
fn extract_zip(src: &Path, dest: &Path) -> Result<usize> {
    let file = File::open(src).with_context(|| format!("cannot open '{}'", src.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("cannot read ZIP '{}'", src.display()))?;

    let total = archive.len();
    let pb = ProgressBar::new(total as u64);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.cyan} [{bar:40.cyan/blue}] {pos}/{len} files  {wide_msg}",
        )
        .unwrap()
        .progress_chars("=>-"),
    );

    // Detect common root directory inside the ZIP so we can strip it.
    // If every entry starts with the same top-level component, strip it.
    let strip_prefix = common_zip_prefix(&mut archive);

    let mut extracted = 0usize;
    for i in 0..total {
        let mut entry = archive.by_index(i)?;
        let raw_name = entry.name().to_string();
        pb.set_message(raw_name.clone());

        let rel = match &strip_prefix {
            Some(prefix) => Path::new(&raw_name)
                .strip_prefix(prefix)
                .unwrap_or(Path::new(&raw_name))
                .to_path_buf(),
            None => PathBuf::from(&raw_name),
        };

        // Skip the stripped root itself (empty path = the dest dir).
        if rel.as_os_str().is_empty() {
            pb.inc(1);
            continue;
        }

        let out = safe_join(dest, &rel)?;

        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
        } else {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut f =
                File::create(&out).with_context(|| format!("cannot create '{}'", out.display()))?;
            std::io::copy(&mut entry, &mut f)?;
            extracted += 1;
        }
        pb.inc(1);
    }
    pb.finish_and_clear();
    Ok(extracted)
}

/// Detect if all ZIP entries share a single top-level *directory* prefix.
/// Returns None if any entry is a root-level file (no '/' in name).
fn common_zip_prefix(archive: &mut zip::ZipArchive<File>) -> Option<String> {
    let mut prefix: Option<String> = None;
    for i in 0..archive.len() {
        let entry = archive.by_index(i).ok()?;
        let name = entry.name();
        // A root-level file has no '/' — no prefix to strip.
        if !name.contains('/') {
            return None;
        }
        let top = name.split('/').next()?;
        if top.is_empty() {
            return None;
        }
        match &prefix {
            None => prefix = Some(top.to_string()),
            Some(p) if p != top => return None,
            _ => {}
        }
    }
    prefix
}

/// If the ZIP extracted to a single file that is itself an archive,
/// extract that inner archive too and remove the intermediate file.
fn maybe_extract_inner(outer_dest: &Path) -> Result<()> {
    let entries: Vec<_> = std::fs::read_dir(outer_dest)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .collect();

    if entries.len() != 1 {
        return Ok(()); // multiple files or none — nothing to do
    }

    let inner_path = entries[0].path();
    let ext = inner_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    let inner_format = match ext.as_str() {
        "7z" => "7z",
        "rar" => "RAR",
        "zip" => "ZIP",
        _ => return Ok(()), // not a known archive, leave as-is
    };

    println!(
        "Inner archive detected ({inner_format}): {}",
        inner_path.display()
    );
    println!("Extracting inner archive…");

    let fake_src = crate::manifest::ArchiveSource {
        identifier: String::new(),
        filename: String::new(),
        format: inner_format.to_string(),
        size: 0,
        sha1: String::new(),
        md5: String::new(),
    };

    let count = extract(&fake_src, &inner_path, outer_dest)?;
    println!("✓ Extracted {count} file(s) from inner archive");
    std::fs::remove_file(&inner_path)?;
    Ok(())
}

/// RAR: shell to `unrar x -o+ <src> <dest>/`
fn extract_with_tool(src: &Path, dest: &Path, args_prefix: &[&str], tool: &str) -> Result<usize> {
    let mut cmd = std::process::Command::new(args_prefix[0]);
    for arg in &args_prefix[1..] {
        cmd.arg(arg);
    }
    cmd.arg(src);
    // unrar wants a trailing slash on the destination
    cmd.arg(format!("{}/", dest.display()));

    let status = cmd
        .status()
        .with_context(|| format!("failed to run `{tool}` — is it installed?"))?;

    if !status.success() {
        bail!("`{tool}` exited with {:?}", status.code());
    }
    // Count extracted files (best-effort)
    Ok(count_files(dest))
}

/// 7z: try `7z`, `7za`, `7zz` in order.
fn extract_7z(src: &Path, dest: &Path) -> Result<usize> {
    let dest_flag = format!("-o{}", dest.display());
    for bin in ["7z", "7za", "7zz"] {
        let result = std::process::Command::new(bin)
            .args(["x", &dest_flag, "-y"])
            .arg(src)
            .status();
        match result {
            Ok(s) if s.success() => return Ok(count_files(dest)),
            Ok(s) => bail!("`{bin}` exited with {:?}", s.code()),
            Err(_) => continue, // binary not found, try next
        }
    }
    bail!(
        "7z extractor not found — install p7zip:\n  \
         Fedora: sudo dnf install p7zip\n  \
         Ubuntu: sudo apt install p7zip-full\n  \
         Arch:   sudo pacman -S p7zip"
    )
}

fn count_files(dir: &Path) -> usize {
    walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .count()
}

// ── Path sanitisation (zip-slip prevention) ───────────────────────────────────

/// Join `base` and `rel`, rejecting any path that would escape `base`.
fn safe_join(base: &Path, rel: &Path) -> Result<PathBuf> {
    let mut out = base.to_path_buf();
    for component in rel.components() {
        match component {
            Component::Normal(c) => out.push(c),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!("unsafe path in archive: '{}'", rel.display());
            }
        }
    }
    Ok(out)
}
