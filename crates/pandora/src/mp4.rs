//! Just enough MP4 parsing to identify what codec Pandora is actually serving.
//!
//! This exists to settle one question: is the stream plain AAC-LC, or HE-AAC (AAC-LC + SBR,
//! possibly + PS)? It decides the audio backend — Symphonia, the obvious pure-Rust choice, does
//! not implement SBR or PS, so on an HE-AAC stream it would silently decode only the core layer:
//! half the sample rate, no high band, audibly dull.
//!
//! We also use it to prove the absence of encryption boxes (`pssh`/`sinf`/`tenc`), i.e. that
//! there is no DRM to contend with.

/// Boxes whose presence would mean the stream is encrypted (Common Encryption / Widevine).
pub const ENCRYPTION_BOXES: [&str; 6] = ["pssh", "sinf", "schm", "tenc", "enca", "encv"];

/// MPEG-4 Audio Object Types we care about telling apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectType {
    AacLc,
    /// AAC-LC + Spectral Band Replication. "HE-AAC" / "aacPlus v1".
    HeAac,
    /// HE-AAC + Parametric Stereo. "HE-AACv2" / "aacPlus v2".
    HeAacV2,
    Other(u8),
}

impl ObjectType {
    fn from_code(code: u8) -> Self {
        match code {
            2 => Self::AacLc,
            5 => Self::HeAac,
            29 => Self::HeAacV2,
            other => Self::Other(other),
        }
    }

    /// Whether decoding this correctly requires SBR (and therefore rules out Symphonia).
    pub fn needs_sbr(self) -> bool {
        matches!(self, Self::HeAac | Self::HeAacV2)
    }
}

impl std::fmt::Display for ObjectType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AacLc => write!(f, "AAC-LC"),
            Self::HeAac => write!(f, "HE-AAC (AAC-LC + SBR)"),
            Self::HeAacV2 => write!(f, "HE-AACv2 (AAC-LC + SBR + PS)"),
            Self::Other(code) => write!(f, "audio object type {code}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AudioConfig {
    pub object_type: ObjectType,
    pub sample_rate: u32,
    pub channels: u8,
    /// Set when the config explicitly signals SBR with an extended sample rate.
    pub extension_sample_rate: Option<u32>,
}

const SAMPLE_RATES: [u32; 13] = [
    96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
];

/// Walk the direct children of an MP4 box payload, yielding `(type, payload)`.
fn children(data: &[u8]) -> impl Iterator<Item = (&str, &[u8])> {
    let mut offset = 0usize;
    std::iter::from_fn(move || {
        // Not a loop: every path below returns. Each call yields exactly one box, and the
        // iterator resumes here on the next `next()`.
        if offset + 8 <= data.len() {
            let size = u32::from_be_bytes(data[offset..offset + 4].try_into().ok()?) as usize;
            let name = std::str::from_utf8(&data[offset + 4..offset + 8]).ok()?;

            // size 0 means "to end of file"; size 1 means a 64-bit size follows the type.
            let (header, size) = match size {
                0 => (8, data.len() - offset),
                1 => {
                    let large = u64::from_be_bytes(data.get(offset + 8..offset + 16)?.try_into().ok()?);
                    (16, large as usize)
                }
                n => (8, n),
            };

            if size < header || offset + header > data.len() {
                return None;
            }
            // A box may extend past a truncated download (mdat especially). Still report it, with
            // whatever payload we have, then stop — otherwise we'd hide every box we did receive.
            let end = (offset + size).min(data.len());
            let payload = &data[offset + header..end];
            offset = if offset + size > data.len() { data.len() } else { offset + size };
            return Some((name, payload));
        }
        None
    })
}

/// Find a box by path, e.g. `["moov", "trak", "mdia", "minf", "stbl", "stsd"]`.
pub fn find(data: &[u8], path: &[&str]) -> Option<Vec<u8>> {
    let Some((head, rest)) = path.split_first() else {
        return Some(data.to_vec());
    };
    for (name, payload) in children(data) {
        if name == *head {
            if let Some(found) = find(payload, rest) {
                return Some(found);
            }
        }
    }
    None
}

/// Boxes that contain other boxes. Descending into anything else would parse raw media or table
/// data as if it were a box tree, producing garbage names — and potentially a *false positive* on
/// the encryption check, since random bytes can spell "sinf".
const CONTAINERS: [&str; 14] = [
    "moov", "trak", "edts", "mdia", "minf", "dinf", "stbl", "udta", "mvex", "moof", "traf",
    "sinf", "schi", "meta",
];

/// How many bytes of fixed header a box has before its children begin.
fn container_header(name: &str) -> Option<usize> {
    match name {
        "stsd" => Some(8),  // full box: version/flags + entry_count
        "mp4a" | "enca" => Some(28), // AudioSampleEntry fixed fields
        "meta" => Some(4),  // full box: version/flags
        name if CONTAINERS.contains(&name) => Some(0),
        _ => None,
    }
}

