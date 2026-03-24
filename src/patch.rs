//! Apply translations from the MHFrontier-Translation release JSON to game files.
//!
//! The JSON maps language → xpath → list of translation entries. Each entry has
//! a `location` (hex offset @ source file), `source` (original text), and
//! `target` (translated text). This module patches the target text into the
//! game binary at the appropriate offset.

use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::path::Path;

use crate::ecd;
use crate::jkr;

/// Result of applying translations to a single game file.
pub struct ApplyResult {
    pub file: String,
    pub count: usize,
}

/// Xpath prefix → game file relative path.
fn xpath_to_game_file(xpath_prefix: &str) -> Option<&'static str> {
    match xpath_prefix {
        "dat" => Some("dat/mhfdat.bin"),
        "pac" => Some("dat/mhfpac.bin"),
        "inf" => Some("dat/mhfinf.bin"),
        "jmp" => Some("dat/mhfjmp.bin"),
        "nav" => Some("dat/mhfnav.bin"),
        _ => None,
    }
}

/// Parse a location string like "0x46e000@mhfdat-jp.bin" → hex offset.
fn parse_location(location: &str) -> Result<usize> {
    let hex_part = location
        .split('@')
        .next()
        .unwrap_or(location);
    let hex_str = hex_part.trim_start_matches("0x").trim_start_matches("0X");
    usize::from_str_radix(hex_str, 16)
        .with_context(|| format!("invalid hex offset in location: {location}"))
}

/// Encode a string to Shift-JIS. Characters that can't be encoded are replaced.
fn encode_shift_jis(text: &str) -> Vec<u8> {
    let (encoded, _, _) = encoding_rs::SHIFT_JIS.encode(text);
    encoded.into_owned()
}

