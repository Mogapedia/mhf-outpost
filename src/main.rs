mod check;
mod download;
mod launcher;
mod manifest;
mod verify;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use manifest::FileKind;
use manifest::Manifest;
use std::path::{Path, PathBuf};
use verify::{FileStatus, VerifyReport};

#[derive(Parser)]
#[command(
    name = "mhf-outpost",
    about = "MHF game file verifier and installer helper",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check system prerequisites (DirectX 9, Japanese fonts, Wine/DXVK on Linux).
    Check {
        /// Also check an existing game installation directory.
        #[arg(short, long, value_name = "GAME_DIR")]
        path: Option<PathBuf>,
    },

    /// List all known game versions and their manifest status.
    List,

    /// Download mhf-iel-cli.exe and mhf-iel-auth.exe from the latest GitHub release.
    FetchLauncher {
        /// Game directory to place the launcher binaries in.
        #[arg(short, long)]
        path: PathBuf,
    },

    /// Launch the game via mhf-iel-cli.exe (Wine on Linux).
    ///
    /// If config.json is missing or the session token is absent, runs
    /// mhf-iel-auth first to authenticate and generate the config.
    Launch {
        /// Game directory containing mhf-iel-cli.exe and config.json.
        #[arg(short, long)]
        path: PathBuf,

        /// Force running mhf-iel-auth before launching (re-authenticate).
        #[arg(long)]
        auth: bool,
    },

    /// Add the game directory to Windows Defender exclusions (Windows only).
    ///
    /// mhf.exe is detected as malware by most AV software. Run this once
    /// after extracting the game files, before the first launch.
    AvExclude {
        /// Game directory to exclude.
        #[arg(short, long)]
        path: PathBuf,
    },

    /// Download and install a game version from archive.org.
    Download {
        /// Version to download (e.g. gg, g10, f4). Case-insensitive.
        #[arg(short, long)]
        version: String,

        /// Directory to extract the game files into.
        /// Defaults to `./<VERSION_ID>` in the current directory.
        #[arg(short, long)]
        path: Option<PathBuf>,

        /// Where to save the downloaded archive file.
        /// Defaults to <path>/<filename>.
        #[arg(long, value_name = "FILE")]
        archive: Option<PathBuf>,

        /// Skip the copyright disclaimer prompt.
        #[arg(short, long)]
        yes: bool,

        /// Keep the archive file after extraction.
        #[arg(long)]
        keep_archive: bool,
    },

    /// Print download URLs and torrent link for a version.
    Info {
        /// Version identifier (e.g. zz, gg, g10). Case-insensitive.
        #[arg(short, long)]
        version: String,
    },

    /// Verify a downloaded archive (.zip/.rar/.7z) against archive.org SHA-1.
    ///
    /// Run this before extracting to confirm the file is unmodified.
    VerifyArchive {
        /// Path to the downloaded archive file.
        path: PathBuf,

        /// Version the archive belongs to (e.g. gg, g10).
        #[arg(short, long)]
        version: String,
    },

    /// Verify an extracted game installation against a version manifest.
    Verify {
        /// Version to verify (e.g. zz, gg, f4). Case-insensitive.
        #[arg(short, long)]
        version: String,

        /// Path to the game directory. Defaults to current directory.
        #[arg(short, long)]
        path: Option<PathBuf>,

        /// Load a custom manifest TOML file instead of the embedded one.
        #[arg(long)]
        manifest: Option<PathBuf>,

        /// Show OK files too, not just problems.
        #[arg(long)]
        verbose: bool,

        /// Fail on any hash mismatch, including url/translation/config files.
        #[arg(long)]
        strict: bool,
    },

    /// Compute the SHA-256 and SHA-1 of a single file.
    Hash { path: PathBuf },

    /// Walk a directory and print manifest [[files]] entries for all files.
    ///
    /// Use this to generate or update a version manifest from an existing install.
    HashDir {
        /// Root of the game directory.
        path: PathBuf,

        /// Relative path prefixes to exclude (repeatable).
        #[arg(long = "exclude", value_name = "PREFIX")]
        exclude: Vec<String>,

        /// Emit entries as TOML [[files]] blocks (default: table format).
        #[arg(long)]
        toml: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Check { path } => cmd_check(path.as_deref()),
        Command::FetchLauncher { path } => launcher::fetch_launcher(&path),
        Command::Launch { path, auth } => launcher::launch(&path, auth),
        Command::AvExclude { path } => launcher::av_exclude(&path),
        Command::Download {
            version,
            path,
            archive,
            yes,
            keep_archive,
        } => cmd_download(&version, path, archive, yes, keep_archive),
        Command::List => cmd_list(),
        Command::Info { version } => cmd_info(&version),
        Command::VerifyArchive { path, version } => cmd_verify_archive(&path, &version),
        Command::Verify {
            version,
            path,
            manifest,
            verbose,
            strict,
        } => cmd_verify(
            &version,
            path.as_deref(),
            manifest.as_deref(),
            verbose,
            strict,
        ),
        Command::Hash { path } => cmd_hash(&path),
        Command::HashDir {
            path,
            exclude,
            toml,
        } => cmd_hash_dir(&path, &exclude, toml),
    }
}