/// Report which box types appear anywhere in the tree. Used to prove encryption boxes are absent.
pub fn box_types(data: &[u8]) -> Vec<String> {
    fn walk(data: &[u8], depth: usize, out: &mut Vec<String>) {
        if depth > 8 {
            return;
        }
        for (name, payload) in children(data) {
            out.push(name.to_string());
            if let Some(header) = container_header(name) {
                if let Some(inner) = payload.get(header..) {
                    walk(inner, depth + 1, out);
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(data, 0, &mut out);
    out
}

/// Extract the AudioSpecificConfig from `moov/…/stsd/mp4a/esds`.
pub fn audio_config(data: &[u8]) -> Option<AudioConfig> {
    let stsd = find(data, &["moov", "trak", "mdia", "minf", "stbl", "stsd"])?;
    // stsd is a full box: 4 bytes version/flags, 4 bytes entry_count, then sample entries.
    let mp4a = children(stsd.get(8..)?).find(|(name, _)| *name == "mp4a")?.1;
    // AudioSampleEntry has a 28-byte fixed header before its child boxes.
    let esds = children(mp4a.get(28..)?).find(|(name, _)| *name == "esds")?.1;

    parse_asc(decoder_specific_info(esds)?)
}

/// Dig the DecoderSpecificInfo out of an esds box's nested descriptors.
fn decoder_specific_info(esds: &[u8]) -> Option<&[u8]> {
    // esds is a full box: skip 4 bytes of version/flags.
    let mut data = esds.get(4..)?;

    loop {
        let (&tag, rest) = data.split_first()?;
        // Descriptor lengths use a variable-length scheme: 7 bits per byte, high bit = continue.
        let mut length = 0usize;
        let mut rest = rest;
        for _ in 0..4 {
            let (&byte, next) = rest.split_first()?;
            rest = next;
            length = (length << 7) | (byte & 0x7f) as usize;
            if byte & 0x80 == 0 {
                break;
            }
        }
        let payload = rest.get(..length.min(rest.len()))?;

        match tag {
            0x03 => {
                // ES_Descriptor: ES_ID(2) + flags(1), plus optional fields we don't need here.
                let flags = *payload.get(2)?;
                let mut offset = 3;
                if flags & 0x80 != 0 {
                    offset += 2; // streamDependenceFlag
                }
                if flags & 0x40 != 0 {
                    offset += 1 + *payload.get(offset)? as usize; // URL
                }
                if flags & 0x20 != 0 {
                    offset += 2; // OCRstream
                }
                data = payload.get(offset..)?;
            }
            // DecoderConfigDescriptor: objectTypeIndication(1) streamType(1) bufferSize(3)
            // maxBitrate(4) avgBitrate(4), then the DecoderSpecificInfo.
            0x04 => data = payload.get(13..)?,
            0x05 => return Some(payload), // DecoderSpecificInfo — the AudioSpecificConfig
            _ => return None,
        }
    }
}

/// Parse an AudioSpecificConfig bitstream (ISO/IEC 14496-3).
fn parse_asc(asc: &[u8]) -> Option<AudioConfig> {
    let mut bits = BitReader::new(asc);

    let mut object_type = bits.read(5)? as u8;
    if object_type == 31 {
        object_type = 32 + bits.read(6)? as u8;
    }

    // Per ISO/IEC 14496-3 the field order is: samplingFrequencyIndex (the *core* rate),
    // channelConfiguration, and only then — for AOT 5/29 — extensionSamplingFrequencyIndex.
    let core_sample_rate = read_sample_rate(&mut bits)?;
    let channels = bits.read(4)? as u8;

    let extension_sample_rate = match object_type {
        5 | 29 => Some(read_sample_rate(&mut bits)?),
        _ => None,
    };

    Some(AudioConfig {
        object_type: ObjectType::from_code(object_type),
        // SBR reconstructs the high band, so the extension rate is the true output rate.
        sample_rate: extension_sample_rate.unwrap_or(core_sample_rate),
        channels,
        extension_sample_rate,
    })
}

fn read_sample_rate(bits: &mut BitReader) -> Option<u32> {
    let index = bits.read(4)? as usize;
    if index == 0x0f {
        return bits.read(24);
    }
    SAMPLE_RATES.get(index).copied()
}

struct BitReader<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    fn read(&mut self, count: usize) -> Option<u32> {
        let mut value = 0u32;
        for _ in 0..count {
            let byte = *self.data.get(self.position / 8)?;
            let bit = (byte >> (7 - self.position % 8)) & 1;
            value = (value << 1) | bit as u32;
            self.position += 1;
        }
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// HE-AACv2 as Pandora would signal it: AOT 29, core 22050, stereo, extension 44100.
    /// Bits: 11101 | 0111 (22050) | 0010 (stereo) | 0100 (44100) -> 0xEB 0x92 0x00
    #[test]
    fn detects_he_aac_v2() {
        let config = parse_asc(&[0xEB, 0x92, 0x00]).expect("parses");
        assert_eq!(config.object_type, ObjectType::HeAacV2);
        assert!(config.object_type.needs_sbr());
        assert_eq!(config.channels, 2);
        // The extension rate is what a correct decoder outputs; the core is half of it.
        assert_eq!(config.extension_sample_rate, Some(44100));
        assert_eq!(config.sample_rate, 44100);
    }

    /// Plain AAC-LC must NOT be reported as needing SBR.
    #[test]
    fn detects_aac_lc() {
        // 2=00010, sfIndex 4 (44100)=0100, channels 2=0010 -> 0001 0010 0001 0
        let asc = [0b0001_0010, 0b0001_0000];
        let config = parse_asc(&asc).expect("parses");
        assert_eq!(config.object_type, ObjectType::AacLc);
        assert!(!config.object_type.needs_sbr());
        assert_eq!(config.sample_rate, 44100);
        assert_eq!(config.channels, 2);
    }

    /// The box walker must not run off the end of a truncated download.
    #[test]
    fn survives_truncated_input() {
        assert!(audio_config(&[0, 0, 0, 200, b'm', b'o', b'o', b'v', 1, 2]).is_none());
        assert!(find(&[], &["moov"]).is_none());
    }
}
