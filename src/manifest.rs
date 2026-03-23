use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

macro_rules! include_manifest {
    ($name:literal) => {
        (
            $name,
            include_str!(concat!("../manifests/", $name, ".toml")),
        )
    };
}

/// All known version manifests, embedded at compile time.
const EMBEDDED: &[(&str, &str)] = &[
    include_manifest!("zz"),
    include_manifest!("g10"),
    include_manifest!("g91"),
    include_manifest!("gg"),
    include_manifest!("g52"),
    include_manifest!("g2"),
    include_manifest!("g1"),
    include_manifest!("f5"),
    include_manifest!("f4"),
    include_manifest!("s6"),
    include_manifest!("wiiu"),
];

#[derive(Debug, Deserialize, Serialize)]
pub struct VersionInfo {
    /// Short identifier used on the CLI (e.g. "ZZ", "GG").
    pub id: String,
    /// Human-readable name (e.g. "G10-ZZ").
    pub name: String,
    pub description: String,
    pub platform: String,
}

/// Metadata for a canonical archive on archive.org.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct ArchiveSource {
    /// archive.org item identifier (e.g. "MHFGG").
    pub identifier: String,
    /// Filename within the item (e.g. "MHFGG.zip").
    pub filename: String,
    /// Container format: ZIP, RAR, 7z …
    pub format: String,
    /// File size in bytes.
    pub size: u64,
    /// SHA-1 hex as provided by archive.org metadata.
    pub sha1: String,
    /// MD5 hex as provided by archive.org metadata.
    pub md5: String,
}

impl ArchiveSource {
    pub fn download_url(&self) -> String {
        format!(
            "https://archive.org/download/{}/{}",
            self.identifier, self.filename
        )
    }

    pub fn torrent_url(&self) -> String {
        format!(
            "https://archive.org/download/{}/{}_archive.torrent",
            self.identifier, self.identifier
        )
    }

    pub fn item_url(&self) -> String {
        format!("https://archive.org/details/{}", self.identifier)
    }
}

/// How to interpret a hash mismatch for a given file.
///
/// The default (`Core`) is intentionally the strictest: any unknown file gets
/// flagged as a tamper until explicitly classified otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FileKind {
    /// Executable or DLL. Any modification is unexpected tampering.
    #[default]
    Core,
    /// Server URL list (`url.lst` etc.). Modification is expected — users point
    /// the client at a community server.
    Url,
    /// Binary data file (`.bin`, `.txb` containing text tables). Modification
    /// usually means a fan translation, but the same files can carry game-value
    /// changes (stat modding). Flagged as a warning, not an error.
    Translation,
    /// Per-user configuration (`.ini`, `guildcard.bin`). Expected to differ
    /// between installs.
    Config,
}

impl FileKind {
    /// Human-readable label used in report output.
    pub fn label(self) -> &'static str {
        match self {
            Self::Core        => "core",
            Self::Url         => "url",
            Self::Translation => "translation",
            Self::Config      => "config",
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FileEntry {
    /// Relative path from the game root, forward-slash separated.
    pub path: String,
    /// Lowercase hex SHA-256. All-zeros means "not yet recorded".
    pub sha256: String,
    /// Expected file size in bytes. 0 means unknown.
    #[serde(default)]
    pub size: u64,
    /// If true, missing file is a warning rather than an error.
    #[serde(default)]
    pub optional: bool,
    /// How to interpret a hash mismatch for this file.
    #[serde(default)]
    pub kind: FileKind,
}

impl FileEntry {
    pub fn is_placeholder(&self) -> bool {
        self.sha256 == "0".repeat(64) || self.sha256.is_empty()
    }

    pub fn absolute(&self, root: &Path) -> PathBuf {
        root.join(self.path.replace('/', std::path::MAIN_SEPARATOR_STR))
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Manifest {
    pub version: VersionInfo,
    /// Canonical archive source on archive.org (absent for ZZ which has no upload).
    pub archive: Option<ArchiveSource>,
    #[serde(default)]
    pub files: Vec<FileEntry>,
}

impl Manifest {
    pub fn load(id: &str) -> Result<Self> {
        let key = id.to_ascii_lowercase();
        let src = EMBEDDED
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, src)| *src)
            .with_context(|| {
                format!(
                    "unknown version '{}' — run `mhf-installer list` to see available versions",
                    id
                )
            })?;
        toml::from_str(src)
            .with_context(|| format!("failed to parse embedded manifest for '{}'", id))
    }

    pub fn load_file(path: &Path) -> Result<Self> {
        let src = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read '{}'", path.display()))?;
        toml::from_str(&src)
            .with_context(|| format!("failed to parse manifest '{}'", path.display()))
    }

    pub fn all() -> Vec<Self> {
        EMBEDDED
            .iter()
            .filter_map(|(_, src)| toml::from_str(src).ok())
            .collect()
    }

    /// Number of extracted-file entries with real (non-placeholder) checksums.
    pub fn recorded_count(&self) -> usize {
        self.files.iter().filter(|f| !f.is_placeholder()).count()
    }
}
