//! Configuración de los códecs de audio compatibles con AirPlay/AVPlayer:
//! AAC, AC-3 (Dolby Digital) y E-AC-3 (Dolby Digital Plus).

use crate::fmp4::{AudioCodec, AudioParams};
use crate::mkv::Track;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioKind {
    Aac,
    Ac3,
    Eac3,
}

pub fn kind_of(codec_id: &str) -> Option<AudioKind> {
    if codec_id.starts_with("A_AAC") {
        Some(AudioKind::Aac)
    } else if codec_id == "A_AC3" || codec_id.starts_with("A_AC3/") {
        Some(AudioKind::Ac3)
    } else if codec_id == "A_EAC3" {
        Some(AudioKind::Eac3)
    } else {
        None
    }
}

pub fn friendly_codec(codec_id: &str) -> String {
    let name = match codec_id {
        c if c.starts_with("A_AAC") => "AAC",
        "A_AC3" => "Dolby Digital (AC-3)",
        "A_EAC3" => "Dolby Digital Plus (E-AC-3)",
        c if c.starts_with("A_DTS") => "DTS",
        "A_TRUEHD" => "Dolby TrueHD",
        "A_FLAC" => "FLAC",
        "A_OPUS" => "Opus",
        "A_VORBIS" => "Vorbis",
        "A_MPEG/L3" => "MP3",
        c if c.starts_with("A_PCM") => "PCM",
        "V_MPEG4/ISO/AVC" => "H.264",
        "V_MPEGH/ISO/HEVC" => "HEVC (H.265)",
        "V_AV1" => "AV1",
        "V_VP9" => "VP9",
        "V_VP8" => "VP8",
        "V_MPEG2" => "MPEG-2",
        other => other,
    };
    name.to_string()
}

const AAC_RATES: [u32; 13] = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350];

/// Parámetros para el sample entry. `first_frame` hace falta para AC-3/E-AC-3.
pub fn audio_params(t: &Track, kind: AudioKind, first_frame: &[u8]) -> Option<(AudioParams, u32)> {
    match kind {
        AudioKind::Aac => {
            let rate = t.sample_rate.round() as u32;
            let asc = if t.codec_private.len() >= 2 {
                t.codec_private.clone()
            } else {
                // A_AAC/MPEG4/LC y similares sin CodecPrivate: construir el ASC.
                let object_type: u8 = if t.codec_id.ends_with("/MAIN") { 1 } else { 2 };
                let idx = AAC_RATES.iter().position(|&r| r == rate).unwrap_or(3) as u8;
                let ch = t.channels.min(7) as u8;
                vec![(object_type << 3) | (idx >> 1), ((idx & 1) << 7) | (ch << 3)]
            };
            // Frecuencia "núcleo" desde el ASC (para HE-AAC con SBR implícito).
            let core_rate = asc_sample_rate(&asc).unwrap_or(rate);
            Some((
                AudioParams { codec: AudioCodec::Aac(asc), sample_rate: core_rate, channels: t.channels as u16 },
                1024,
            ))
        }
        AudioKind::Ac3 => {
            let h = parse_ac3(first_frame)?;
            Some((
                AudioParams { codec: AudioCodec::Ac3(h.dac3), sample_rate: h.sample_rate, channels: t.channels as u16 },
                1536,
            ))
        }
        AudioKind::Eac3 => {
            let h = parse_eac3(first_frame)?;
            Some((
                AudioParams { codec: AudioCodec::Eac3(h.dec3), sample_rate: h.sample_rate, channels: t.channels as u16 },
                h.samples_per_frame,
            ))
        }
    }
}

fn asc_sample_rate(asc: &[u8]) -> Option<u32> {
    if asc.len() < 2 {
        return None;
    }
    let mut br = BitReader::new(asc);
    let mut ot = br.bits(5)?;
    if ot == 31 {
        ot = 32 + br.bits(6)?;
    }
    let _ = ot;
    let idx = br.bits(4)?;
    if idx == 15 {
        br.bits(24)
    } else {
        AAC_RATES.get(idx as usize).copied()
    }
}

struct BitReader<'a> {
    data: &'a [u8],
    bit: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, bit: 0 }
    }
    fn bits(&mut self, n: usize) -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            let byte = *self.data.get(self.bit / 8)?;
            let b = (byte >> (7 - (self.bit % 8))) & 1;
            v = (v << 1) | b as u32;
            self.bit += 1;
        }
        Some(v)
    }
}

struct BitWriter {
    out: Vec<u8>,
    bit: usize,
}

impl BitWriter {
    fn new() -> Self {
        Self { out: vec![], bit: 0 }
    }
    fn put(&mut self, n: usize, v: u32) {
        for i in (0..n).rev() {
            if self.bit % 8 == 0 {
                self.out.push(0);
            }
            let b = ((v >> i) & 1) as u8;
            let last = self.out.len() - 1;
            self.out[last] |= b << (7 - (self.bit % 8));
            self.bit += 1;
        }
    }
}

