use crate::manifest::{ArchiveSource, FileEntry, FileKind, Manifest};
use anyhow::Result;
use indicatif::{ParallelProgressIterator, ProgressBar, ProgressStyle};
use rayon::prelude::*;
use sha1::Digest as Sha1Digest;
use sha2::Digest as Sha2Digest;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

const BUF_SIZE: usize = 1024 * 1024; // 1 MiB

// ── Archive verification (SHA-1, matches archive.org) ───────────────────────

pub fn hash_file_sha1(path: &Path) -> Result<String, std::io::Error> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut hasher = sha1::Sha1::new();
    let mut buf = vec![0u8; BUF_SIZE];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        Sha1Digest::update(&mut hasher, &buf[..n]);
    }
    Ok(hex::encode(Sha1Digest::finalize(hasher)))
}

pub struct ArchiveCheckResult {
    pub path: PathBuf,
    pub expected_sha1: String,
    pub actual_sha1: String,
    pub expected_size: u64,
    pub actual_size: u64,
}

impl ArchiveCheckResult {
    pub fn sha1_ok(&self) -> bool {
        self.actual_sha1 == self.expected_sha1
    }
    pub fn size_ok(&self) -> bool {
        self.actual_size == self.expected_size
    }
}

pub fn verify_archive(archive: &ArchiveSource, path: &Path) -> Result<ArchiveCheckResult> {
    let meta = path
        .metadata()
        .map_err(|e| anyhow::anyhow!("cannot stat '{}': {}", path.display(), e))?;

    let pb = progress_bar(meta.len(), "hashing");
    let actual_sha1 = hash_with_progress_sha1(path, &pb)?;
    pb.finish_and_clear();

    Ok(ArchiveCheckResult {
        path: path.to_path_buf(),
        expected_sha1: archive.sha1.clone(),
        actual_sha1,
        expected_size: archive.size,
        actual_size: meta.len(),
    })
}

// ── Extracted-file verification (SHA-256) ───────────────────────────────────

pub fn hash_file_sha256(path: &Path) -> Result<String, std::io::Error> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut hasher = sha2::Sha256::new();
    let mut buf = vec![0u8; BUF_SIZE];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        Sha2Digest::update(&mut hasher, &buf[..n]);
    }
    Ok(hex::encode(Sha2Digest::finalize(hasher)))
}

// Keep the old name as an alias for callers (hash command, hash-dir).
pub use hash_file_sha256 as hash_file;

#[derive(Debug)]
pub enum FileStatus {
    Ok,
    Missing,
    SizeMismatch { expected: u64, actual: u64 },
    /// File hash differs from manifest. Interpretation depends on `FileResult::kind`.
    Modified { expected: String, actual: String },
    Placeholder,
    Unreadable(String),
}

impl FileStatus {
    pub fn is_ok(&self) -> bool {
        matches!(self, FileStatus::Ok | FileStatus::Placeholder)
    }
}

#[derive(Debug)]
pub struct FileResult {
    pub path: String,
    pub optional: bool,
    pub kind: FileKind,
    pub status: FileStatus,
}

impl FileResult {
    /// True when this result should cause `verify` to exit with an error.
    /// URL / config / translation mismatches are not hard failures.
    pub fn is_hard_failure(&self) -> bool {
        if self.optional {
            return false;
        }
        match &self.status {
            FileStatus::Ok | FileStatus::Placeholder => false,
            FileStatus::Missing => true,
            FileStatus::SizeMismatch { .. } => true,
            FileStatus::Unreadable(_) => true,
            FileStatus::Modified { .. } => self.kind == FileKind::Core,
        }
    }
}

pub struct VerifyReport {
    pub results: Vec<FileResult>,
}

impl VerifyReport {
    pub fn hard_failures(&self) -> impl Iterator<Item = &FileResult> {
        self.results.iter().filter(|r| r.is_hard_failure())
    }

    pub fn modified(&self) -> impl Iterator<Item = &FileResult> {
        self.results.iter().filter(|r| {
            matches!(r.status, FileStatus::Modified { .. }) && r.kind != FileKind::Core
        })
    }

    pub fn ok_count(&self) -> usize {
        self.results.iter().filter(|r| r.status.is_ok()).count()
    }

    pub fn placeholder_count(&self) -> usize {
        self.results
            .iter()
            .filter(|r| matches!(r.status, FileStatus::Placeholder))
            .count()
    }
}

fn check_entry(entry: &FileEntry, root: &Path) -> FileResult {
    let abs = entry.absolute(root);
    let status = match abs.metadata() {
        Err(_) => FileStatus::Missing,
        Ok(meta) => {
            if entry.size > 0 && meta.len() != entry.size {
                FileStatus::SizeMismatch {
                    expected: entry.size,
                    actual: meta.len(),
                }
            } else if entry.is_placeholder() {
                FileStatus::Placeholder
            } else {
                match hash_file_sha256(&abs) {
                    Err(e) => FileStatus::Unreadable(e.to_string()),
                    Ok(actual) if actual == entry.sha256 => FileStatus::Ok,
                    Ok(actual) => FileStatus::Modified {
                        expected: entry.sha256.clone(),
                        actual,
                    },
                }
            }
        }
    };
    FileResult {
        path: entry.path.clone(),
        optional: entry.optional,
        kind: entry.kind,
        status,
    }
}

pub fn verify(manifest: &Manifest, root: &Path) -> VerifyReport {
    let total = manifest.files.len() as u64;
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.cyan} [{bar:40.cyan/blue}] {pos}/{len} {wide_msg}",
        )
        .unwrap()
        .progress_chars("=>-"),
    );

    let results: Vec<FileResult> = manifest
        .files
        .par_iter()
        .progress_with(pb.clone())
        .map(|entry| check_entry(entry, root))
        .collect();

    pb.finish_and_clear();
    VerifyReport { results }
}

// ── hash-dir ─────────────────────────────────────────────────────────────────

pub fn hash_dir(root: &Path, exclude: &[&str]) -> Result<Vec<(PathBuf, String, u64)>> {
    use walkdir::WalkDir;

    let entries: Vec<_> = WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .collect();

    let total = entries.len() as u64;
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.cyan} [{bar:40.cyan/blue}] {pos}/{len} hashing {wide_msg}",
        )
        .unwrap()
        .progress_chars("=>-"),
    );

    let results: Vec<_> = entries
        .par_iter()
        .progress_with(pb.clone())
        .filter_map(|e| {
            let rel = e.path().strip_prefix(root).ok()?;
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            if exclude.iter().any(|ex| rel_str.starts_with(ex)) {
                return None;
            }
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            let hash = hash_file_sha256(e.path()).ok()?;
            Some((e.path().to_path_buf(), hash, size))
        })
        .collect();

    pb.finish_and_clear();

    let mut sorted = results;
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(sorted)
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn progress_bar(size: u64, verb: &str) -> ProgressBar {
    let pb = ProgressBar::new(size);
    pb.set_style(
        ProgressStyle::with_template(&format!(
            "{{spinner:.cyan}} [{{bar:40.cyan/blue}}] {{bytes}}/{{total_bytes}} {} {{wide_msg}}",
            verb
        ))
        .unwrap()
        .progress_chars("=>-"),
    );
    pb
}

fn hash_with_progress_sha1(path: &Path, pb: &ProgressBar) -> Result<String, std::io::Error> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut hasher = sha1::Sha1::new();
    let mut buf = vec![0u8; BUF_SIZE];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        Sha1Digest::update(&mut hasher, &buf[..n]);
        pb.inc(n as u64);
    }
    Ok(hex::encode(Sha1Digest::finalize(hasher)))
}
