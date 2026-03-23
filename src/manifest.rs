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
                    "unknown version '{}' — run `mhf-outpost list` to see available versions",
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    // ── FileEntry::is_placeholder ────────────────────────────────────────────

    #[test]
    fn placeholder_all_zeros() {
        let entry = FileEntry {
            path: "test.exe".into(),
            sha256: "0".repeat(64),
            size: 0,
            optional: false,
            kind: FileKind::Core,
        };
        assert!(entry.is_placeholder());
    }

    #[test]
    fn placeholder_empty_sha256() {
        let entry = FileEntry {
            path: "test.exe".into(),
            sha256: String::new(),
            size: 0,
            optional: false,
            kind: FileKind::Core,
        };
        assert!(entry.is_placeholder());
    }

    #[test]
    fn not_placeholder_with_real_hash() {
        let entry = FileEntry {
            path: "test.exe".into(),
            sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into(),
            size: 1234,
            optional: false,
            kind: FileKind::Core,
        };
        assert!(!entry.is_placeholder());
    }

    // ── FileEntry::absolute ──────────────────────────────────────────────────

    #[test]
    fn absolute_joins_root_and_path() {
        let entry = FileEntry {
            path: "mhf/mhf.exe".into(),
            sha256: "0".repeat(64),
            size: 0,
            optional: false,
            kind: FileKind::Core,
        };
        let root = Path::new("/game");
        let abs = entry.absolute(root);
        // Forward slashes in path are normalised to the OS separator.
        assert!(abs.starts_with("/game"));
        assert!(abs.ends_with("mhf.exe"));
    }

    // ── Manifest::load — embedded manifests round-trip ───────────────────────

    #[test]
    fn load_known_version_zz() {
        let m = Manifest::load("zz").expect("zz manifest should be embedded");
        assert_eq!(m.version.id.to_ascii_uppercase(), "ZZ");
        assert!(!m.version.name.is_empty());
    }

    #[test]
    fn load_unknown_version_errors() {
        let result = Manifest::load("does_not_exist");
        assert!(result.is_err());
    }

    // ── Manifest::load_file — inline TOML round-trip ─────────────────────────

    #[test]
    fn load_file_minimal_toml() {
        use std::io::Write;
        let toml = r#"
[version]
id = "TEST"
name = "Test Version"
description = "Unit test manifest"
platform = "PC"

[[files]]
path = "test.exe"
sha256 = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
size = 1024
"#;
        let mut tmp = std::env::temp_dir();
        tmp.push("mhf_outpost_manifest_test.toml");
        {
            let mut f = std::fs::File::create(&tmp).unwrap();
            f.write_all(toml.as_bytes()).unwrap();
        }
        let m = Manifest::load_file(&tmp).expect("should parse minimal TOML");
        std::fs::remove_file(&tmp).ok();

        assert_eq!(m.version.id, "TEST");
        assert_eq!(m.files.len(), 1);
        assert!(!m.files[0].is_placeholder());
        assert_eq!(m.recorded_count(), 1);
    }

    // ── Manifest::all ────────────────────────────────────────────────────────

    #[test]
    fn all_returns_non_empty_list() {
        let all = Manifest::all();
        assert!(!all.is_empty(), "at least one embedded manifest expected");
    }
}
