//! Rango dinámico del video (SDR, HDR10/PQ, HLG) a partir del SPS de HEVC.
//!
//! En macOS, AVPlayer rechaza (-12927) una master playlist con video PQ si la variante no
//! declara `VIDEO-RANGE=PQ` y `FRAME-RATE`. Los MKV no siempre traen el elemento Colour,
//! así que se lee la característica de transferencia del VUI del SPS.

/// `VIDEO-RANGE` para HLS a partir de un hvcC (HEVCDecoderConfigurationRecord).
pub fn hevc_video_range(hvcc: &[u8]) -> &'static str {
    match hevc_sps(hvcc).and_then(|sps| hevc_transfer(&sps)) {
        Some(16) => "PQ",  // SMPTE ST 2084 (HDR10, Dolby Vision 8.1)
        Some(18) => "HLG", // ARIB STD-B67
        _ => "SDR",
    }
}

/// Primer SPS del hvcC, sin bytes de prevención de emulación.
fn hevc_sps(hvcc: &[u8]) -> Option<Vec<u8>> {
    let mut p = 22;
    let arrays = *hvcc.get(p)?;
    p += 1;
    for _ in 0..arrays {
        let nal_type = hvcc.get(p)? & 0x3F;
        let count = u16::from_be_bytes([*hvcc.get(p + 1)?, *hvcc.get(p + 2)?]);
        p += 3;
        for _ in 0..count {
            let len = u16::from_be_bytes([*hvcc.get(p)?, *hvcc.get(p + 1)?]) as usize;
            p += 2;
            let nal = hvcc.get(p..p + len)?;
            if nal_type == 33 {
                return Some(unescape(nal));
            }
            p += len;
        }
    }
    None
}

fn unescape(nal: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal.len());
    let mut zeros = 0;
    for &b in nal {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn bit(&mut self) -> Option<u32> {
        let byte = *self.data.get(self.pos / 8)?;
        let b = (byte >> (7 - self.pos % 8)) & 1;
        self.pos += 1;
        Some(b as u32)
    }
    fn u(&mut self, n: u32) -> Option<u32> {
        let mut v = 0;
        for _ in 0..n {
            v = (v << 1) | self.bit()?;
        }
        Some(v)
    }
    fn skip(&mut self, n: usize) {
        self.pos += n;
    }
    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        Some((1u32 << zeros) - 1 + self.u(zeros)?)
    }
}

/// transfer_characteristics del VUI de un SPS HEVC (ITU-T H.265 §7.3.2.2), si lo declara.
fn hevc_transfer(sps: &[u8]) -> Option<u8> {
    let mut r = Bits { data: sps, pos: 16 }; // cabecera NAL
    r.skip(4); // sps_video_parameter_set_id
    let max_sub_layers_minus1 = r.u(3)?;
    r.skip(1);
    // profile_tier_level
    r.skip(96);
    let mut sub_profile = vec![];
    let mut sub_level = vec![];
    for _ in 0..max_sub_layers_minus1 {
        sub_profile.push(r.bit()? == 1);
        sub_level.push(r.bit()? == 1);
    }
    if max_sub_layers_minus1 > 0 {
        r.skip(2 * (8 - max_sub_layers_minus1 as usize));
    }
    for i in 0..max_sub_layers_minus1 as usize {
        if sub_profile[i] {
            r.skip(88);
        }
        if sub_level[i] {
            r.skip(8);
        }
    }
    r.ue()?; // sps_seq_parameter_set_id
    if r.ue()? == 3 {
        r.skip(1);
    }
    r.ue()?;
    r.ue()?; // ancho, alto
    if r.bit()? == 1 {
        for _ in 0..4 {
            r.ue()?;
        }
    }
    r.ue()?;
    r.ue()?; // profundidad de bits
    let log2_max_poc_lsb = r.ue()? + 4;
    let ordering_all = r.bit()? == 1;
    let first = if ordering_all { 0 } else { max_sub_layers_minus1 };
    for _ in first..=max_sub_layers_minus1 {
        r.ue()?;
        r.ue()?;
        r.ue()?;
    }
    for _ in 0..6 {
        r.ue()?;
    }
    if r.bit()? == 1 && r.bit()? == 1 {
        scaling_list_data(&mut r)?;
    }
    r.skip(2); // amp, sao
    if r.bit()? == 1 {
        r.skip(8);
        r.ue()?;
        r.ue()?;
        r.skip(1);
    }
    let num_sets = r.ue()? as usize;
    if num_sets > 64 {
        return None;
    }
    let mut num_delta_pocs: Vec<u32> = Vec::with_capacity(num_sets);
    for idx in 0..num_sets {
        let inter = idx != 0 && r.bit()? == 1;
        if inter {
            r.skip(1);
            r.ue()?;
            let reference = num_delta_pocs[idx - 1];
            let mut count = 0;
            for _ in 0..=reference {
                let used = r.bit()? == 1;
                let use_delta = used || r.bit()? == 1;
                if use_delta {
                    count += 1;
                }
            }
            num_delta_pocs.push(count);
        } else {
            let neg = r.ue()?;
            let pos = r.ue()?;
            if neg + pos > 32 {
                return None;
            }
            for _ in 0..neg + pos {
                r.ue()?;
                r.skip(1);
            }
            num_delta_pocs.push(neg + pos);
        }
    }
    if r.bit()? == 1 {
        let n = r.ue()?;
        for _ in 0..n {
            r.skip(log2_max_poc_lsb as usize + 1);
        }
    }
    r.skip(2); // temporal_mvp, strong_intra_smoothing
    if r.bit()? == 0 {
        return None; // sin VUI
    }
    if r.bit()? == 1 && r.u(8)? == 255 {
        r.skip(32);
    }
    if r.bit()? == 1 {
        r.skip(1);
    }
    if r.bit()? == 0 {
        return None; // sin video_signal_type
    }
    r.skip(4); // video_format, full_range
    if r.bit()? == 0 {
        return None;
    }
    r.skip(8); // colour_primaries
    Some(r.u(8)? as u8)
}

fn scaling_list_data(r: &mut Bits) -> Option<()> {
    for size_id in 0..4 {
        let step = if size_id == 3 { 3 } else { 1 };
        for _ in (0..6).step_by(step) {
            if r.bit()? == 0 {
                r.ue()?;
            } else {
                let coefs = 64.min(1 << (4 + (size_id << 1)));
                if size_id > 1 {
                    r.ue()?;
                }
                for _ in 0..coefs {
                    r.ue()?;
                }
            }
        }
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// hvcC de un WEB-DL 4K HDR10 / Dolby Vision 8.1 (Main 10, nivel 5.0).
    const HVCC_PQ: &str = concat!(
        "012220000000b0000000000096f000fcfdfafa00000f03a00001002140010c01ffff222000000300b000000300000300",
        "9618b03000003e900005dc0e80a100010046420101222000000300b00000030000030096a001e020064936718b924480",
        "53888892cf24a69272c9124922dc91aa48fca223ff000100016a1220136c2000007d20000bb81880a2000100084401c0",
        "252f85b240",
    );

    fn hex(s: &str) -> Vec<u8> {
        let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn detects_pq() {
        assert_eq!(hevc_video_range(&hex(HVCC_PQ)), "PQ");
    }

    #[test]
    fn garbage_is_sdr() {
        assert_eq!(hevc_video_range(&[1, 2, 3]), "SDR");
    }
}
