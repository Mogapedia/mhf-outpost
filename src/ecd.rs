//! ECD encryption and decryption for MHF game files.
//!
//! All MHF files use ECD with key index 4. The ECD format wraps a 16-byte
//! header around XOR-encrypted payload data.

use anyhow::{bail, Result};

const ECD_MAGIC: u32 = 0x1A646365;
const HEADER_SIZE: usize = 16;

/// Per-key RNG parameters: each entry is (multiplier: u32be, increment: u32be).
#[rustfmt::skip]
const RND_BUF_ECD: &[u8] = &[
    0x4A, 0x4B, 0x52, 0x2E, 0x00, 0x00, 0x00, 0x01, // Key 0
    0x00, 0x01, 0x0D, 0xCD, 0x00, 0x00, 0x00, 0x01, // Key 1
    0x00, 0x01, 0x0D, 0xCD, 0x00, 0x00, 0x00, 0x01, // Key 2
    0x00, 0x01, 0x0D, 0xCD, 0x00, 0x00, 0x00, 0x01, // Key 3
    0x00, 0x19, 0x66, 0x0D, 0x00, 0x00, 0x00, 0x03, // Key 4 (default)
    0x7D, 0x2B, 0x89, 0xDD, 0x00, 0x00, 0x00, 0x01, // Key 5
];

fn load_u32_be(buf: &[u8], offset: usize) -> u32 {
    ((buf[offset] as u32) << 24)
        | ((buf[offset + 1] as u32) << 16)
        | ((buf[offset + 2] as u32) << 8)
        | (buf[offset + 3] as u32)
}