// ── check ─────────────────────────────────────────────────────────────────────

fn cmd_check(game_path: Option<&Path>) -> Result<()> {
    let os = std::env::consts::OS;
    println!("System check ({os})\n");

    let mut all = check::system_checks();

    if let Some(path) = game_path {
        println!("Game directory: {}\n", path.display());
        all.extend(check::game_dir_checks(path));
    }

    let mut errors = 0usize;
    let mut warnings = 0usize;

    for c in &all {
        let (icon, label) = match c.status {
            check::Status::Ok => ("✓", "OK     "),
            check::Status::Warning => ("⚠", "WARN   "),
            check::Status::Error => ("✗", "ERROR  "),
        };
        println!("  {icon} {label} {}: {}", c.name, c.detail);
        if let Some(fix) = &c.fix {
            // Indent each line of the fix text
            for line in fix.lines() {
                println!("           → {line}");
            }
        }
        match c.status {
            check::Status::Warning => warnings += 1,
            check::Status::Error => errors += 1,
            _ => {}
        }
    }

    println!();
    if errors == 0 && warnings == 0 {
        println!("All checks passed.");
        Ok(())
    } else {
        if errors > 0 {
            bail!(
                "{errors} error(s), {warnings} warning(s) — fix the errors above before launching"
            );
        } else {
            println!("{warnings} warning(s) — review the recommendations above");
            Ok(())
        }
    }
}

// ── download ──────────────────────────────────────────────────────────────────

fn cmd_download(
    version: &str,
    path: Option<PathBuf>,
    archive: Option<PathBuf>,
    yes: bool,
    keep_archive: bool,
) -> Result<()> {
    let manifest = Manifest::load(version)?;

    // Run system + extractor checks upfront; warn but don't block.
    let sys = check::system_checks();
    let errors: Vec<_> = sys
        .iter()
        .filter(|c| c.status == check::Status::Error)
        .collect();
    if !errors.is_empty() {
        println!("⚠  System issues detected (run `mhf-outpost check` for details):");
        for e in &errors {
            println!("   ✗ {}: {}", e.name, e.detail);
        }
        println!();
    }

    if let Some(archive_src) = &manifest.archive {
        let ext_checks = check::extractor_checks(&archive_src.format);
        let ext_errors: Vec<_> = ext_checks
            .iter()
            .filter(|c| c.status == check::Status::Error)
            .collect();
        if !ext_errors.is_empty() {
            for e in &ext_errors {
                println!("✗ {}: {}", e.name, e.detail);
                if let Some(fix) = &e.fix {
                    for line in fix.lines() {
                        println!("  → {line}");
                    }
                }
            }
            bail!("install required extractor tools before downloading");
        }
    }

    let dest = path.unwrap_or_else(|| PathBuf::from(manifest.version.id.to_ascii_lowercase()));

    download::run(
        &manifest,
        download::DownloadOptions {
            dest,
            archive_path: archive,
            yes,
            keep_archive,
        },
    )
}

// ── list ─────────────────────────────────────────────────────────────────────

