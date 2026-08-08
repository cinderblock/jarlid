//! Blowfish-ECB codec used by Pandora's tuner API.
//!
//! Requests are JSON, Blowfish-ECB encrypted, then lowercase-hex encoded. Responses that carry
//! `syncTime` encode it the same way. Verified against pydora's `transport.py` (2026-08-07).

use blowfish::Blowfish;
use cipher::generic_array::GenericArray;
use cipher::{BlockDecrypt, BlockEncrypt, KeyInit};

const BLOCK: usize = 8;

/// A keyed Blowfish-ECB codec. Pandora uses a *pair* of keys per partner: one for encrypting
/// what we send, a different one for decrypting what we receive.
pub struct Codec {
    cipher: Blowfish,
}

impl Codec {
    /// Blowfish accepts variable-length keys (4..=56 bytes); every Pandora partner key is in range.
    pub fn new(key: &str) -> Self {
        let cipher = Blowfish::new_from_slice(key.as_bytes())
            .expect("Pandora partner keys are always a valid Blowfish key length");
        Self { cipher }
    }

    /// Encrypt to lowercase hex. Pads with PKCS#7-style bytes to the 8-byte block size — note
    /// that a length that is already a multiple of 8 still gets a full block of padding.
    pub fn encrypt(&self, plaintext: &[u8]) -> String {
        let pad = BLOCK - (plaintext.len() % BLOCK);
        let mut buf = Vec::with_capacity(plaintext.len() + pad);
        buf.extend_from_slice(plaintext);
        buf.extend(std::iter::repeat_n(pad as u8, pad));

        for chunk in buf.chunks_mut(BLOCK) {
            let block = GenericArray::from_mut_slice(chunk);
            self.cipher.encrypt_block(block);
        }
        hex::encode(buf)
    }

    /// Decrypt from hex. Padding is left intact — callers strip what they know is there, because
    /// Pandora is not consistent about padding these payloads correctly.
    pub fn decrypt(&self, hex_text: &str) -> Result<Vec<u8>, hex::FromHexError> {
        let mut buf = hex::decode(hex_text)?;
        // A truncated trailing block can't be decrypted; drop it rather than panicking.
        let usable = buf.len() - (buf.len() % BLOCK);
        for chunk in buf[..usable].chunks_mut(BLOCK) {
            let block = GenericArray::from_mut_slice(chunk);
            self.cipher.decrypt_block(block);
        }
        buf.truncate(usable);
        Ok(buf)
    }

    /// Decrypt a `syncTime` field. The plaintext is 4 bytes of garbage followed by the ASCII
    /// decimal epoch seconds, followed by padding — so scan for digits rather than trusting a
    /// fixed slice (pydora hardcodes `[4:-2]`, which breaks if the padding length changes).
    pub fn decrypt_sync_time(&self, hex_text: &str) -> Option<u64> {
        let plain = self.decrypt(hex_text).ok()?;
        let digits: String = plain
            .get(4..)?
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .map(|&b| b as char)
            .collect();
        digits.parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-tripping through one key proves padding and block handling are self-consistent.
    #[test]
    fn round_trip() {
        let codec = Codec::new("R=U!LH$O2B#");
        for input in ["", "a", "12345678", "123456789", "{\"syncTime\":1234567890}"] {
            let decrypted = codec.decrypt(&codec.encrypt(input.as_bytes())).unwrap();
            assert!(
                decrypted.starts_with(input.as_bytes()),
                "round trip lost data for {input:?}"
            );
        }
    }

    /// An 8-byte input must grow to 16 bytes: a full extra block of padding, per PKCS#7.
    #[test]
    fn pads_full_block_when_already_aligned() {
        let codec = Codec::new("R=U!LH$O2B#");
        assert_eq!(codec.encrypt(b"12345678").len(), 32); // 16 bytes -> 32 hex chars
    }

    /// syncTime extraction must survive the 4 junk bytes and trailing padding.
    #[test]
    fn extracts_sync_time() {
        let codec = Codec::new("R=U!LH$O2B#");
        let payload = [b"\xde\xad\xbe\xef".as_slice(), b"1770000000".as_slice()].concat();
        assert_eq!(
            codec.decrypt_sync_time(&codec.encrypt(&payload)),
            Some(1_770_000_000)
        );
    }
}
