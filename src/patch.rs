//! Apply translations from the MHFrontier-Translation v0.1.0+ release JSON.
//!
//! The release JSON shape is:
//!
//! ```json
//! { "<lang>": { "<xpath>": [ { "index": "0", "source": "…", "target": "…" }, … ] } }
//! ```
//!
//! Since release v0.2.0 the `source` field may be absent (per-language
//! `--no-source` payload) — entries are resolved purely by `index`.
//!
//! `index` is a stable per-section slot number into the section's pointer
//! table. We resolve indexes to absolute file offsets by re-extracting
//! the section live from the (decrypted/decompressed) game binary using
//! [`crate::pointer_tables`], which ports FTH's extractor.
//!
//! Strings whose source extraction grouped multiple pointers appear as a
//! single JSON entry with segments separated by `{j}` (v0.2.0+) or the
//! legacy `<join at="…">` tag (v0.1.0). Both forms are accepted. Segments
//! are mapped **positionally** onto the entry's offset list; any literal
//! offset value inside a legacy tag is discarded because it bakes the
//! original binary's offsets into the JSON and would not survive any
//! string-length shift.
//!
//! Inline color spans use the ASCII-safe brace form `{cNN}` (open) and
//! `{/c}` (close/reset) in the JSON. Before Shift-JIS encoding we rewrite
//! them to the game's native `~CNN` / `~C00` form (byte `0x7E` followed by
//! `'C'` and two decimal digits), which is what the renderer expects.
//!
//! Strategy: **rebuild_section** (matches FTH's `rebuild_section`).
//! For each game file, for each section, we extract every entry, build
//! a `pointer_offset → new_text` map merging the translations on top of
//! the originals, then write the entire section's string blob
//! contiguously at EOF and rewrite all pointers to it. This eliminates
//! orphaned bytes from the in-place pointer table and produces clean
//! output.

use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::path::Path;

use crate::ecd;
use crate::jkr;
use crate::pointer_tables::{self, EntryOffsets, SectionConfig};

/// Result of applying translations to a single game file.
pub struct ApplyResult {
    pub file: String,
    pub count: usize,
}

/// Map xpath top-level prefix → game file relative path.
fn xpath_to_game_file(prefix: &str) -> Option<&'static str> {
    match prefix {
        "dat" => Some("dat/mhfdat.bin"),
        "pac" => Some("dat/mhfpac.bin"),
        "inf" => Some("dat/mhfinf.bin"),
        "jmp" => Some("dat/mhfjmp.bin"),
        "nav" => Some("dat/mhfnav.bin"),
        "gao" => Some("dat/mhfgao.bin"),
        "sqd" => Some("dat/mhfsqd.bin"),
        "rcc" => Some("dat/mhfrcc.bin"),
        "msx" => Some("dat/mhfmsx.bin"),
        _ => None,
    }
}

/// One translation entry from the JSON, after parsing.
struct Translation {
    index: usize,
    /// Original `target` string from the JSON (may contain `<join at="…">` markup).
    /// We store it raw and split it positionally per entry at apply time.
    target_raw: String,
    /// Original `source` (used to suppress no-op writes).
    source_raw: String,
}

/// Split a grouped-entry text on its join markers. Accepts both the
/// v0.2.0+ `{j}` marker and the legacy `<join at="…">` tag. Any literal
/// offset inside a legacy tag is discarded; segments are matched
/// positionally to the extractor's offset list at apply time.
fn split_joined(text: &str) -> Vec<String> {
    // Fast path
    if !text.contains("<join") && !text.contains("{j}") {
        return vec![text.to_string()];
    }
    let mut out = Vec::new();
    let mut rest = text;
    loop {
        let Some((start, end)) = find_next_join_marker(rest) else {
            out.push(rest.to_string());
            return out;
        };
        out.push(rest[..start].to_string());
        rest = &rest[end..];
    }
}

/// Locate the next join marker in `s`. Returns `(start, end)` byte offsets
/// bracketing the marker (the slice `s[start..end]` is the marker text),
/// or `None` if neither form is present.
fn find_next_join_marker(s: &str) -> Option<(usize, usize)> {
    let legacy = s.find("<join");
    let brace = s.find("{j}");
    let use_legacy = match (legacy, brace) {
        (None, None) => return None,
        (Some(_), None) => true,
        (None, Some(_)) => false,
        (Some(a), Some(b)) => a < b,
    };
    if use_legacy {
        let start = legacy.unwrap();
        // A malformed tag with no closing '>' — swallow the rest so we
        // don't loop forever; the caller will treat what follows as one
        // trailing segment on the next iteration.
        let close_rel = s[start..].find('>').unwrap_or(s.len() - start - 1);
        Some((start, start + close_rel + 1))
    } else {
        let start = brace.unwrap();
        Some((start, start + "{j}".len()))
    }
}