fn cmd_list() -> Result<()> {
    let manifests = Manifest::all();
    println!(
        "{:<6} {:<12} {:<8} {:<8} Description",
        "ID", "Name", "Platform", "Archive"
    );
    println!("{}", "-".repeat(80));
    for m in &manifests {
        let archive_status = match &m.archive {
            Some(a) => a.identifier.clone(),
            None => "-".to_string(),
        };
        let file_status = if m.files.is_empty() {
            String::new()
        } else {
            let r = m.recorded_count();
            let t = m.files.len();
            format!("  [{}/{} files hashed]", r, t)
        };
        println!(
            "{:<6} {:<12} {:<8} {:<8} {}{}",
            m.version.id,
            m.version.name,
            m.version.platform,
            archive_status,
            m.version.description,
            file_status,
        );
    }
    Ok(())
}

// ── info ─────────────────────────────────────────────────────────────────────

fn cmd_info(version: &str) -> Result<()> {
    let m = Manifest::load(version)?;
    println!("{} — {}", m.version.name, m.version.description);
    println!("Platform: {}", m.version.platform);
    println!();

    match &m.archive {
        None => println!("No archive.org source recorded for this version."),
        Some(a) => {
            let size_gb = a.size as f64 / 1_073_741_824.0;
            println!("Archive:  {} ({}, {:.2} GB)", a.filename, a.format, size_gb);
            println!("SHA-1:    {}", a.sha1);
            println!("MD5:      {}", a.md5);
            println!();
            println!("Page:     {}", a.item_url());
            println!("Direct:   {}", a.download_url());
            println!("Torrent:  {}", a.torrent_url());
        }
    }
    Ok(())
}

// ── verify-archive ────────────────────────────────────────────────────────────

fn cmd_verify_archive(path: &Path, version: &str) -> Result<()> {
    let m = Manifest::load(version)?;

    let archive = m.archive.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "version '{}' has no archive.org source recorded — cannot verify",
            version
        )
    })?;

    let size_gb = path.metadata().map(|m| m.len()).unwrap_or(0) as f64 / 1_073_741_824.0;
    println!(
        "Verifying archive for {} ({}) — {:.2} GB",
        m.version.name, m.version.id, size_gb
    );
    println!("File: {}", path.display());
    println!();

    let result = verify::verify_archive(archive, path)?;

    if !result.size_ok() {
        println!(
            "  SIZE MISMATCH  expected {} B, got {} B",
            result.expected_size, result.actual_size
        );
    }

    if result.sha1_ok() {
        println!("✓ SHA-1 OK  {}", result.actual_sha1);
        Ok(())
    } else {
        println!("  expected: {}", result.expected_sha1);
        println!("  actual:   {}", result.actual_sha1);
        bail!("✗ SHA-1 mismatch — archive may be corrupted or modified");
    }
}

// ── verify ────────────────────────────────────────────────────────────────────

fn cmd_verify(
    version: &str,
    path: Option<&Path>,
    manifest_path: Option<&Path>,
    verbose: bool,
    strict: bool,
) -> Result<()> {
    let manifest = match manifest_path {
        Some(p) => Manifest::load_file(p)?,
        None => Manifest::load(version)?,
    };

    if manifest.files.is_empty() {
        bail!(
            "manifest for '{}' has no [[files]] entries yet.\n\
             Run `mhf-outpost hash-dir <game_dir> --toml` to generate them.",
            version
        );
    }

    let root = path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().expect("cannot get current directory"));

    println!(
        "Verifying {} ({}) at {}",
        manifest.version.name,
        manifest.version.id,
        root.display()
    );
    println!("{} files in manifest", manifest.files.len());
    if manifest.recorded_count() < manifest.files.len() {
        println!(
            "  ⚠  {}/{} files have placeholder hashes — those will be skipped",
            manifest.files.len() - manifest.recorded_count(),
            manifest.files.len()
        );
    }
    println!();

    let report = verify::verify(&manifest, &root);
    print_report(&report, verbose);

    let failures = report.hard_failures().count();
    let modified = report.modified().count();

    println!();
    if failures == 0 && modified == 0 {
        println!(
            "✓ All files OK ({} verified, {} placeholders skipped)",
            report.ok_count() - report.placeholder_count(),
            report.placeholder_count()
        );
    } else if failures == 0 {
        println!(
            "✓ No tampering detected — {} file(s) modified (see above)",
            modified
        );
    }

    if failures > 0 {
        bail!("✗ {} tampered/missing file(s)", failures);
    }
    if strict && modified > 0 {
        bail!("✗ {} modified file(s) (--strict)", modified);
    }
    Ok(())
}