struct Ac3Header {
    dac3: [u8; 3],
    sample_rate: u32,
}

fn parse_ac3(f: &[u8]) -> Option<Ac3Header> {
    if f.len() < 7 || f[0] != 0x0B || f[1] != 0x77 {
        return None;
    }
    let mut br = BitReader::new(&f[4..]);
    let fscod = br.bits(2)?;
    let frmsizecod = br.bits(6)?;
    let bsid = br.bits(5)?;
    let bsmod = br.bits(3)?;
    let acmod = br.bits(3)?;
    if acmod & 1 != 0 && acmod != 1 {
        br.bits(2)?; // cmixlev
    }
    if acmod & 4 != 0 {
        br.bits(2)?; // surmixlev
    }
    if acmod == 2 {
        br.bits(2)?; // dsurmod
    }
    let lfeon = br.bits(1)?;
    let sample_rate = [48000, 44100, 32000].get(fscod as usize).copied()?;
    let mut w = BitWriter::new();
    w.put(2, fscod);
    w.put(5, bsid);
    w.put(3, bsmod);
    w.put(3, acmod);
    w.put(1, lfeon);
    w.put(5, frmsizecod >> 1);
    w.put(5, 0);
    let mut dac3 = [0u8; 3];
    dac3.copy_from_slice(&w.out[..3]);
    Some(Ac3Header { dac3, sample_rate })
}

struct Eac3Header {
    dec3: Vec<u8>,
    sample_rate: u32,
    samples_per_frame: u32,
}

/// Cabecera de un syncframe E-AC-3 (lo necesario para la caja dec3).
struct Eac3Sync {
    strmtyp: u32,
    bytes: usize,
    fscod: u32,
    sample_rate: u32,
    numblks: u32,
    acmod: u32,
    lfeon: u32,
    bsid: u32,
    chanmap: Option<u32>,
}

fn parse_eac3_sync(f: &[u8]) -> Option<Eac3Sync> {
    if f.len() < 6 || f[0] != 0x0B || f[1] != 0x77 {
        return None;
    }
    let mut br = BitReader::new(&f[2..]);
    let strmtyp = br.bits(2)?;
    br.bits(3)?; // substreamid
    let frmsiz = br.bits(11)?;
    let fscod = br.bits(2)?;
    let (sample_rate, numblks) = if fscod == 3 {
        let fscod2 = br.bits(2)?;
        ([24000, 22050, 16000].get(fscod2 as usize).copied()?, 6)
    } else {
        let numblkscod = br.bits(2)?;
        ([48000, 44100, 32000][fscod as usize], [1, 2, 3, 6][numblkscod as usize])
    };
    let acmod = br.bits(3)?;
    let lfeon = br.bits(1)?;
    let bsid = br.bits(5)?;
    // dialnorm, compr y (si acmod == 0) su par para el segundo canal mono.
    br.bits(5)?;
    if br.bits(1)? == 1 {
        br.bits(8)?;
    }
    if acmod == 0 {
        br.bits(5)?;
        if br.bits(1)? == 1 {
            br.bits(8)?;
        }
    }
    let chanmap = if strmtyp == 1 && br.bits(1)? == 1 { Some(br.bits(16)?) } else { None };
    Some(Eac3Sync {
        strmtyp,
        bytes: (frmsiz as usize + 1) * 2,
        fscod,
        sample_rate,
        numblks,
        acmod,
        lfeon,
        bsid,
        chanmap,
    })
}

/// chan_loc (9 bits) de la caja dec3 a partir del chanmap de un substream dependiente.
/// chanmap (bit 0 = MSB): 5 Lc/Rc, 6 Lrs/Rrs, 7 Cs, 8 Ts, 9 Lsd/Rsd, 10 Lw/Rw,
/// 11 Lvh/Rvh, 12 Cvh, 14 LFE2. chan_loc usa ese mismo orden sin Lts/Rts (bit 13).
fn chan_loc_from_chanmap(chanmap: u32) -> u32 {
    let bit = |i: u32| (chanmap >> (15 - i)) & 1;
    let mut loc = 0;
    for (k, i) in (5..=12).enumerate() {
        loc |= bit(i) << (8 - k as u32);
    }
    loc | bit(14)
}