fn read_u32_le(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn read_u16_le(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

/// Advance the ECD PRNG and return (new_rnd, xorpad).
fn get_rnd_ecd(ecd_key: u16, rnd: u32) -> (u32, u32) {
    let multiplier = load_u32_be(RND_BUF_ECD, 8 * ecd_key as usize);
    let increment = load_u32_be(RND_BUF_ECD, 8 * ecd_key as usize + 4);
    let rnd = rnd.wrapping_mul(multiplier).wrapping_add(increment);
    (rnd, rnd)
}

/// Returns `true` if `data` starts with the ECD magic bytes.
pub fn is_ecd(data: &[u8]) -> bool {
    if data.len() < 4 {
        return false;
    }
    read_u32_le(data, 0) == ECD_MAGIC
}

/// Decrypt an ECD-wrapped buffer, returning the plaintext payload.
pub fn decode_ecd(data: &[u8]) -> Result<Vec<u8>> {
    if data.len() < HEADER_SIZE {
        bail!(
            "ECD data too short ({} bytes, need at least {})",
            data.len(),
            HEADER_SIZE
        );
    }
    let magic = read_u32_le(data, 0);
    if magic != ECD_MAGIC {
        bail!("not an ECD file (magic: {:#010x})", magic);
    }

    let ecd_key = read_u16_le(data, 4);
    let payload_size = read_u32_le(data, 8) as usize;
    let crc32 = read_u32_le(data, 12);

    if data.len() < HEADER_SIZE + payload_size {
        bail!(
            "ECD payload truncated: header says {} bytes but only {} available",
            payload_size,
            data.len() - HEADER_SIZE
        );
    }

    let mut rnd = crc32.rotate_right(16) | 1;
    let (new_rnd, xorpad) = get_rnd_ecd(ecd_key, rnd);
    rnd = new_rnd;
    let mut r8 = (xorpad & 0xFF) as u8;

    let mut output = vec![0u8; payload_size];
    for i in 0..payload_size {
        let (new_rnd, mut xorpad) = get_rnd_ecd(ecd_key, rnd);
        rnd = new_rnd;

        let encrypted_byte = data[HEADER_SIZE + i];
        let mut r11 = encrypted_byte ^ r8;
        let mut r12 = r11 >> 4;

        for _ in 0..8 {
            let r10 = (xorpad as u8) ^ r11;
            r11 = r12;
            r12 ^= r10;
            xorpad >>= 4;
        }

        r8 = (r12 & 0xF) | ((r11 & 0xF) << 4);
        output[i] = r8;
    }

    Ok(output)
}

/// Encrypt plaintext `data` into ECD format with the given key index.
pub fn encode_ecd(data: &[u8], key_index: u16) -> Vec<u8> {
    let payload_size = data.len();
    let crc32_val = crc32fast::hash(data);

    // Build 16-byte header.
    let mut output = Vec::with_capacity(HEADER_SIZE + payload_size);
    output.extend_from_slice(&ECD_MAGIC.to_le_bytes());
    output.extend_from_slice(&key_index.to_le_bytes());
    output.extend_from_slice(&0u16.to_le_bytes());
    output.extend_from_slice(&(payload_size as u32).to_le_bytes());
    output.extend_from_slice(&crc32_val.to_le_bytes());
    output.resize(HEADER_SIZE + payload_size, 0);

    let mut rnd = crc32_val.rotate_right(16) | 1;
    let (new_rnd, xorpad) = get_rnd_ecd(key_index, rnd);
    rnd = new_rnd;
    let mut r8 = (xorpad & 0xFF) as u8;

    for i in 0..payload_size {
        let (new_rnd, mut xorpad) = get_rnd_ecd(key_index, rnd);
        rnd = new_rnd;

        let mut r11: u8 = 0;
        let mut r12: u8 = 0;
        for _ in 0..8 {
            let r10 = (xorpad as u8) ^ r11;
            r11 = r12;
            r12 ^= r10;
            xorpad >>= 4;
        }

        let plaintext_byte = data[i];
        let dig2 = plaintext_byte ^ r12;
        let dig1 = ((plaintext_byte >> 4) ^ r11) ^ dig2;
        let rr = ((dig2 & 0xF) | ((dig1 & 0xF) << 4)) ^ r8;

        output[HEADER_SIZE + i] = rr;
        r8 = plaintext_byte;
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_ecd() {
        let mut buf = vec![0u8; 20];
        buf[..4].copy_from_slice(&ECD_MAGIC.to_le_bytes());
        assert!(is_ecd(&buf));
        assert!(!is_ecd(&[0, 0, 0, 0]));
        assert!(!is_ecd(&[1, 2]));
    }

    #[test]
    fn test_roundtrip() {
        let original = b"Hello, Monster Hunter Frontier! This is a test payload with varied bytes: \x00\x01\xff\x80\x7f";
        let encrypted = encode_ecd(original, 4);
        assert!(is_ecd(&encrypted));
        let decrypted = decode_ecd(&encrypted).unwrap();
        assert_eq!(decrypted, original);
    }

    #[test]
    fn test_decode_known() {
        // Encode then decode with different key indices.
        for key in [0u16, 1, 2, 3, 4, 5] {
            let data = vec![42u8; 256];
            let enc = encode_ecd(&data, key);
            let dec = decode_ecd(&enc).unwrap();
            assert_eq!(dec, data, "roundtrip failed for key {key}");
        }
    }

    #[test]
    fn test_roundtrip_empty() {
        let original: &[u8] = &[];
        let encrypted = encode_ecd(original, 4);
        let decrypted = decode_ecd(&encrypted).unwrap();
        assert_eq!(decrypted, original);
    }

    #[test]
    fn test_cross_validate_python_ecd() {
        // ECD encrypted by Python FrontierTextHandler (key 4)
        let python_ecd = hex::decode(
            "6563641a040000001f00000065ba2db6\
             e5d18309bc6376cb67812d7f0f4cfed2\
             b8e921dd59a4d523ab3179e59eaf37",
        )
        .unwrap();
        let decrypted = decode_ecd(&python_ecd).unwrap();
        assert_eq!(&decrypted, b"Hello, Monster Hunter Frontier!");
    }
}