fn print_report(report: &VerifyReport, verbose: bool) {
    for r in &report.results {
        match &r.status {
            FileStatus::Ok => {
                if verbose {
                    println!("  OK             {}", r.path);
                }
            }
            FileStatus::Placeholder => {
                if verbose {
                    println!("  SKIP           {} (placeholder hash)", r.path);
                }
            }
            FileStatus::Missing if r.optional => {
                if verbose {
                    println!("  OPTIONAL       {} (not present)", r.path);
                }
            }
            FileStatus::Missing => println!("  MISSING        {}", r.path),
            FileStatus::SizeMismatch { expected, actual } => {
                println!(
                    "  SIZE MISMATCH  {}  (expected {} B, got {} B)",
                    r.path, expected, actual
                );
            }
            FileStatus::Modified { expected, actual } => {
                match r.kind {
                    FileKind::Core => {
                        println!(
                            "  TAMPERED       {}  [core file — unexpected modification]",
                            r.path
                        );
                        println!("    expected: {}", expected);
                        println!("    actual:   {}", actual);
                    }
                    FileKind::Url => {
                        // Expected — user pointed the client at a community server.
                        if verbose {
                            println!(
                                "  URL PATCHED    {}  [server URL customization — OK]",
                                r.path
                            );
                        }
                    }
                    FileKind::Translation => {
                        println!(
                            "  MODIFIED       {}  [data file — likely fan translation; \
                             may also contain game-value changes (modding)]",
                            r.path
                        );
                        if verbose {
                            println!("    expected: {}", expected);
                            println!("    actual:   {}", actual);
                        }
                    }
                    FileKind::Config => {
                        if verbose {
                            println!("  CONFIG         {}  [user configuration — OK]", r.path);
                        }
                    }
                }
            }
            FileStatus::Unreadable(e) => {
                println!("  UNREADABLE     {}  ({})", r.path, e);
            }
        }
    }
}

// ── hash ──────────────────────────────────────────────────────────────────────

fn cmd_hash(path: &Path) -> Result<()> {
    let size = path.metadata().map(|m| m.len()).unwrap_or(0);
    let sha256 = verify::hash_file(path)
        .map_err(|e| anyhow::anyhow!("failed to read '{}': {}", path.display(), e))?;
    let sha1 = verify::hash_file_sha1(path)
        .map_err(|e| anyhow::anyhow!("failed to read '{}': {}", path.display(), e))?;
    println!("SHA-256: {}", sha256);
    println!("SHA-1:   {}", sha1);
    println!("Size:    {} B", size);
    Ok(())
}

// ── hash-dir ──────────────────────────────────────────────────────────────────

fn cmd_hash_dir(path: &Path, exclude: &[String], toml_output: bool) -> Result<()> {
    let exclude_refs: Vec<&str> = exclude.iter().map(|s| s.as_str()).collect();
    let entries = verify::hash_dir(path, &exclude_refs)?;

    if entries.is_empty() {
        println!("No files found.");
        return Ok(());
    }

    if toml_output {
        for (abs_path, hash, size) in &entries {
            let rel = abs_path
                .strip_prefix(path)
                .unwrap_or(abs_path)
                .to_string_lossy()
                .replace('\\', "/");
            println!("[[files]]");
            println!("path   = {:?}", rel);
            println!("sha256 = {:?}", hash);
            println!("size   = {}", size);
            println!();
        }
    } else {
        println!("{:<64}  {:>12}  Path", "SHA-256", "Size (B)");
        println!("{}", "-".repeat(120));
        for (abs_path, hash, size) in &entries {
            let rel = abs_path
                .strip_prefix(path)
                .unwrap_or(abs_path)
                .to_string_lossy()
                .replace('\\', "/");
            println!("{hash}  {size:>12}  {rel}");
        }
    }

    println!("\n{} files", entries.len());
    Ok(())
}
