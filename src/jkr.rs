//! JKR (JPK) compression and decompression for MHF game files.
//!
//! Supports all decompression types (RW, NONE, HFIRW, LZ, HFI) and HFI
//! compression for producing game-ready files.

use anyhow::{bail, Result};

const JKR_MAGIC: u32 = 0x1A524B4A;
const JKR_HEADER_SIZE: usize = 16;

// Compression types
const TYPE_RW: u16 = 0;
const TYPE_NONE: u16 = 1;
const TYPE_HFIRW: u16 = 2;
const TYPE_LZ: u16 = 3;
const TYPE_HFI: u16 = 4;

fn read_u32_le(data: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
}

fn read_u16_le(data: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([data[off], data[off + 1]])
}

fn read_i16_le(data: &[u8], off: usize) -> i16 {
    i16::from_le_bytes([data[off], data[off + 1]])
}

/// Returns `true` if `data` starts with the JKR magic bytes.
pub fn is_jkr(data: &[u8]) -> bool {
    data.len() >= 4 && read_u32_le(data, 0) == JKR_MAGIC
}

/// Decompress a JKR-wrapped buffer. Returns the decompressed data.
pub fn decompress_jkr(data: &[u8]) -> Result<Vec<u8>> {
    if data.len() < JKR_HEADER_SIZE {
        bail!("JKR data too short");
    }
    if read_u32_le(data, 0) != JKR_MAGIC {
        bail!("not a JKR file");
    }

    let _version = read_u16_le(data, 4);
    let comp_type = read_u16_le(data, 6);
    let data_offset = read_u32_le(data, 8) as usize;
    let decompressed_size = read_u32_le(data, 12) as usize;

    let payload = if data_offset <= data.len() {
        &data[data_offset..]
    } else {
        bail!("JKR data_offset ({data_offset}) exceeds data length ({})", data.len());
    };

    match comp_type {
        TYPE_NONE => {
            // Uncompressed - just copy.
            Ok(payload[..decompressed_size.min(payload.len())].to_vec())
        }
        TYPE_RW => {
            // RW is same as LZ.
            lz_decode(payload, decompressed_size)
        }
        TYPE_LZ => lz_decode(payload, decompressed_size),
        TYPE_HFIRW | TYPE_HFI => hfi_decode(payload, decompressed_size),
        _ => bail!("unknown JKR compression type {comp_type}"),
    }
}

// ── LZ77 decoder ─────────────────────────────────────────────────────────────

struct LzBitReader<'a> {
    data: &'a [u8],
    pos: usize,
    flag_byte: u8,
    shift_index: i32,
}

impl<'a> LzBitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            flag_byte: 0,
            shift_index: 0,
        }
    }

    fn read_bit(&mut self) -> Result<u8> {
        if self.shift_index <= 0 {
            if self.pos >= self.data.len() {
                bail!("LZ bit reader: unexpected end of data");
            }
            self.flag_byte = self.data[self.pos];
            self.pos += 1;
            self.shift_index = 8;
        }
        self.shift_index -= 1;
        let bit = (self.flag_byte >> self.shift_index) & 1;
        Ok(bit)
    }

    fn read_byte(&mut self) -> Result<u8> {
        if self.pos >= self.data.len() {
            bail!("LZ reader: unexpected end of data");
        }
        let b = self.data[self.pos];
        self.pos += 1;
        Ok(b)
    }
}

