use crate::manifest::{ArchiveSource, Manifest};
use crate::verify;
use anyhow::{bail, Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const CHUNK: usize = 64 * 1024; // 64 KiB
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Progress callback invoked during archive download: `(bytes_done, bytes_total)`.
/// When set, the in-process indicatif bar is suppressed so only the callback drives
/// progress reporting (used by the Tauri GUI layer).
pub type ProgressCallback = Arc<dyn Fn(u64, u64) + Send + Sync>;

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
    /// Optional progress callback invoked every ~64 KiB during the download phase.
    /// When `Some`, the indicatif progress bar is suppressed on stdout.
    pub on_progress: Option<ProgressCallback>,
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

    let archive_path = opts
        .archive_path
        .clone()
        .unwrap_or_else(|| opts.dest.join(&archive.filename));

    check_dest_safe(&opts.dest, &archive_path)?;

    std::fs::create_dir_all(&opts.dest)
        .with_context(|| format!("cannot create '{}'", opts.dest.display()))?;

    // ── 1. Download ───────────────────────────────────────────────────────────
    download_file(archive, &archive_path, opts.on_progress.as_ref())?;

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

// ── Destination safety check ─────────────────────────────────────────────────

/// Refuse to install into a directory we don't recognise. Allowed shapes:
///
/// 1. Non-existent or empty directory — fresh install.
/// 2. Directory holding an existing MHF install (an `mhf.exe` is present at
///    the root or one level below). Re-installing on top of itself is fine
///    because the archive contains the same files.
/// 3. Directory whose only contents are the in-progress archive (`<filename>`
///    or `<filename>.part`) — happens when a previous run downloaded the
///    archive but was interrupted before extraction.
///
/// Anything else (the user picked `~/Documents`, their desktop, an unrelated
/// project folder…) is rejected with a message that names the offending entry
/// so they understand what we're protecting them from.
fn check_dest_safe(dest: &Path, archive_path: &Path) -> Result<()> {
    let entries = match std::fs::read_dir(dest) {
        Ok(it) => it,
        Err(_) => return Ok(()), // dest doesn't exist yet — we'll create it
    };

    let archive_name = archive_path.file_name();
    let mut foreign: Option<String> = None;
    let mut has_mhf_exe = false;

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy().to_string();

        if name_str.eq_ignore_ascii_case("mhf.exe") {
            has_mhf_exe = true;
            continue;
        }
        // One-level-down check (extracted archives often have a top-level dir).
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false)
            && entry.path().join("mhf.exe").exists()
        {
            has_mhf_exe = true;
            continue;
        }
        // The archive itself, or a partial download next to it.
        if let Some(an) = archive_name {
            if name == an || name_str == format!("{}.part", an.to_string_lossy()) {
                continue;
            }
        }
        if foreign.is_none() {
            foreign = Some(name_str);
        }
    }

    if has_mhf_exe || foreign.is_none() {
        return Ok(());
    }

    bail!(
        "install folder '{}' is not empty (contains '{}' and possibly other files).\n\
         Refusing to extract on top of unknown contents.\n\
         Pick an empty folder, or delete the existing contents first.",
        dest.display(),
        foreign.unwrap()
    );
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

fn download_file(
    archive: &ArchiveSource,
    dest: &Path,
    on_progress: Option<&ProgressCallback>,
) -> Result<()> {
    let existing = dest.metadata().map(|m| m.len()).unwrap_or(0);

    if existing == archive.size {
        println!("Archive already present and correct size — skipping download.");
        if let Some(cb) = on_progress {
            cb(archive.size, archive.size);
        }
        return Ok(());
    }

    let client = reqwest::blocking::Client::builder()
        .user_agent("mhf-outpost/0.1")
        .connect_timeout(CONNECT_TIMEOUT)
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

    // Use indicatif only when no external callback drives progress reporting.
    let pb = if on_progress.is_none() {
        let bar = ProgressBar::new(total);
        bar.set_style(
            ProgressStyle::with_template(
                "{spinner:.cyan} [{bar:40.cyan/blue}] {bytes}/{total_bytes} \
                 ({binary_bytes_per_sec}, {eta})",
            )
            .unwrap()
            .progress_chars("=>-"),
        );
        bar.set_position(existing);
        Some(bar)
    } else {
        None
    };

    let mut file = if resumed {
        OpenOptions::new()
            .append(true)
            .open(dest)
            .with_context(|| format!("cannot open '{}'", dest.display()))?
    } else {
        File::create(dest).with_context(|| format!("cannot create '{}'", dest.display()))?
    };

    let mut bytes_done = existing;
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        bytes_done += n as u64;
        if let Some(bar) = &pb {
            bar.inc(n as u64);
        }
        if let Some(cb) = on_progress {
            cb(bytes_done, total);
        }
    }
    if let Some(bar) = pb {
        bar.finish_and_clear();
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_dir(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "mhf_outpost_dest_{tag}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::File::create(path).unwrap();
    }

    #[test]
    fn safe_when_dest_missing() {
        let dest = fresh_dir("missing");
        assert!(check_dest_safe(&dest, &dest.join("mhfo.7z")).is_ok());
    }

    #[test]
    fn safe_when_dest_empty() {
        let dest = fresh_dir("empty");
        std::fs::create_dir_all(&dest).unwrap();
        assert!(check_dest_safe(&dest, &dest.join("mhfo.7z")).is_ok());
        std::fs::remove_dir_all(&dest).ok();
    }

    #[test]
    fn safe_when_existing_install_at_root() {
        let dest = fresh_dir("install_root");
        touch(&dest.join("mhf.exe"));
        touch(&dest.join("dat").join("foo.bin"));
        assert!(check_dest_safe(&dest, &dest.join("mhfo.7z")).is_ok());
        std::fs::remove_dir_all(&dest).ok();
    }

    #[test]
    fn safe_when_existing_install_in_subdir() {
        let dest = fresh_dir("install_sub");
        touch(&dest.join("MHFO").join("mhf.exe"));
        assert!(check_dest_safe(&dest, &dest.join("mhfo.7z")).is_ok());
        std::fs::remove_dir_all(&dest).ok();
    }

    #[test]
    fn safe_when_only_archive_present() {
        let dest = fresh_dir("archive_only");
        let archive = dest.join("mhfo.7z");
        touch(&archive);
        assert!(check_dest_safe(&dest, &archive).is_ok());
        std::fs::remove_dir_all(&dest).ok();
    }

    #[test]
    fn safe_when_partial_download_present() {
        let dest = fresh_dir("partial");
        let archive = dest.join("mhfo.7z");
        touch(&dest.join("mhfo.7z.part"));
        assert!(check_dest_safe(&dest, &archive).is_ok());
        std::fs::remove_dir_all(&dest).ok();
    }

    #[test]
    fn rejects_unrelated_files() {
        let dest = fresh_dir("foreign");
        touch(&dest.join("notes.txt"));
        let err = check_dest_safe(&dest, &dest.join("mhfo.7z")).unwrap_err();
        assert!(err.to_string().contains("notes.txt"));
        std::fs::remove_dir_all(&dest).ok();
    }
}