/// Rewrite the brace-form color codes (`{cNN}` / `{/c}`) used in the
/// translation JSON back to the game's `~CNN` / `~C00` wire form before
/// Shift-JIS encoding.
///
/// The game stores color spans as the byte `0x7E` followed by `'C'` and
/// two decimal digits. In plain Shift-JIS, `0x7E` decodes to ASCII tilde
/// (`~`), so we intentionally emit ASCII tilde — `encoding_rs::SHIFT_JIS`
/// maps it back to `0x7E` byte-for-byte. (The Python toolchain uses
/// `shift_jisx0213` where the same byte is `‾`, the overline character;
/// both Unicode spellings produce the same byte in the file, but ASCII
/// tilde is the portable choice for `encoding_rs`.)
///
/// Bytes in multi-byte UTF-8 runs are always `>= 0x80`, so scanning at
/// the byte level for the ASCII markers never confuses continuation
/// bytes with real `{`/`/`/`c`/`}` characters — any `{` byte we see is a
/// genuine brace.
fn color_codes_from_csv(text: &str) -> String {
    if !text.contains('{') {
        return text.to_string();
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        // {/c}  → ~C00
        if i + 4 <= bytes.len() && &bytes[i..i + 4] == b"{/c}" {
            out.extend_from_slice(b"~C00");
            i += 4;
            continue;
        }
        // {cNN} → ~CNN
        if i + 5 <= bytes.len()
            && bytes[i] == b'{'
            && bytes[i + 1] == b'c'
            && bytes[i + 2].is_ascii_digit()
            && bytes[i + 3].is_ascii_digit()
            && bytes[i + 4] == b'}'
        {
            out.extend_from_slice(b"~C");
            out.push(bytes[i + 2]);
            out.push(bytes[i + 3]);
            i += 5;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    // Safe: we only copy whole bytes of valid UTF-8 and inject ASCII.
    String::from_utf8(out).expect("color_codes_from_csv preserves UTF-8")
}

/// Transliterate characters that have no Shift-JIS representation into
/// ASCII-ish equivalents. The MHF font has no glyphs for accented Latin
/// letters, so without this step `encoding_rs` would emit HTML numeric
/// character references (e.g. `é` → `&#233;`) which render literally
/// in-game. This is a lossy fallback for the official clients; a proper
/// custom font/codepage would be needed to keep the accents.
fn transliterate_for_sjis(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' => "A".to_string(),
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => "a".to_string(),
            'Æ' => "AE".to_string(),
            'æ' => "ae".to_string(),
            'Ç' => "C".to_string(),
            'ç' => "c".to_string(),
            'È' | 'É' | 'Ê' | 'Ë' => "E".to_string(),
            'è' | 'é' | 'ê' | 'ë' => "e".to_string(),
            'Ì' | 'Í' | 'Î' | 'Ï' => "I".to_string(),
            'ì' | 'í' | 'î' | 'ï' => "i".to_string(),
            'Ñ' => "N".to_string(),
            'ñ' => "n".to_string(),
            'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' | 'Ø' => "O".to_string(),
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' => "o".to_string(),
            'Œ' => "OE".to_string(),
            'œ' => "oe".to_string(),
            'Ù' | 'Ú' | 'Û' | 'Ü' => "U".to_string(),
            'ù' | 'ú' | 'û' | 'ü' => "u".to_string(),
            'Ý' | 'Ÿ' => "Y".to_string(),
            'ý' | 'ÿ' => "y".to_string(),
            'ß' => "ss".to_string(),
            '«' | '»' => "\"".to_string(),
            '‹' | '›' => "'".to_string(),
            '“' | '”' | '„' => "\"".to_string(),
            '‘' | '’' | '‚' => "'".to_string(),
            '–' | '—' => "-".to_string(),
            '…' => "...".to_string(),
            ' ' => " ".to_string(), // non-breaking space → regular space
            other => other.to_string(),
        })
        .collect()
}

