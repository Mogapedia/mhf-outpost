//! Port of FrontierTextHandler's `pointer_tables.py` extractor.
//!
//! Loads `assets/headers.json` (vendored from FTH) and walks pointer
//! tables in a decrypted/decompressed game binary to produce, for each
//! section entry, the list of pointer-slot offsets it owns. The first
//! offset in each entry is the "primary" slot (matches FTH's
//! `entry["offset"]`); the remaining offsets correspond to FTH's
//! `<join at="N">` continuations.
//!
//! No string decoding happens here except inside `scan_region`'s
//! validation filter (which needs to reject mid-character pointers and
//! garbage decodes the same way FTH does, so the offset lists match).

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;

const HEADERS_JSON: &str = include_str!("../assets/headers.json");

/// One section entry: a list of pointer-slot offsets.
///
/// `offsets[0]` is the primary slot (FTH's `entry["offset"]`).
/// `offsets[1..]` are the join continuations (FTH's `<join at="…">`).
#[derive(Debug, Clone)]
pub struct EntryOffsets {
    pub offsets: Vec<u32>,
}

/// Parsed extraction config for a single xpath leaf.
#[derive(Debug, Clone)]
pub struct SectionConfig {
    pub begin_pointer: u32,
    pub mode: Mode,
}

#[derive(Debug, Clone)]
pub enum Mode {
    /// `next_field_pointer` mode — pointer pair defines [start, end)
    /// of a flat pointer table; consecutive null pointers terminate
    /// joined entries.
    PointerPair {
        next_field_pointer: u32,
        crop_end: u32,
    },
    /// `count_pointer` mode — `begin_pointer` deref is array start,
    /// `count_pointer` deref is the entry count.
    CountPointer { count_pointer: u32 },
    /// `count_base_pointer` (no `entry_size`) — flat pointer array
    /// whose count is read from an indirect header table.
    IndirectCountFlat {
        count_base_pointer: u32,
        count_offset: u32,
        count_type: CountType,
        count_adjust: i32,
        pointers_per_entry: u32,
    },
    /// `count_base_pointer` + `entry_size` — struct-strided table
    /// with count from an indirect header table.
    IndirectCountStrided {
        count_base_pointer: u32,
        count_offset: u32,
        count_type: CountType,
        count_adjust: i32,
        entry_size: u32,
        field_offsets: Vec<u32>,
    },
    /// `null_terminated` — terminated by a null first-pointer.
    NullTerminated {
        pointers_per_entry: u32,
        grouped: bool,
    },
    /// `entry_count` + `entry_size` — fixed-size struct array.
    StructStrided {
        entry_count: u32,
        entry_size: u32,
        field_offsets: Vec<u32>,
        literal_base: bool,
    },
    /// `quest_table` — multi-level category table (mhfinf.bin).
    QuestTable {
        count_base_pointer: u32,
        count_offset: u32,
        count_type: CountType,
        count_adjust: i32,
        quest_text_offset: u32,
        text_pointers_count: u32,
    },
    /// `scan_region` — walk every 4-byte slot in [begin, end) and
    /// keep slots whose target is a clean Shift-JIS string start.
    ScanRegion {
        next_field_pointer: u32,
        min_length: usize,
        max_length: usize,
        dedupe: bool,
    },
}

#[derive(Debug, Clone, Copy)]
pub enum CountType {
    U16,
    U32,
}

// ── headers.json walking ──────────────────────────────────────────────────────

fn root() -> Result<&'static Value> {
    use std::sync::OnceLock;
    static ROOT: OnceLock<Value> = OnceLock::new();
    if let Some(v) = ROOT.get() {
        return Ok(v);
    }
    let parsed: Value =
        serde_json::from_str(HEADERS_JSON).context("vendored headers.json is invalid")?;
    let _ = ROOT.set(parsed);
    Ok(ROOT.get().unwrap())
}