fn lz_decode(data: &[u8], out_size: usize) -> Result<Vec<u8>> {
    let mut reader = LzBitReader::new(data);
    let mut output = vec![0u8; out_size];
    let mut out_idx = 0;

    while out_idx < out_size {
        let bit = reader.read_bit()?;
        if bit == 0 {
            // Literal byte
            output[out_idx] = reader.read_byte()?;
            out_idx += 1;
        } else {
            let bit2 = reader.read_bit()?;
            if bit2 == 0 {
                // Case 0: short back-reference
                let b0 = reader.read_bit()?;
                let b1 = reader.read_bit()?;
                let length = ((b0 as usize) << 1 | b1 as usize) + 3;
                let offset_byte = reader.read_byte()? as usize;
                let src = out_idx.wrapping_sub(offset_byte + 1);
                for j in 0..length {
                    if out_idx >= out_size {
                        break;
                    }
                    output[out_idx] = output[src + j];
                    out_idx += 1;
                }
            } else {
                // Read hi, lo data bytes
                let hi = reader.read_byte()? as usize;
                let lo = reader.read_byte()? as usize;
                let length_raw = (hi & 0xE0) >> 5;
                let offset = ((hi & 0x1F) << 8) | lo;

                if length_raw != 0 {
                    // Case 1: medium back-reference
                    let length = length_raw + 2;
                    let src = out_idx.wrapping_sub(offset + 1);
                    for j in 0..length {
                        if out_idx >= out_size {
                            break;
                        }
                        output[out_idx] = output[src + j];
                        out_idx += 1;
                    }
                } else {
                    let bit3 = reader.read_bit()?;
                    if bit3 == 0 {
                        // Case 2: read 4 bits as length
                        let mut nibble = 0usize;
                        for _ in 0..4 {
                            nibble = (nibble << 1) | reader.read_bit()? as usize;
                        }
                        let length = nibble + 10;
                        let src = out_idx.wrapping_sub(offset + 1);
                        for j in 0..length {
                            if out_idx >= out_size {
                                break;
                            }
                            output[out_idx] = output[src + j];
                            out_idx += 1;
                        }
                    } else {
                        // Read temp byte
                        let temp = reader.read_byte()?;
                        if temp == 0xFF {
                            // Case 4: literal run
                            let run_len = offset + 27;
                            for _ in 0..run_len {
                                if out_idx >= out_size {
                                    break;
                                }
                                output[out_idx] = reader.read_byte()?;
                                out_idx += 1;
                            }
                        } else {
                            // Case 3: long back-reference
                            let length = temp as usize + 26;
                            let src = out_idx.wrapping_sub(offset + 1);
                            for j in 0..length {
                                if out_idx >= out_size {
                                    break;
                                }
                                output[out_idx] = output[src + j];
                                out_idx += 1;
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(output)
}

// ── HFI decoder (Huffman + LZ) ──────────────────────────────────────────────

fn hfi_decode(data: &[u8], out_size: usize) -> Result<Vec<u8>> {
    if data.len() < 2 {
        bail!("HFI data too short for table length");
    }
    let table_len = read_i16_le(data, 0) as i32;
    let hf_table_offset: usize = 2;
    let hf_data_offset = hf_table_offset as i64 + (table_len as i64) * 4 - 0x3FC;
    if hf_data_offset < 0 {
        bail!("HFI: computed data offset is negative");
    }

    let mut hfi = HfiReader {
        data,
        hf_table_offset,
        hf_data_offset: hf_data_offset as usize,
        hf_bit_pos: 0,
        hf_bit_byte: 0,
        hf_bit_shift: 0,
        table_len,
    };

    // The HFI reader feeds bytes into the LZ decoder.
    let mut lz_data = Vec::new();
    // We need to read enough bytes for the LZ decoder. The LZ compressed
    // data length isn't known ahead of time, so we read all available.
    let available = data.len().saturating_sub(hfi.hf_data_offset);
    // Estimate: LZ data can't be bigger than the huffman-coded stream.
    // We'll read bytes on demand via a custom approach.

    // Actually, we need to integrate the huffman reader into the LZ bit reader.
    // Let's decode the full huffman stream first, then LZ-decode it.
    // Upper bound: LZ compressed data is at most out_size * 2 (generous).
    let max_bytes = available * 8; // each huffman symbol is at least 1 bit
    let limit = max_bytes.min(out_size * 4 + 1024);

    for _ in 0..limit {
        match hfi.read_hf_byte() {
            Ok(b) => lz_data.push(b),
            Err(_) => break,
        }
    }

    lz_decode(&lz_data, out_size)
}

struct HfiReader<'a> {
    data: &'a [u8],
    hf_table_offset: usize,
    hf_data_offset: usize,
    hf_bit_pos: usize, // bit position in the data stream
    hf_bit_byte: u8,
    hf_bit_shift: i32,
    table_len: i32,
}

impl<'a> HfiReader<'a> {
    fn read_hf_bit(&mut self) -> Result<u8> {
        if self.hf_bit_shift <= 0 {
            let byte_pos = self.hf_data_offset + self.hf_bit_pos / 8;
            if byte_pos >= self.data.len() {
                bail!("HFI: end of huffman bitstream");
            }
            self.hf_bit_byte = self.data[byte_pos];
            self.hf_bit_shift = 8;
        }
        self.hf_bit_shift -= 1;
        let bit = (self.hf_bit_byte >> self.hf_bit_shift) & 1;
        self.hf_bit_pos += 1;
        Ok(bit)
    }

    fn read_hf_byte(&mut self) -> Result<u8> {
        let mut node = self.table_len;
        while node >= 0x100 {
            let bit = self.read_hf_bit()? as i32;
            let table_idx = ((node * 2 - 0x200 + bit) * 2) as usize + self.hf_table_offset;
            if table_idx + 1 >= self.data.len() {
                bail!("HFI: table index out of bounds");
            }
            node = read_i16_le(self.data, table_idx) as i32;
        }
        Ok((node & 0xFF) as u8)
    }
}

// ── LZ77 encoder ─────────────────────────────────────────────────────────────

const WINDOW_SIZE: usize = 8192;
const MIN_MATCH: usize = 3;
const MAX_MATCH_SHORT: usize = 6; // Case 0: 3-6
const MAX_MATCH_MEDIUM: usize = 9; // Case 1: 3-9 (length_raw 1-7 → +2)
const MAX_MATCH_NIBBLE: usize = 25; // Case 2: 10-25
const MAX_MATCH_LONG: usize = 254 + 26; // Case 3: 26-280 (temp byte 0xFF is reserved for Case 4)

/// Hash chain-based LZ77 match finder.
struct MatchFinder {
    head: Vec<i32>,       // hash → most recent position
    prev: Vec<i32>,       // position → previous position with same hash
}

impl MatchFinder {
    fn new() -> Self {
        Self {
            head: vec![-1; 65536],
            prev: vec![-1; WINDOW_SIZE],
        }
    }

    fn hash(data: &[u8], pos: usize) -> usize {
        if pos + 2 >= data.len() {
            return 0;
        }
        let h = (data[pos] as usize) << 8 ^ (data[pos + 1] as usize) << 4 ^ data[pos + 2] as usize;
        h & 0xFFFF
    }

    fn insert(&mut self, data: &[u8], pos: usize) {
        let h = Self::hash(data, pos);
        self.prev[pos % WINDOW_SIZE] = self.head[h];
        self.head[h] = pos as i32;
    }

    fn find_match(&self, data: &[u8], pos: usize) -> (usize, usize) {
        // Returns (offset, length) where offset is distance back - 1.
        if pos + MIN_MATCH > data.len() {
            return (0, 0);
        }
        let h = Self::hash(data, pos);
        let mut candidate = self.head[h];
        let mut best_len = 0usize;
        let mut best_offset = 0usize;
        let max_len = MAX_MATCH_LONG.min(data.len() - pos);
        let min_pos = if pos >= WINDOW_SIZE { pos - WINDOW_SIZE + 1 } else { 0 };
        let mut chain = 0;

        while candidate >= min_pos as i32 && candidate >= 0 && chain < 256 {
            let cpos = candidate as usize;
            if cpos < pos {
                // Check match length.
                let mut len = 0;
                while len < max_len && data[cpos + len] == data[pos + len] {
                    len += 1;
                }
                if len >= MIN_MATCH && len > best_len {
                    best_len = len;
                    best_offset = pos - cpos - 1;
                    if len == max_len {
                        break;
                    }
                }
            }
            candidate = self.prev[cpos % WINDOW_SIZE];
            chain += 1;
        }

        if best_len < MIN_MATCH {
            (0, 0)
        } else {
            (best_offset, best_len)
        }
    }
}

struct LzWriter {
    output: Vec<u8>,
    flag_byte: u8,
    flag_bits: u8,
    flag_pos: usize, // position of current flag byte in output
    data_buf: Vec<u8>, // data bytes for current flag group
}

impl LzWriter {
    fn new() -> Self {
        let mut w = Self {
            output: Vec::new(),
            flag_byte: 0,
            flag_bits: 0,
            flag_pos: 0,
            data_buf: Vec::new(),
        };
        // Reserve space for first flag byte.
        w.flag_pos = 0;
        w.output.push(0);
        w
    }

    fn write_bit(&mut self, bit: u8) {
        if self.flag_bits == 8 {
            self.flush_flag();
        }
        self.flag_byte = (self.flag_byte << 1) | (bit & 1);
        self.flag_bits += 1;
    }

    fn write_byte(&mut self, b: u8) {
        self.data_buf.push(b);
    }

    fn flush_flag(&mut self) {
        // Shift remaining bits to MSB position.
        // flag_bits should be 8 here normally.
        self.output[self.flag_pos] = self.flag_byte;
        self.output.extend_from_slice(&self.data_buf);
        self.data_buf.clear();
        self.flag_byte = 0;
        self.flag_bits = 0;
        self.flag_pos = self.output.len();
        self.output.push(0); // placeholder for next flag byte
    }

    fn finish(mut self) -> Vec<u8> {
        if self.flag_bits > 0 {
            // Shift remaining bits left so they're MSB-aligned.
            self.flag_byte <<= 8 - self.flag_bits;
            self.output[self.flag_pos] = self.flag_byte;
            self.output.extend_from_slice(&self.data_buf);
        } else {
            // Remove unused flag byte placeholder.
            self.output.pop();
        }
        self.output
    }
}

fn lz_encode(data: &[u8]) -> Vec<u8> {
    let mut writer = LzWriter::new();
    let mut finder = MatchFinder::new();
    let mut pos = 0;

    while pos < data.len() {
        let (offset, length) = finder.find_match(data, pos);

        if length < MIN_MATCH {
            // Literal byte: bit 0
            writer.write_bit(0);
            writer.write_byte(data[pos]);
            finder.insert(data, pos);
            pos += 1;
        } else if length <= MAX_MATCH_SHORT && offset <= 255 {
            // Case 0: bits 1,0 + 2 length bits + 1 offset byte
            writer.write_bit(1);
            writer.write_bit(0);
            let len_code = (length - 3) as u8;
            writer.write_bit((len_code >> 1) & 1);
            writer.write_bit(len_code & 1);
            writer.write_byte(offset as u8);
            for i in 0..length {
                finder.insert(data, pos + i);
            }
            pos += length;
        } else if length <= MAX_MATCH_MEDIUM && offset <= 0x1FFF {
            // Case 1: bits 1,1 + hi,lo bytes
            writer.write_bit(1);
            writer.write_bit(1);
            let length_raw = (length - 2) as u8;
            let hi = ((length_raw & 0x07) << 5) | ((offset >> 8) & 0x1F) as u8;
            let lo = (offset & 0xFF) as u8;
            writer.write_byte(hi);
            writer.write_byte(lo);
            for i in 0..length {
                finder.insert(data, pos + i);
            }
            pos += length;
        } else if length >= 10 && length <= MAX_MATCH_NIBBLE && offset <= 0x1FFF {
            // Case 2: bits 1,1 + hi,lo (length_raw=0) + bit 0 + 4 nibble bits
            writer.write_bit(1);
            writer.write_bit(1);
            let hi = ((offset >> 8) & 0x1F) as u8;
            let lo = (offset & 0xFF) as u8;
            writer.write_byte(hi);
            writer.write_byte(lo);
            writer.write_bit(0);
            let nibble = (length - 10) as u8;
            writer.write_bit((nibble >> 3) & 1);
            writer.write_bit((nibble >> 2) & 1);
            writer.write_bit((nibble >> 1) & 1);
            writer.write_bit(nibble & 1);
            for i in 0..length {
                finder.insert(data, pos + i);
            }
            pos += length;
        } else if length >= 26 && offset <= 0x1FFF {
            // Case 3: bits 1,1 + hi,lo (length_raw=0) + bit 1 + temp byte
            let actual_len = length.min(MAX_MATCH_LONG);
            writer.write_bit(1);
            writer.write_bit(1);
            let hi = ((offset >> 8) & 0x1F) as u8;
            let lo = (offset & 0xFF) as u8;
            writer.write_byte(hi);
            writer.write_byte(lo);
            writer.write_bit(1);
            writer.write_byte((actual_len - 26) as u8);
            for i in 0..actual_len {
                finder.insert(data, pos + i);
            }
            pos += actual_len;
        } else {
            // Fallback: emit as literal
            writer.write_bit(0);
            writer.write_byte(data[pos]);
            finder.insert(data, pos);
            pos += 1;
        }
    }

    writer.finish()
}

// ── Huffman encoder ──────────────────────────────────────────────────────────

struct HuffNode {
    freq: u64,
    symbol: Option<u16>, // leaf: 0-255, internal: None
    left: Option<usize>,
    right: Option<usize>,
}

fn huffman_encode(data: &[u8]) -> Vec<u8> {
    if data.is_empty() {
        // Minimal valid HFI: table_len=0x100, empty table, no data.
        let mut out = Vec::new();
        out.extend_from_slice(&0x100i16.to_le_bytes());
        return out;
    }

    // Count frequencies.
    let mut freq = [0u64; 256];
    for &b in data {
        freq[b as usize] += 1;
    }

    // Build Huffman tree using a priority queue (simple insertion sort).
    let mut nodes: Vec<HuffNode> = Vec::with_capacity(512);

    // Add leaf nodes for all symbols that appear.
    let mut active: Vec<usize> = Vec::new();
    for (i, &f) in freq.iter().enumerate() {
        if f > 0 {
            let idx = nodes.len();
            nodes.push(HuffNode {
                freq: f,
                symbol: Some(i as u16),
                left: None,
                right: None,
            });
            active.push(idx);
        }
    }

    // Handle edge case: only one unique symbol.
    if active.len() == 1 {
        let sym = nodes[active[0]].symbol.unwrap();
        // Build minimal tree: root (>=0x100) with one child.
        let dummy_sym = if sym == 0 { 1u16 } else { 0u16 };
        let dummy_idx = nodes.len();
        nodes.push(HuffNode {
            freq: 0,
            symbol: Some(dummy_sym),
            left: None,
            right: None,
        });
        active.push(dummy_idx);
    }

    // Sort by frequency ascending.
    active.sort_by_key(|&i| nodes[i].freq);

    // Build tree bottom-up.
    while active.len() > 1 {
        let left = active.remove(0);
        let right = active.remove(0);
        let combined_freq = nodes[left].freq + nodes[right].freq;
        let new_idx = nodes.len();
        nodes.push(HuffNode {
            freq: combined_freq,
            symbol: None,
            left: Some(left),
            right: Some(right),
        });
        // Insert maintaining sort order.
        let insert_pos = active
            .iter()
            .position(|&i| nodes[i].freq > combined_freq)
            .unwrap_or(active.len());
        active.insert(insert_pos, new_idx);
    }

    let root = active[0];

    // Generate codes for each symbol.
    let mut codes: [(u32, u8); 256] = [(0u32, 0u8); 256]; // (code, bit_length)
    fn assign_codes(
        nodes: &[HuffNode],
        idx: usize,
        code: u32,
        depth: u8,
        codes: &mut [(u32, u8); 256],
    ) {
        if let Some(sym) = nodes[idx].symbol {
            codes[sym as usize] = (code, depth);
        } else {
            if let Some(l) = nodes[idx].left {
                assign_codes(nodes, l, code << 1, depth + 1, codes);
            }
            if let Some(r) = nodes[idx].right {
                assign_codes(nodes, r, (code << 1) | 1, depth + 1, codes);
            }
        }
    }
    assign_codes(&nodes, root, 0, 0, &mut codes);

    // Build the HFI table format:
    // The table is indexed by node_id. For internal nodes (id >= 0x100),
    // entry at (id*2 - 0x200 + bit)*2 gives child id as i16.
    // We need to assign IDs to nodes.
    // Leaves get their symbol value (0-255).
    // Internal nodes get IDs starting at 0x100.

    let mut node_ids = vec![0i32; nodes.len()];
    let mut next_internal_id: i32 = 0x100;

    // Assign IDs: leaves = symbol, internal = 0x100+
    for (i, node) in nodes.iter().enumerate() {
        if let Some(sym) = node.symbol {
            node_ids[i] = sym as i32;
        } else {
            node_ids[i] = next_internal_id;
            next_internal_id += 1;
        }
    }

    let table_len = node_ids[root] as i16;
    // Table needs entries for each internal node.
    // Each internal node with id N has 2 entries at index (N*2 - 0x200)*2 and (N*2 - 0x200 + 1)*2.
    // The table base is at offset 2 (after table_len i16).
    // Entry offset = (id*2 - 0x200 + bit)*2
    // Max entry offset = (max_id*2 - 0x200 + 1)*2
    let max_id = next_internal_id - 1;
    let table_entries = if max_id >= 0x100 {
        ((max_id * 2 - 0x200 + 1) + 1) as usize
    } else {
        2 // minimal
    };
    let table_byte_size = table_entries * 2;

    let mut output = Vec::new();
    output.extend_from_slice(&table_len.to_le_bytes());
    let table_start = output.len();
    output.resize(table_start + table_byte_size, 0);

    // Fill table entries for internal nodes.
    for (i, node) in nodes.iter().enumerate() {
        if node.symbol.is_none() {
            let id = node_ids[i];
            if let Some(left) = node.left {
                let entry_offset = ((id * 2 - 0x200) * 2) as usize;
                let child_id = node_ids[left] as i16;
                let abs = table_start + entry_offset;
                if abs + 1 < output.len() {
                    output[abs] = child_id as u8;
                    output[abs + 1] = (child_id >> 8) as u8;
                }
            }
            if let Some(right) = node.right {
                let entry_offset = ((id * 2 - 0x200 + 1) * 2) as usize;
                let child_id = node_ids[right] as i16;
                let abs = table_start + entry_offset;
                if abs + 1 < output.len() {
                    output[abs] = child_id as u8;
                    output[abs + 1] = (child_id >> 8) as u8;
                }
            }
        }
    }

    // Now encode the data as a bit stream.
    let mut bit_buf: u8 = 0;
    let mut bit_count: u8 = 0;
    let mut bit_output: Vec<u8> = Vec::new();

    for &b in data {
        let (code, len) = codes[b as usize];
        for i in (0..len).rev() {
            bit_buf = (bit_buf << 1) | ((code >> i) & 1) as u8;
            bit_count += 1;
            if bit_count == 8 {
                bit_output.push(bit_buf);
                bit_buf = 0;
                bit_count = 0;
            }
        }
    }
    if bit_count > 0 {
        bit_buf <<= 8 - bit_count;
        bit_output.push(bit_buf);
    }

    // Verify: hf_data_offset should land at the start of bit_output.
    // hf_data_offset = table_offset + table_len*4 - 0x3FC
    // table_offset = 2 (relative to payload start, which is our offset 0)
    // So hf_data_offset = 2 + table_len*4 - 0x3FC
    // = 2 + table_len*4 - 1020
    // This should equal output.len() (current end = table start + table_byte_size = 2 + table_byte_size)
    // Pad or adjust table_byte_size so hf_data_offset lines up.
    let expected_data_start = (2i64 + (table_len as i64) * 4 - 0x3FC) as usize;
    let current_end = output.len();
    if expected_data_start > current_end {
        output.resize(expected_data_start, 0);
    }
    // If expected_data_start < current_end, we have a problem - the table is larger
    // than expected. This shouldn't happen with correct table_len assignment.

    output.extend_from_slice(&bit_output);
    output
}

/// Compress `data` using JKR HFI format (LZ77 + Huffman).
pub fn compress_jkr_hfi(data: &[u8]) -> Vec<u8> {
    // LZ77 encode first.
    let lz_data = lz_encode(data);
    // Then Huffman encode the LZ output.
    let hfi_data = huffman_encode(&lz_data);

    // Build JKR header.
    let data_offset = JKR_HEADER_SIZE as u32;
    let mut output = Vec::with_capacity(JKR_HEADER_SIZE + hfi_data.len());
    output.extend_from_slice(&JKR_MAGIC.to_le_bytes());
    output.extend_from_slice(&1u16.to_le_bytes()); // version
    output.extend_from_slice(&TYPE_HFI.to_le_bytes()); // compression type
    output.extend_from_slice(&data_offset.to_le_bytes());
    output.extend_from_slice(&(data.len() as u32).to_le_bytes());
    output.extend_from_slice(&hfi_data);
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_jkr() {
        let mut buf = vec![0u8; 20];
        buf[..4].copy_from_slice(&JKR_MAGIC.to_le_bytes());
        assert!(is_jkr(&buf));
        assert!(!is_jkr(&[0, 0, 0, 0]));
    }

    #[test]
    fn test_lz_roundtrip() {
        let original = b"Hello World! Hello World! Hello World! Test data here.";
        let compressed = lz_encode(original);
        let decompressed = lz_decode(&compressed, original.len()).unwrap();
        assert_eq!(decompressed, original);
    }

    #[test]
    fn test_roundtrip_hfi() {
        let original = b"The quick brown fox jumps over the lazy dog. Pack my box with five dozen liquor jugs.";
        let compressed = compress_jkr_hfi(original);
        assert!(is_jkr(&compressed));
        let decompressed = decompress_jkr(&compressed).unwrap();
        assert_eq!(decompressed, original.to_vec());
    }

    #[test]
    fn test_roundtrip_with_repetitive_data() {
        // Lots of repetition to exercise LZ back-references.
        let mut data = Vec::new();
        for i in 0..100 {
            data.extend_from_slice(b"ABCDEFGHIJKLMNOP");
            data.push(b'0' + (i % 10));
        }
        let compressed = compress_jkr_hfi(&data);
        assert!(is_jkr(&compressed));
        let decompressed = decompress_jkr(&compressed).unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn test_roundtrip_single_byte() {
        let original = &[42u8];
        let compressed = compress_jkr_hfi(original);
        let decompressed = decompress_jkr(&compressed).unwrap();
        assert_eq!(decompressed, original);
    }

    #[test]
    fn test_roundtrip_all_same() {
        let original = vec![0xAA; 500];
        let compressed = compress_jkr_hfi(&original);
        let decompressed = decompress_jkr(&compressed).unwrap();
        assert_eq!(decompressed, original);
    }

    #[test]
    fn test_roundtrip_binary_data() {
        let original: Vec<u8> = (0..=255).cycle().take(1024).collect();
        let compressed = compress_jkr_hfi(&original);
        let decompressed = decompress_jkr(&compressed).unwrap();
        assert_eq!(decompressed, original);
    }

    #[test]
    fn test_lz_roundtrip_all_same() {
        let original = vec![0xAA; 500];
        let compressed = lz_encode(&original);
        let decompressed = lz_decode(&compressed, original.len()).unwrap();
        assert_eq!(decompressed, original);
    }

    #[test]
    fn test_lz_roundtrip_binary() {
        let original: Vec<u8> = (0..=255).cycle().take(1024).collect();
        let compressed = lz_encode(&original);
        let decompressed = lz_decode(&compressed, original.len()).unwrap();
        assert_eq!(decompressed, original);
    }

    #[test]
    fn test_cross_validate_python_jkr() {
        // JKR HFI compressed by Python FrontierTextHandler
        let python_jkr = hex::decode(
            "4a4b521a08010400100000001f000000\
             10010600210024002c0046004d006900\
             73007500030102010101000120004800\
             6c0074000401000065006e006f007200\
             07010601050108010a0109010b010c01\
             0d010e010f010c5dfef351fdbe152d6d\
             742887f66c5440",
        )
        .unwrap();
        let decompressed = decompress_jkr(&python_jkr).unwrap();
        assert_eq!(&decompressed, b"Hello, Monster Hunter Frontier!");
    }
}