fn encode_shift_jis(text: &str) -> Vec<u8> {
    let with_game_codes = color_codes_from_csv(text);
    let translit = transliterate_for_sjis(&with_game_codes);
    let (encoded, _, _) = encoding_rs::SHIFT_JIS.encode(&translit);
    encoded.into_owned()
}

/// Decode a Shift-JIS string at `start` until null. Used to read the
/// untranslated original strings out of an entry's pointer slot when
/// rebuilding the section.
fn read_string_at(data: &[u8], slot: u32) -> Result<String> {
    let ptr = u32::from_le_bytes([
        data[slot as usize],
        data[slot as usize + 1],
        data[slot as usize + 2],
        data[slot as usize + 3],
    ]) as usize;
    if ptr >= data.len() {
        bail!("string pointer {ptr:#x} oob");
    }
    let mut end = ptr;
    while end < data.len() && data[end] != 0 {
        end += 1;
    }
    let (s, _, _) = encoding_rs::SHIFT_JIS.decode(&data[ptr..end]);
    Ok(s.into_owned())
}

/// Apply all translations from `json_path` to game files in `game_dir`.
pub fn apply_translations(
    json_path: &Path,
    lang: &str,
    game_dir: &Path,
    compress: bool,
    encrypt: bool,
) -> Result<Vec<ApplyResult>> {
    let json_data = std::fs::read_to_string(json_path)
        .with_context(|| format!("failed to read {}", json_path.display()))?;
    let root: serde_json::Value =
        serde_json::from_str(&json_data).context("failed to parse translation JSON")?;

    let lang_obj = root
        .get(lang)
        .and_then(|v| v.as_object())
        .ok_or_else(|| anyhow::anyhow!("language '{}' not found in translation JSON", lang))?;

    // Group sections by target game file: rel_path → Vec<(xpath, [Translation])>
    let mut by_file: HashMap<String, Vec<(String, Vec<Translation>)>> = HashMap::new();
    for (xpath, entries_val) in lang_obj {
        let prefix = xpath.split('/').next().unwrap_or("");
        let rel_path = match xpath_to_game_file(prefix) {
            Some(f) => f.to_string(),
            None => {
                eprintln!("  skipping unknown xpath prefix '{prefix}' for section '{xpath}'");
                continue;
            }
        };
        let entries = match entries_val.as_array() {
            Some(a) => a,
            None => continue,
        };
        let mut translations = Vec::new();
        for entry in entries {
            let target = entry.get("target").and_then(|v| v.as_str()).unwrap_or("");
            let source = entry.get("source").and_then(|v| v.as_str()).unwrap_or("");
            if target.is_empty() || target == source {
                continue;
            }
            // `index` may be a string ("0") or a number; accept both.
            let index = match entry.get("index") {
                Some(serde_json::Value::String(s)) => s.parse::<usize>().ok(),
                Some(serde_json::Value::Number(n)) => n.as_u64().map(|u| u as usize),
                _ => None,
            };
            let Some(index) = index else { continue };
            translations.push(Translation {
                index,
                target_raw: target.to_string(),
                source_raw: source.to_string(),
            });
        }
        if !translations.is_empty() {
            by_file
                .entry(rel_path)
                .or_default()
                .push((xpath.clone(), translations));
        }
    }

    let mut results = Vec::new();
    for (rel_path, sections) in by_file {
        let file_path = game_dir.join(&rel_path);
        if !file_path.exists() {
            eprintln!(
                "  game file not found, skipping: {} (expected at {})",
                rel_path,
                file_path.display()
            );
            continue;
        }
        let raw = std::fs::read(&file_path)
            .with_context(|| format!("failed to read {}", file_path.display()))?;

        let was_encrypted = ecd::is_ecd(&raw);
        let decrypted = if was_encrypted {
            ecd::decode_ecd(&raw)?
        } else {
            raw
        };
        let was_compressed = jkr::is_jkr(&decrypted);
        let mut data = if was_compressed {
            jkr::decompress_jkr(&decrypted)?
        } else {
            decrypted
        };

        let mut total = 0;
        for (xpath, translations) in &sections {
            let cfg = pointer_tables::lookup(xpath)
                .with_context(|| format!("loading config for {xpath}"))?;
            let count = rebuild_section(&mut data, &cfg, translations, xpath)?;
            total += count;
        }

        let mut output = data;
        if was_compressed || compress {
            output = jkr::compress_jkr_hfi(&output);
        }
        if was_encrypted || encrypt {
            output = ecd::encode_ecd(&output, 4);
        }
        std::fs::write(&file_path, &output)
            .with_context(|| format!("failed to write {}", file_path.display()))?;

        results.push(ApplyResult {
            file: rel_path,
            count: total,
        });
    }

    Ok(results)
}