/// Look up a section config by xpath (e.g. `"dat/armors/head"`).
pub fn lookup(xpath: &str) -> Result<SectionConfig> {
    let mut node = root()?;
    for part in xpath.split('/') {
        node = node
            .get(part)
            .ok_or_else(|| anyhow!("xpath '{xpath}' not found in headers.json (at '{part}')"))?;
    }
    parse_config(xpath, node)
}

fn parse_hex(v: &Value, field: &str) -> Result<u32> {
    let s = v
        .as_str()
        .ok_or_else(|| anyhow!("'{field}' must be a hex string"))?;
    let s = s.trim_start_matches("0x").trim_start_matches("0X");
    u32::from_str_radix(s, 16).with_context(|| format!("invalid hex in '{field}': {s}"))
}

fn parse_config(xpath: &str, node: &Value) -> Result<SectionConfig> {
    let obj = node
        .as_object()
        .ok_or_else(|| anyhow!("xpath '{xpath}' is not a section config"))?;

    let begin_pointer = parse_hex(
        obj.get("begin_pointer")
            .ok_or_else(|| anyhow!("xpath '{xpath}' missing begin_pointer"))?,
        "begin_pointer",
    )?;

    let count_type = match obj.get("count_type").and_then(|v| v.as_str()) {
        Some("u32") => CountType::U32,
        _ => CountType::U16,
    };
    let count_adjust = obj
        .get("count_adjust")
        .and_then(|v| v.as_i64())
        .unwrap_or(0) as i32;

    let field_offsets: Vec<u32> = match obj.get("field_offset") {
        None => vec![0],
        Some(Value::Number(n)) => vec![n.as_u64().unwrap_or(0) as u32],
        Some(Value::Array(a)) => a.iter().map(|v| v.as_u64().unwrap_or(0) as u32).collect(),
        _ => bail!("'{xpath}' has invalid field_offset"),
    };

    let mode = if obj.get("scan_region").and_then(|v| v.as_bool()) == Some(true) {
        let next_field_pointer = parse_hex(
            obj.get("next_field_pointer")
                .ok_or_else(|| anyhow!("scan_region requires next_field_pointer"))?,
            "next_field_pointer",
        )?;
        Mode::ScanRegion {
            next_field_pointer,
            min_length: obj.get("min_length").and_then(|v| v.as_u64()).unwrap_or(4) as usize,
            max_length: obj
                .get("max_length")
                .and_then(|v| v.as_u64())
                .unwrap_or(400) as usize,
            dedupe: obj.get("dedupe").and_then(|v| v.as_bool()).unwrap_or(true),
        }
    } else if let Some(nfp) = obj.get("next_field_pointer") {
        Mode::PointerPair {
            next_field_pointer: parse_hex(nfp, "next_field_pointer")?,
            crop_end: obj.get("crop_end").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
        }
    } else if let Some(cp) = obj.get("count_pointer") {
        Mode::CountPointer {
            count_pointer: parse_hex(cp, "count_pointer")?,
        }
    } else if obj.get("quest_table").and_then(|v| v.as_bool()) == Some(true) {
        Mode::QuestTable {
            count_base_pointer: parse_hex(
                obj.get("count_base_pointer")
                    .ok_or_else(|| anyhow!("quest_table requires count_base_pointer"))?,
                "count_base_pointer",
            )?,
            count_offset: parse_hex(
                obj.get("count_offset")
                    .ok_or_else(|| anyhow!("quest_table requires count_offset"))?,
                "count_offset",
            )?,
            count_type,
            count_adjust,
            quest_text_offset: obj
                .get("quest_text_offset")
                .map(|v| parse_hex(v, "quest_text_offset"))
                .transpose()?
                .unwrap_or(0x28),
            text_pointers_count: obj
                .get("text_pointers_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(8) as u32,
        }
    } else if obj.contains_key("count_base_pointer") && obj.contains_key("entry_size") {
        Mode::IndirectCountStrided {
            count_base_pointer: parse_hex(&obj["count_base_pointer"], "count_base_pointer")?,
            count_offset: parse_hex(&obj["count_offset"], "count_offset")?,
            count_type,
            count_adjust,
            entry_size: obj["entry_size"].as_u64().unwrap_or(0) as u32,
            field_offsets,
        }
    } else if obj.contains_key("count_base_pointer") {
        Mode::IndirectCountFlat {
            count_base_pointer: parse_hex(&obj["count_base_pointer"], "count_base_pointer")?,
            count_offset: parse_hex(&obj["count_offset"], "count_offset")?,
            count_type,
            count_adjust,
            pointers_per_entry: obj
                .get("pointers_per_entry")
                .and_then(|v| v.as_u64())
                .unwrap_or(1) as u32,
        }
    } else if obj.get("null_terminated").and_then(|v| v.as_bool()) == Some(true) {
        Mode::NullTerminated {
            pointers_per_entry: obj
                .get("pointers_per_entry")
                .and_then(|v| v.as_u64())
                .unwrap_or(1) as u32,
            grouped: obj
                .get("grouped_entries")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        }
    } else if obj.contains_key("entry_count") {
        Mode::StructStrided {
            entry_count: obj["entry_count"].as_u64().unwrap_or(0) as u32,
            entry_size: obj["entry_size"].as_u64().unwrap_or(0) as u32,
            field_offsets,
            literal_base: obj
                .get("literal_base")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        }
    } else {
        bail!(
            "unknown extraction config shape at '{xpath}': keys = {:?}",
            obj.keys().collect::<Vec<_>>()
        )
    };

    Ok(SectionConfig {
        begin_pointer,
        mode,
    })
}

// ── binary helpers ────────────────────────────────────────────────────────────

fn read_u32(data: &[u8], at: u32) -> Result<u32> {
    let at = at as usize;
    let end = at
        .checked_add(4)
        .ok_or_else(|| anyhow!("u32 read overflow at {at:#x}"))?;
    if end > data.len() {
        bail!("u32 read out of bounds at {at:#x}");
    }
    Ok(u32::from_le_bytes([
        data[at],
        data[at + 1],
        data[at + 2],
        data[at + 3],
    ]))
}

fn read_u16(data: &[u8], at: u32) -> Result<u16> {
    let at = at as usize;
    if at + 2 > data.len() {
        bail!("u16 read out of bounds at {at:#x}");
    }
    Ok(u16::from_le_bytes([data[at], data[at + 1]]))
}

fn read_indirect_count(
    data: &[u8],
    base_ptr: u32,
    offset: u32,
    ty: CountType,
    adjust: i32,
) -> Result<u32> {
    let base = read_u32(data, base_ptr)?;
    let addr = base
        .checked_add(offset)
        .ok_or_else(|| anyhow!("count addr overflow"))?;
    let raw = match ty {
        CountType::U16 => read_u16(data, addr)? as u32,
        CountType::U32 => read_u32(data, addr)?,
    };
    Ok(((raw as i64) + adjust as i64).max(0) as u32)
}

// ── extraction ────────────────────────────────────────────────────────────────

/// Extract the entry list for a section from raw (decrypted/decompressed)
/// binary data. Output order matches FTH's `extract_text_data_from_bytes`.
pub fn extract(data: &[u8], cfg: &SectionConfig) -> Result<Vec<EntryOffsets>> {
    match &cfg.mode {
        Mode::PointerPair {
            next_field_pointer,
            crop_end,
        } => {
            let start = read_u32(data, cfg.begin_pointer)?;
            let end = read_u32(data, *next_field_pointer)?;
            let length = (end as i64) - (start as i64) - (*crop_end as i64);
            if length <= 0 {
                return Ok(vec![]);
            }
            read_flat_pointer_table(data, start, length as u32)
        }
        Mode::CountPointer { count_pointer } => {
            let start = read_u32(data, cfg.begin_pointer)?;
            let count = read_u32(data, *count_pointer)?;
            if count == 0 {
                return Ok(vec![]);
            }
            read_flat_pointer_table(data, start, count * 4)
        }
        Mode::IndirectCountFlat {
            count_base_pointer,
            count_offset,
            count_type,
            count_adjust,
            pointers_per_entry,
        } => {
            let count = read_indirect_count(
                data,
                *count_base_pointer,
                *count_offset,
                *count_type,
                *count_adjust,
            )?;
            if count == 0 {
                return Ok(vec![]);
            }
            let start = read_u32(data, cfg.begin_pointer)?;
            read_flat_pointer_table(data, start, count * pointers_per_entry * 4)
        }
        Mode::IndirectCountStrided {
            count_base_pointer,
            count_offset,
            count_type,
            count_adjust,
            entry_size,
            field_offsets,
        } => {
            let count = read_indirect_count(
                data,
                *count_base_pointer,
                *count_offset,
                *count_type,
                *count_adjust,
            )?;
            if count == 0 {
                return Ok(vec![]);
            }
            let base = read_u32(data, cfg.begin_pointer)?;
            read_struct_strided(data, base, count, *entry_size, field_offsets)
        }
        Mode::NullTerminated {
            pointers_per_entry,
            grouped,
        } => {
            let start = read_u32(data, cfg.begin_pointer)?;
            if *grouped && *pointers_per_entry > 1 {
                read_multi_pointer_entries(data, start, *pointers_per_entry)
            } else {
                // Walk forward in groups of (ppe*4) bytes until first ptr of group is 0.
                let group = pointers_per_entry * 4;
                let mut pos = start;
                loop {
                    let first = read_u32(data, pos)?;
                    if first == 0 {
                        break;
                    }
                    pos = pos
                        .checked_add(group)
                        .ok_or_else(|| anyhow!("ptr overflow"))?;
                }
                let length = pos - start;
                if length == 0 {
                    return Ok(vec![]);
                }
                read_flat_pointer_table(data, start, length)
            }
        }
        Mode::StructStrided {
            entry_count,
            entry_size,
            field_offsets,
            literal_base,
        } => {
            let base = if *literal_base {
                cfg.begin_pointer
            } else {
                read_u32(data, cfg.begin_pointer)?
            };
            read_struct_strided(data, base, *entry_count, *entry_size, field_offsets)
        }
        Mode::QuestTable {
            count_base_pointer,
            count_offset,
            count_type,
            count_adjust,
            quest_text_offset,
            text_pointers_count,
        } => {
            let count = read_indirect_count(
                data,
                *count_base_pointer,
                *count_offset,
                *count_type,
                *count_adjust,
            )?;
            if count == 0 {
                return Ok(vec![]);
            }
            let cat_table = read_u32(data, cfg.begin_pointer)?;
            read_quest_table(
                data,
                cat_table,
                count,
                *quest_text_offset,
                *text_pointers_count,
            )
        }
        Mode::ScanRegion {
            next_field_pointer,
            min_length,
            max_length,
            dedupe,
        } => {
            let region_start = read_u32(data, cfg.begin_pointer)?;
            let region_end = read_u32(data, *next_field_pointer)?;
            scan_region(
                data,
                region_start,
                region_end,
                *min_length,
                *max_length,
                *dedupe,
            )
        }
    }
}

/// Read a flat block of u32 pointers and group consecutive non-zero
/// pointers between zero terminators (when any zero is present) into
/// joined entries — matches FTH's `read_file_section`.
fn read_flat_pointer_table(data: &[u8], start: u32, length: u32) -> Result<Vec<EntryOffsets>> {
    if length == 0 || !length.is_multiple_of(4) {
        if length == 0 {
            return Ok(vec![]);
        }
        bail!("flat pointer table length {length} is not 4-aligned");
    }
    let n = (length / 4) as usize;
    let mut pointers = Vec::with_capacity(n);
    for i in 0..n {
        pointers.push(read_u32(data, start + (i as u32) * 4)?);
    }
    let join_lines = pointers.contains(&0);

    let mut out: Vec<EntryOffsets> = Vec::new();
    let mut current_id: i32 = if join_lines { 0 } else { -1 };
    let mut last_id: i32 = -1;
    for (i, &ptr) in pointers.iter().enumerate() {
        let slot = start + (i as u32) * 4;
        if join_lines {
            if ptr == 0 {
                current_id += 1;
                continue;
            }
        } else {
            current_id += 1;
        }
        // Pointer must be in-bounds (FTH validates each before seeking).
        if ptr as usize >= data.len() {
            bail!("pointer {ptr:#x} at slot {slot:#x} is out of bounds");
        }
        if current_id == last_id {
            out.last_mut().unwrap().offsets.push(slot);
        } else {
            out.push(EntryOffsets {
                offsets: vec![slot],
            });
            last_id = current_id;
        }
    }
    Ok(out)
}

/// Port of `read_multi_pointer_entries`: fixed-size groups, terminator
/// is a group whose first pointer is 0; null inner pointers are skipped.
fn read_multi_pointer_entries(data: &[u8], start: u32, ppe: u32) -> Result<Vec<EntryOffsets>> {
    let mut out = Vec::new();
    let mut pos = start;
    loop {
        let first = read_u32(data, pos)?;
        if first == 0 {
            break;
        }
        let mut entry: Option<EntryOffsets> = None;
        for i in 0..ppe {
            let slot = pos + i * 4;
            let ptr = read_u32(data, slot)?;
            if ptr == 0 {
                continue;
            }
            if (ptr as usize) >= data.len() {
                bail!("pointer {ptr:#x} at slot {slot:#x} oob");
            }
            match &mut entry {
                None => {
                    entry = Some(EntryOffsets {
                        offsets: vec![slot],
                    })
                }
                Some(e) => e.offsets.push(slot),
            }
        }
        if let Some(e) = entry {
            out.push(e);
        }
        pos = pos
            .checked_add(ppe * 4)
            .ok_or_else(|| anyhow!("overflow"))?;
    }
    Ok(out)
}

/// Port of `read_struct_strings` — one entry per (struct, field).
fn read_struct_strided(
    data: &[u8],
    base: u32,
    count: u32,
    entry_size: u32,
    field_offsets: &[u32],
) -> Result<Vec<EntryOffsets>> {
    let mut out = Vec::new();
    for i in 0..count {
        let entry_base = base + i * entry_size;
        for &fo in field_offsets {
            let slot = entry_base + fo;
            let ptr = read_u32(data, slot)?;
            if ptr == 0 {
                continue;
            }
            if (ptr as usize) >= data.len() {
                bail!("pointer {ptr:#x} at slot {slot:#x} oob");
            }
            out.push(EntryOffsets {
                offsets: vec![slot],
            });
        }
    }
    Ok(out)
}

/// Port of `read_quest_table` — multi-level mhfinf.bin walker.
fn read_quest_table(
    data: &[u8],
    cat_table_ptr: u32,
    num_categories: u32,
    quest_text_offset: u32,
    text_pointers_count: u32,
) -> Result<Vec<EntryOffsets>> {
    let mut out = Vec::new();
    for cat_idx in 0..num_categories {
        let cat_addr = cat_table_ptr + cat_idx * 8;
        let count = read_u16(data, cat_addr + 2)? as u32;
        let quest_array_ptr = read_u32(data, cat_addr + 4)?;
        if quest_array_ptr == 0 || count == 0 {
            continue;
        }
        let mut quest_ptrs = Vec::with_capacity(count as usize);
        for j in 0..count {
            quest_ptrs.push(read_u32(data, quest_array_ptr + j * 4)?);
        }
        for quest_ptr in quest_ptrs {
            if quest_ptr == 0 {
                continue;
            }
            let text_block_ptr = read_u32(data, quest_ptr + quest_text_offset)?;
            if text_block_ptr == 0 {
                continue;
            }
            let mut entry: Option<EntryOffsets> = None;
            for i in 0..text_pointers_count {
                let slot = text_block_ptr + i * 4;
                let sp = read_u32(data, slot)?;
                if sp == 0 {
                    continue;
                }
                if (sp as usize) >= data.len() {
                    bail!("quest string ptr {sp:#x} oob");
                }
                match &mut entry {
                    None => {
                        entry = Some(EntryOffsets {
                            offsets: vec![slot],
                        })
                    }
                    Some(e) => e.offsets.push(slot),
                }
            }
            if let Some(e) = entry {
                out.push(e);
            }
        }
    }
    Ok(out)
}

/// Port of `scan_region_for_strings`. Mirrors all six FTH filters so the
/// produced offset list is bit-identical for `gao/situational_dialogue`.
fn scan_region(
    data: &[u8],
    region_start: u32,
    region_end: u32,
    min_length: usize,
    max_length: usize,
    dedupe: bool,
) -> Result<Vec<EntryOffsets>> {
    if region_end <= region_start {
        return Ok(vec![]);
    }
    let file_size = data.len() as u32;
    let mut out = Vec::new();
    let mut seen: std::collections::HashSet<u32> = Default::default();

    let mut slot = region_start;
    while slot + 4 <= region_end {
        let pointer = read_u32(data, slot)?;
        // Filter 1: null / OOB
        if pointer == 0 || pointer >= file_size {
            slot += 4;
            continue;
        }
        // Filter 2: mid-multibyte (preceding byte is SJIS lead byte)
        if pointer > 0 {
            let prev = data[(pointer - 1) as usize];
            if (0x81..=0x9F).contains(&prev) || (0xE0..=0xFC).contains(&prev) {
                slot += 4;
                continue;
            }
        }
        // Filter 3: read null-terminated span and length-bound it
        let start = pointer as usize;
        let mut end = start;
        while end < data.len() && data[end] != 0 {
            end += 1;
        }
        let raw = &data[start..end];
        if raw.len() < min_length || raw.len() > max_length {
            slot += 4;
            continue;
        }
        // Filter 4 & 5: must decode cleanly as Shift-JIS, no U+FFFD
        let (text, _, had_errors) = encoding_rs::SHIFT_JIS.decode(raw);
        if had_errors || text.is_empty() || text.contains('\u{FFFD}') {
            slot += 4;
            continue;
        }
        // Filter 6: first char must not be a C0 control (except \n, \t)
        let first = text.chars().next().unwrap();
        if (first as u32) < 0x20 && first != '\n' && first != '\t' {
            slot += 4;
            continue;
        }
        if dedupe && !seen.insert(pointer) {
            slot += 4;
            continue;
        }
        out.push(EntryOffsets {
            offsets: vec![slot],
        });
        slot += 4;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_loads() {
        // Sanity: every xpath used by the v0.1.0 release JSON parses.
        for x in [
            "dat/armors/head",
            "dat/weapons/melee/name",
            "dat/weapons/ranged/description",
            "dat/items/source",
            "dat/equipment/description",
            "dat/ranks/label",
            "pac/skills/name",
            "pac/text_14",
            "pac/text_48/field_0",
            "pac/text_94/field_1",
            "inf/quests",
            "jmp/menu/title",
            "jmp/strings",
            "gao/armor_helm",
            "gao/armor_mail",
            "gao/situational_dialogue",
            "rcc/events_full",
            "msx/item_name",
        ] {
            lookup(x).unwrap_or_else(|e| panic!("{x}: {e}"));
        }
    }

    /// Build a tiny binary with a 3-entry pointer pair table at 0x40 →
    /// pointer table at 0x100..0x10C → strings at 0x200,0x208,0x210.
    /// `next_field_pointer` at 0x44 → 0x10C.
    fn fixture_pointer_pair() -> (Vec<u8>, SectionConfig) {
        let mut data = vec![0u8; 0x300];
        // begin_pointer @ 0x40 = 0x100
        data[0x40..0x44].copy_from_slice(&0x100u32.to_le_bytes());
        // next_field_pointer @ 0x44 = 0x10C
        data[0x44..0x48].copy_from_slice(&0x10Cu32.to_le_bytes());
        // pointer table
        for (i, p) in [0x200u32, 0x208, 0x210].iter().enumerate() {
            let off = 0x100 + i * 4;
            data[off..off + 4].copy_from_slice(&p.to_le_bytes());
        }
        // strings (anything non-null + null terminator)
        data[0x200..0x205].copy_from_slice(b"abc\0\0");
        data[0x208..0x20D].copy_from_slice(b"def\0\0");
        data[0x210..0x215].copy_from_slice(b"ghi\0\0");

        let cfg = SectionConfig {
            begin_pointer: 0x40,
            mode: Mode::PointerPair {
                next_field_pointer: 0x44,
                crop_end: 0,
            },
        };
        (data, cfg)
    }

    #[test]
    fn pointer_pair_extracts_three_entries() {
        let (data, cfg) = fixture_pointer_pair();
        let entries = extract(&data, &cfg).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].offsets, vec![0x100]);
        assert_eq!(entries[1].offsets, vec![0x104]);
        assert_eq!(entries[2].offsets, vec![0x108]);
    }

    #[test]
    fn struct_strided_multi_field() {
        // 2 structs of 8 bytes at literal 0x40, two string ptr fields per struct.
        let mut data = vec![0u8; 0x200];
        for (i, p) in [0x80u32, 0x88, 0x90, 0x98].iter().enumerate() {
            let off = 0x40 + i * 4;
            data[off..off + 4].copy_from_slice(&p.to_le_bytes());
            data[*p as usize..*p as usize + 2].copy_from_slice(b"x\0");
        }
        let cfg = SectionConfig {
            begin_pointer: 0x40,
            mode: Mode::StructStrided {
                entry_count: 2,
                entry_size: 8,
                field_offsets: vec![0, 4],
                literal_base: true,
            },
        };
        let entries = extract(&data, &cfg).unwrap();
        // multi-field is entry-major: e0.f0, e0.f1, e1.f0, e1.f1
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].offsets, vec![0x40]);
        assert_eq!(entries[1].offsets, vec![0x44]);
        assert_eq!(entries[2].offsets, vec![0x48]);
        assert_eq!(entries[3].offsets, vec![0x4C]);
    }

    #[test]
    fn null_terminated_grouped_joins() {
        // 2 entries of 3 ptrs each, then a null terminator group.
        // Entry 0: [a, b, 0]  → 2 join slots
        // Entry 1: [c, 0, d]  → 2 join slots (skipping null inner ptr)
        // Term:    [0, …]
        let mut data = vec![0u8; 0x200];
        // begin_pointer @ 0x10 = 0x40
        data[0x10..0x14].copy_from_slice(&0x40u32.to_le_bytes());
        let table = [0x80u32, 0x84, 0, 0x88, 0, 0x8C, 0, 0, 0];
        for (i, p) in table.iter().enumerate() {
            let off = 0x40 + i * 4;
            data[off..off + 4].copy_from_slice(&p.to_le_bytes());
        }
        for &p in &[0x80u32, 0x84, 0x88, 0x8C] {
            data[p as usize] = b'x';
        }
        let cfg = SectionConfig {
            begin_pointer: 0x10,
            mode: Mode::NullTerminated {
                pointers_per_entry: 3,
                grouped: true,
            },
        };
        let entries = extract(&data, &cfg).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].offsets, vec![0x40, 0x44]);
        assert_eq!(entries[1].offsets, vec![0x4C, 0x54]);
    }
}