fn parse_eac3(f: &[u8]) -> Option<Eac3Header> {
    let first = parse_eac3_sync(f)?;
    // Un frame de Matroska puede contener el substream independiente seguido
    // de substreams dependientes (p. ej. 7.1 = 5.1 + 2 canales traseros).
    let mut num_dep_sub = 0u32;
    let mut chan_loc = 0u32;
    let mut total_bytes = first.bytes;
    let mut off = first.bytes;
    while off < f.len() {
        let Some(s) = parse_eac3_sync(&f[off..]) else { break };
        // Solo los dependientes (strmtyp 1); otro independiente sería otro programa.
        if s.strmtyp != 1 {
            break;
        }
        num_dep_sub += 1;
        chan_loc |= match s.chanmap {
            Some(m) => chan_loc_from_chanmap(m),
            // Sin chanmap, un dependiente estéreo suele ser el par trasero Lrs/Rrs.
            None if s.acmod == 2 => 1 << 7,
            None => 0,
        };
        total_bytes += s.bytes;
        off += s.bytes;
    }

    let samples = first.numblks * 256;
    let data_rate = (total_bytes as u64 * 8 * first.sample_rate as u64 / samples as u64 / 1000) as u32;

    let mut w = BitWriter::new();
    w.put(13, data_rate);
    w.put(3, 0); // num_ind_sub - 1
    w.put(2, first.fscod);
    w.put(5, first.bsid);
    w.put(1, 0); // reserved
    w.put(1, 0); // asvc
    w.put(3, 0); // bsmod
    w.put(3, first.acmod);
    w.put(1, first.lfeon);
    w.put(3, 0); // reserved
    w.put(4, num_dep_sub.min(15));
    if num_dep_sub > 0 {
        w.put(9, chan_loc);
    } else {
        w.put(1, 0); // reserved
    }
    Some(Eac3Header { dec3: w.out, sample_rate: first.sample_rate, samples_per_frame: samples })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asc_rate() {
        // AAC-LC 48 kHz estéreo
        assert_eq!(asc_sample_rate(&[0x11, 0x90]), Some(48000));
    }

    #[test]
    fn ac3_header() {
        // 48 kHz, 384 kbps, 5.1
        let frame = [0x0B, 0x77, 0, 0, 0b0001_1100, 0b0100_0111, 0b1000_0000, 0, 0, 0];
        let h = parse_ac3(&frame).unwrap();
        assert_eq!(h.sample_rate, 48000);
    }

    /// Syncframe E-AC-3 mínimo de `bytes` bytes.
    fn eac3_frame(strmtyp: u32, acmod: u32, lfeon: u32, chanmap: Option<u32>, bytes: usize) -> Vec<u8> {
        let mut w = BitWriter::new();
        w.put(16, 0x0B77);
        w.put(2, strmtyp);
        w.put(3, 0); // substreamid
        w.put(11, (bytes / 2 - 1) as u32);
        w.put(2, 0); // fscod 48 kHz
        w.put(2, 3); // 6 bloques
        w.put(3, acmod);
        w.put(1, lfeon);
        w.put(5, 16); // bsid
        w.put(5, 31); // dialnorm
        w.put(1, 0); // compre
        if strmtyp == 1 {
            match chanmap {
                Some(m) => {
                    w.put(1, 1);
                    w.put(16, m);
                }
                None => w.put(1, 0),
            }
        }
        let mut v = w.out;
        v.resize(bytes, 0);
        v
    }

    #[test]
    fn eac3_5_1() {
        let h = parse_eac3(&eac3_frame(0, 7, 1, None, 768)).unwrap();
        assert_eq!(h.samples_per_frame, 1536);
        assert_eq!(h.dec3.len(), 5);
        // num_dep_sub = 0
        assert_eq!((h.dec3[4] >> 1) & 0x0F, 0);
    }

    #[test]
    fn eac3_7_1_con_dependiente() {
        // Independiente 5.1 + dependiente con Lrs/Rrs (bit 6 del chanmap).
        let mut f = eac3_frame(0, 7, 1, None, 768);
        f.extend(eac3_frame(1, 2, 0, Some(1 << (15 - 6)), 256));
        let h = parse_eac3(&f).unwrap();
        assert_eq!(h.dec3.len(), 6);
        let mut br = BitReader::new(&h.dec3);
        let data_rate = br.bits(13).unwrap();
        assert_eq!(data_rate, (1024 * 8 * 48000 / 1536 / 1000) as u32);
        br.bits(3 + 2 + 5 + 1 + 1 + 3 + 3 + 1 + 3).unwrap();
        assert_eq!(br.bits(4), Some(1)); // un substream dependiente
        assert_eq!(br.bits(9), Some(1 << 7)); // Lrs/Rrs
    }

    #[test]
    fn chan_loc_mapping() {
        assert_eq!(chan_loc_from_chanmap(1 << (15 - 5)), 1 << 8); // Lc/Rc
        assert_eq!(chan_loc_from_chanmap(1 << (15 - 14)), 1); // LFE2
        assert_eq!(chan_loc_from_chanmap(1 << (15 - 13)), 0); // Lts/Rts no existe en dec3
    }
}