/// Apply all translations from the JSON file to game files in `game_dir`.
///
/// If `compress` is true, output files are JKR-HFI compressed.
/// If `encrypt` is true, output files are ECD-encrypted (key 4).
pub fn apply_translations(
    json_path: &Path,
    lang: &str,
    game_dir: &Path,
    compress: bool,
    encrypt: bool,
) -> Result<Vec<ApplyResult>> {
    let json_data = std::fs::read_to_string(json_path)
        .with_context(|| format!("failed to read {}", json_path.display()))?;

    // Parse as { lang: { xpath: [ { location, source, target } ] } }
    let root: serde_json::Value =
        serde_json::from_str(&json_data).context("failed to parse translation JSON")?;

    let lang_obj = root
        .get(lang)
        .and_then(|v| v.as_object())
        .ok_or_else(|| anyhow::anyhow!("language '{}' not found in translation JSON", lang))?;

    // Group entries by target game file.
    // Map: game_file_rel_path → Vec<(offset, target_text)>
    let mut file_entries: HashMap<String, Vec<(usize, String)>> = HashMap::new();

    for (xpath, entries_val) in lang_obj {
        let prefix = xpath.split('/').next().unwrap_or("");
        let game_file = match xpath_to_game_file(prefix) {
            Some(f) => f.to_string(),
            None => continue, // skip unknown prefixes
        };

        let entries = match entries_val.as_array() {
            Some(a) => a,
            None => continue,
        };

        for entry in entries {
            let location = entry
                .get("location")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let target = entry
                .get("target")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            if target.is_empty() || location.is_empty() {
                continue;
            }

            let offset = parse_location(location)?;
            file_entries
                .entry(game_file.clone())
                .or_default()
                .push((offset, target.to_string()));
        }
    }

    let mut results = Vec::new();

    for (rel_path, entries) in &file_entries {
        let file_path = game_dir.join(rel_path);
        if !file_path.exists() {
            bail!(
                "game file not found: {} (expected at {})",
                rel_path,
                file_path.display()
            );
        }

        let raw = std::fs::read(&file_path)
            .with_context(|| format!("failed to read {}", file_path.display()))?;

        // Auto-detect and strip ECD encryption.
        let decrypted = if ecd::is_ecd(&raw) {
            ecd::decode_ecd(&raw)?
        } else {
            raw
        };

        // Auto-detect and decompress JKR.
        let mut data = if jkr::is_jkr(&decrypted) {
            jkr::decompress_jkr(&decrypted)?
        } else {
            decrypted
        };

        // Apply translations: append encoded string to end of file,
        // write new pointer at the original offset.
        let mut count = 0;
        for (offset, target_text) in entries {
            let encoded = encode_shift_jis(target_text);
            let new_pos = data.len() as u32;

            // Ensure the offset is within bounds for writing a u32.
            if *offset + 4 > data.len() {
                // Extend if needed (pointer table might be near the end).
                data.resize(*offset + 4, 0);
            }

            // Write the new position as u32le at the pointer offset.
            data[*offset..*offset + 4].copy_from_slice(&new_pos.to_le_bytes());

            // Append the encoded string + null terminator.
            data.extend_from_slice(&encoded);
            data.push(0);

            count += 1;
        }

        // Repack: compress then encrypt.
        let mut output = data;
        if compress {
            output = jkr::compress_jkr_hfi(&output);
        }
        if encrypt {
            output = ecd::encode_ecd(&output, 4);
        }

        std::fs::write(&file_path, &output)
            .with_context(|| format!("failed to write {}", file_path.display()))?;

        results.push(ApplyResult {
            file: rel_path.clone(),
            count,
        });
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_apply_plain_binary() {
        // Build a small binary with a pointer table.
        // Layout: [ptr0 @ 0] [ptr1 @ 4] [original_str @ 8]
        let original_str = b"Hello\0";
        let mut binary = Vec::new();
        // ptr0 points to offset 8 (where "Hello" starts)
        binary.extend_from_slice(&8u32.to_le_bytes()); // offset 0: ptr0
        binary.extend_from_slice(&8u32.to_le_bytes()); // offset 4: ptr1
        binary.extend_from_slice(original_str); // offset 8: "Hello\0"

        // Create temp dir.
        let dir = std::env::temp_dir().join("mhf_outpost_test_patch");
        let dat_dir = dir.join("dat");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dat_dir).unwrap();

        // Write binary as dat/mhfdat.bin (plain, no ECD/JKR).
        let bin_path = dat_dir.join("mhfdat.bin");
        std::fs::write(&bin_path, &binary).unwrap();

        // Create JSON.
        let json = serde_json::json!({
            "fr": {
                "dat/armors/head": [
                    {
                        "location": "0x0@mhfdat-jp.bin",
                        "source": "Hello",
                        "target": "Bonjour"
                    }
                ]
            }
        });
        let json_path = dir.join("translations.json");
        let mut f = std::fs::File::create(&json_path).unwrap();
        write!(f, "{}", serde_json::to_string(&json).unwrap()).unwrap();

        // Apply without compress/encrypt so we can inspect raw output.
        let results =
            apply_translations(&json_path, "fr", &dir, false, false).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].file, "dat/mhfdat.bin");
        assert_eq!(results[0].count, 1);

        // Read back the patched binary.
        let patched = std::fs::read(&bin_path).unwrap();

        // The pointer at offset 0 should now point past the original data.
        let new_ptr = u32::from_le_bytes([patched[0], patched[1], patched[2], patched[3]]);
        assert_eq!(new_ptr as usize, binary.len()); // appended right after original

        // The appended data should be Shift-JIS "Bonjour" + null.
        let appended_start = new_ptr as usize;
        let (expected, _, _) = encoding_rs::SHIFT_JIS.encode("Bonjour");
        let appended = &patched[appended_start..appended_start + expected.len()];
        assert_eq!(appended, expected.as_ref());
        assert_eq!(patched[appended_start + expected.len()], 0); // null terminator

        // ptr1 at offset 4 should be unchanged (we only patched offset 0).
        let ptr1 = u32::from_le_bytes([patched[4], patched[5], patched[6], patched[7]]);
        assert_eq!(ptr1, 8);

        // Cleanup.
        let _ = std::fs::remove_dir_all(&dir);
    }
}
