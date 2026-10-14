//! Reading the app's 16 kHz mono WAV into the floats the model takes.
//!
//! Only what the app writes is accepted — 16 kHz, mono, 16-bit PCM or 32-bit float — and anything
//! else is an error rather than a silent resample: a wrong rate would not fail, it would just
//! make every timestamp wrong.

use autosync::window::RATE;

/// The samples of `bytes`, as floats in −1..1.
pub fn read(bytes: &[u8]) -> Result<Vec<f32>, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".into());
    }
    let mut format: Option<(u16, u16, u32, u16)> = None;
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = at + 8;
        let end = (body + size).min(bytes.len());
        match id {
            b"fmt " if end - body >= 16 => {
                let u16_at = |o: usize| u16::from_le_bytes([bytes[body + o], bytes[body + o + 1]]);
                let rate = u32::from_le_bytes(bytes[body + 4..body + 8].try_into().unwrap());
                format = Some((u16_at(0), u16_at(2), rate, u16_at(14)));
            }
            b"data" => {
                let (tag, channels, rate, bits) = format.ok_or("data before fmt")?;
                if channels != 1 || rate != RATE as u32 {
                    return Err(format!("{channels} ch at {rate} Hz; need mono {RATE} Hz"));
                }
                let data = &bytes[body..end];
                return match (tag, bits) {
                    (1, 16) => Ok(data
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|s| i16::from_le_bytes(*s) as f32 / 32768.0)
                        .collect()),
                    (3, 32) => Ok(data
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|s| f32::from_le_bytes(*s))
                        .collect()),
                    _ => Err(format!(
                        "format {tag}, {bits}-bit; need 16-bit PCM or float"
                    )),
                };
            }
            _ => {}
        }
        // Chunks are padded to an even length.
        at = body + size + (size & 1);
    }
    Err("no data chunk".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(tag: u16, channels: u16, rate: u32, bits: u16, data: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&tag.to_le_bytes());
        v.extend_from_slice(&channels.to_le_bytes());
        v.extend_from_slice(&rate.to_le_bytes());
        v.extend_from_slice(&(rate * (bits as u32 / 8)).to_le_bytes());
        v.extend_from_slice(&(bits / 8).to_le_bytes());
        v.extend_from_slice(&bits.to_le_bytes());
        // An unrelated chunk with odd length, to exercise padding.
        v.extend_from_slice(b"LIST");
        v.extend_from_slice(&3u32.to_le_bytes());
        v.extend_from_slice(&[0, 0, 0, 0]);
        v.extend_from_slice(b"data");
        v.extend_from_slice(&(data.len() as u32).to_le_bytes());
        v.extend_from_slice(data);
        v
    }

    #[test]
    fn reads_16_bit_pcm() {
        let data: Vec<u8> = [0i16, 16384, -32768]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        assert_eq!(
            read(&wav(1, 1, 16_000, 16, &data)).unwrap(),
            [0.0, 0.5, -1.0]
        );
    }

    #[test]
    fn reads_float() {
        let data: Vec<u8> = [0.25f32, -0.5]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        assert_eq!(read(&wav(3, 1, 16_000, 32, &data)).unwrap(), [0.25, -0.5]);
    }

    #[test]
    fn refuses_the_wrong_rate_or_channels() {
        assert!(read(&wav(1, 1, 44_100, 16, &[0, 0])).is_err());
        assert!(read(&wav(1, 2, 16_000, 16, &[0, 0, 0, 0])).is_err());
        assert!(read(b"not a wav").is_err());
    }
}