/// Rebuild one section in `data` in-place: appends a fresh contiguous
/// string blob at EOF carrying every entry of the section (translated
/// where present, original otherwise) and rewrites every pointer slot
/// in the section to point at it.
///
/// Returns the number of translations actually applied (i.e. how many
/// indexes from `translations` resolved to a slot).
fn rebuild_section(
    data: &mut Vec<u8>,
    cfg: &SectionConfig,
    translations: &[Translation],
    xpath: &str,
) -> Result<usize> {
    let entries: Vec<EntryOffsets> = pointer_tables::extract(data, cfg)
        .with_context(|| format!("re-extracting section {xpath}"))?;

    // Build slot → new_text from translations, splitting joined entries
    // positionally onto the entry's offset list.
    let mut overrides: HashMap<u32, String> = HashMap::new();
    let mut applied = 0;
    for t in translations {
        if t.index >= entries.len() {
            eprintln!(
                "  {xpath}: index {} out of range (section has {} entries) — skipping",
                t.index,
                entries.len()
            );
            continue;
        }
        let entry = &entries[t.index];
        let target_segments = split_joined(&t.target_raw);
        let source_segments = split_joined(&t.source_raw);
        // Use the shorter of (segments, entry slots) — if mismatched,
        // we still apply what we can and warn.
        if target_segments.len() != entry.offsets.len() {
            eprintln!(
                "  {xpath}[{}]: {} target segment(s) but {} pointer slot(s); applying min",
                t.index,
                target_segments.len(),
                entry.offsets.len()
            );
        }
        let n = target_segments.len().min(entry.offsets.len());
        for (i, target) in target_segments.iter().enumerate().take(n) {
            // Skip segments that are unchanged from source.
            let src = source_segments.get(i).map(String::as_str).unwrap_or("");
            if target == src {
                continue;
            }
            overrides.insert(entry.offsets[i], target.clone());
        }
        applied += 1;
    }

    // Walk every entry slot in the section, write its (overridden or
    // original) string contiguously at EOF, and rewrite the pointer.
    for entry in &entries {
        for &slot in &entry.offsets {
            let text = match overrides.get(&slot) {
                Some(t) => t.clone(),
                None => read_string_at(data, slot)?,
            };
            let encoded = encode_shift_jis(&text);
            let new_pos = data.len() as u32;
            data.extend_from_slice(&encoded);
            data.push(0);
            // Rewrite the pointer slot.
            let s = slot as usize;
            if s + 4 > data.len() {
                bail!("slot {slot:#x} oob during pointer rewrite");
            }
            data[s..s + 4].copy_from_slice(&new_pos.to_le_bytes());
        }
    }

    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pointer_tables::Mode;

    #[test]
    fn split_joined_no_markup() {
        assert_eq!(split_joined("hello"), vec!["hello".to_string()]);
    }

    #[test]
    fn split_joined_legacy_tags() {
        let s = r#"a<join at="100">b<join at="104">c"#;
        assert_eq!(split_joined(s), vec!["a", "b", "c"]);
    }

    #[test]
    fn split_joined_brace_marker() {
        assert_eq!(split_joined("a{j}b{j}c"), vec!["a", "b", "c"]);
    }

    #[test]
    fn split_joined_mixed_forms() {
        // Some pre-1.6.0 translation entries may still carry a mix after
        // partial hand-editing; we should handle both markers in one text.
        let s = r#"a<join at="100">b{j}c"#;
        assert_eq!(split_joined(s), vec!["a", "b", "c"]);
    }

    #[test]
    fn color_codes_from_csv_open_and_close() {
        assert_eq!(color_codes_from_csv("{c05}Red{/c}"), "~C05Red~C00");
    }

    #[test]
    fn color_codes_from_csv_no_markers() {
        assert_eq!(color_codes_from_csv("plain text"), "plain text");
    }

    #[test]
    fn color_codes_from_csv_preserves_utf8() {
        // Japanese characters are multi-byte UTF-8; byte-level scanning
        // must not split or corrupt them.
        assert_eq!(
            color_codes_from_csv("{c07}ＳＰ防具：青{/c}"),
            "~C07ＳＰ防具：青~C00"
        );
    }

    #[test]
    fn color_codes_from_csv_non_marker_brace_untouched() {
        // Unrelated braces should pass through without being mangled.
        assert_eq!(color_codes_from_csv("{ not a marker }"), "{ not a marker }");
        assert_eq!(color_codes_from_csv("{cXX}"), "{cXX}");
    }

    #[test]
    fn encode_shift_jis_produces_game_color_bytes() {
        let bytes = encode_shift_jis("{c05}A{/c}");
        // 0x7E 'C' '0' '5' 'A' 0x7E 'C' '0' '0'
        assert_eq!(bytes, b"\x7eC05A\x7eC00");
    }

    /// End-to-end: build a fixture binary with one pointer-pair section,
    /// craft a v0.1.0-shaped JSON, run apply_translations, and verify
    /// each pointer ends up pointing at the new Shift-JIS text.
    #[test]
    fn apply_v0_1_0_format_end_to_end() {
        // Two entries, each one pointer slot.
        let mut data = vec![0u8; 0x300];
        data[0x40..0x44].copy_from_slice(&0x100u32.to_le_bytes()); // begin
        data[0x44..0x48].copy_from_slice(&0x108u32.to_le_bytes()); // next_field
        data[0x100..0x104].copy_from_slice(&0x200u32.to_le_bytes());
        data[0x104..0x108].copy_from_slice(&0x208u32.to_le_bytes());
        data[0x200..0x205].copy_from_slice(b"foo\0\0");
        data[0x208..0x20C].copy_from_slice(b"bar\0");

        // Patch in-place via rebuild_section using a fake config that
        // matches the fixture (instead of going through headers.json).
        let cfg = SectionConfig {
            begin_pointer: 0x40,
            mode: Mode::PointerPair {
                next_field_pointer: 0x44,
                crop_end: 0,
            },
        };
        let translations = vec![
            Translation {
                index: 0,
                source_raw: "foo".to_string(),
                target_raw: "BONJOUR".to_string(),
            },
            Translation {
                index: 1,
                source_raw: "bar".to_string(),
                target_raw: "MONDE".to_string(),
            },
        ];

        let applied = rebuild_section(&mut data, &cfg, &translations, "test").unwrap();
        assert_eq!(applied, 2);

        // After rebuild, slot 0x100 → "BONJOUR\0", slot 0x104 → "MONDE\0",
        // both appended at end of file in section order.
        let p0 = u32::from_le_bytes([data[0x100], data[0x101], data[0x102], data[0x103]]) as usize;
        let p1 = u32::from_le_bytes([data[0x104], data[0x105], data[0x106], data[0x107]]) as usize;
        assert_eq!(&data[p0..p0 + 8], b"BONJOUR\0");
        assert_eq!(&data[p1..p1 + 6], b"MONDE\0");
        // p1 should come immediately after p0's null terminator.
        assert_eq!(p1, p0 + 8);
    }

    #[test]
    fn unchanged_target_is_skipped() {
        let mut data = vec![0u8; 0x300];
        data[0x40..0x44].copy_from_slice(&0x100u32.to_le_bytes());
        data[0x44..0x48].copy_from_slice(&0x104u32.to_le_bytes());
        data[0x100..0x104].copy_from_slice(&0x200u32.to_le_bytes());
        data[0x200..0x204].copy_from_slice(b"abc\0");

        let cfg = SectionConfig {
            begin_pointer: 0x40,
            mode: Mode::PointerPair {
                next_field_pointer: 0x44,
                crop_end: 0,
            },
        };
        // target == source: should still rewrite slot to point to a fresh
        // copy of the original text (rebuild semantics), but should not
        // count as an override entry. We just check it doesn't panic and
        // the file remains semantically valid.
        let translations = vec![Translation {
            index: 0,
            source_raw: "abc".to_string(),
            target_raw: "abc".to_string(),
        }];
        let _ = rebuild_section(&mut data, &cfg, &translations, "test").unwrap();
        let p0 = u32::from_le_bytes([data[0x100], data[0x101], data[0x102], data[0x103]]) as usize;
        assert_eq!(&data[p0..p0 + 4], b"abc\0");
    }
}
