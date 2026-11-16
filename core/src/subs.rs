//! Subtítulos de texto (SRT, ASS/SSA, WebVTT) → WebVTT para HLS,
//! y utilidades para la master playlist (idiomas y cadenas CODECS).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubKind {
    Srt,
    Ass,
    WebVtt,
}

pub fn kind_of(codec_id: &str) -> Option<SubKind> {
    match codec_id {
        "S_TEXT/UTF8" | "S_TEXT/ASCII" => Some(SubKind::Srt),
        "S_TEXT/ASS" | "S_TEXT/SSA" | "S_ASS" | "S_SSA" => Some(SubKind::Ass),
        "S_TEXT/WEBVTT" => Some(SubKind::WebVtt),
        _ => None,
    }
}

pub fn friendly_codec(codec_id: &str) -> String {
    match codec_id {
        "S_TEXT/UTF8" | "S_TEXT/ASCII" => "SRT",
        "S_TEXT/ASS" | "S_ASS" => "ASS",
        "S_TEXT/SSA" | "S_SSA" => "SSA",
        "S_TEXT/WEBVTT" => "WebVTT",
        "S_HDMV/PGS" => "PGS (imagen)",
        "S_VOBSUB" => "VobSub (imagen)",
        "S_DVBSUB" => "DVB (imagen)",
        other => other,
    }
    .to_string()
}

/// Texto de un bloque de subtítulo listo para el cuerpo de un cue WebVTT.
pub fn to_vtt_text(kind: SubKind, data: &[u8]) -> Option<String> {
    let raw = String::from_utf8_lossy(data);
    let text = match kind {
        SubKind::Srt | SubKind::WebVtt => clean_html(&raw),
        SubKind::Ass => ass_text(&raw),
    };
    // Una línea en blanco cerraría el cue; "-->" lo rompería.
    let lines: Vec<String> = text
        .lines()
        .map(|l| l.trim().replace("-->", "→"))
        .filter(|l| !l.is_empty())
        .collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// Deja solo <i>, <b> y <u> (lo que WebVTT entiende) y escapa '&'.
fn clean_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('<') {
        out.push_str(&escape_amp(&rest[..i]));
        let tail = &rest[i..];
        match tail.find('>') {
            Some(j) => {
                let tag = tail[1..j].trim().to_ascii_lowercase();
                if matches!(tag.as_str(), "i" | "/i" | "b" | "/b" | "u" | "/u") {
                    out.push('<');
                    out.push_str(&tag);
                    out.push('>');
                }
                rest = &tail[j + 1..];
            }
            None => {
                out.push_str("&lt;");
                rest = &tail[1..];
            }
        }
    }
    out.push_str(&escape_amp(rest));
    out
}

fn escape_amp(s: &str) -> String {
    s.replace('&', "&amp;")
}

/// Un bloque ASS en Matroska es: ReadOrder,Layer,Style,Name,MarginL,MarginR,MarginV,Effect,Text
fn ass_text(s: &str) -> String {
    let text = s.splitn(9, ',').nth(8).unwrap_or(s);
    let mut out = String::with_capacity(text.len());
    let mut italic = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' => {
                // Etiquetas de estilo: solo conservamos la cursiva.
                let mut tag = String::new();
                for t in chars.by_ref() {
                    if t == '}' {
                        break;
                    }
                    tag.push(t);
                }
                if tag.contains("\\i1") && !italic {
                    out.push_str("<i>");
                    italic = true;
                } else if tag.contains("\\i0") && italic {
                    out.push_str("</i>");
                    italic = false;
                }
            }
            '\\' => match chars.peek() {
                Some('N') | Some('n') => {
                    chars.next();
                    out.push('\n');
                }
                Some('h') => {
                    chars.next();
                    out.push(' ');
                }
                _ => out.push('\\'),
            },
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            _ => out.push(c),
        }
    }
    if italic {
        out.push_str("</i>");
    }
    out
}

/// Marca de tiempo WebVTT (hh:mm:ss.mmm).
pub fn vtt_time(secs: f64) -> String {
    let ms = (secs.max(0.0) * 1000.0).round() as u64;
    format!("{:02}:{:02}:{:02}.{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

/// ISO 639-2 (Matroska) → etiqueta BCP 47 corta para HLS.
pub fn lang_tag(code: &str) -> String {
    let c = code.to_ascii_lowercase();
    let short = match c.as_str() {
        "spa" | "es" => "es",
        "eng" | "en" => "en",
        "fre" | "fra" | "fr" => "fr",
        "ger" | "deu" | "de" => "de",
        "ita" | "it" => "it",
        "por" | "pt" => "pt",
        "jpn" | "ja" => "ja",
        "chi" | "zho" | "zh" => "zh",
        "kor" | "ko" => "ko",
        "rus" | "ru" => "ru",
        "dut" | "nld" | "nl" => "nl",
        "cat" | "ca" => "ca",
        "ara" | "ar" => "ar",
        "hin" | "hi" => "hi",
        _ => return code.to_string(),
    };
    short.to_string()
}

pub fn lang_name(tag: &str) -> Option<&'static str> {
    let base = tag.split('-').next().unwrap_or(tag);
    Some(match base {
        "es" => "Español",
        "en" => "English",
        "fr" => "Français",
        "de" => "Deutsch",
        "it" => "Italiano",
        "pt" => "Português",
        "ja" => "日本語",
        "zh" => "中文",
        "ko" => "한국어",
        "ru" => "Русский",
        "nl" => "Nederlands",
        "ca" => "Català",
        "ar" => "العربية",
        "hi" => "हिन्दी",
        _ => return None,
    })
}

/// Cadena RFC 6381 para H.264 a partir del avcC.
pub fn avc_codec(avcc: &[u8]) -> Option<String> {
    (avcc.len() >= 4).then(|| format!("avc1.{:02X}{:02X}{:02X}", avcc[1], avcc[2], avcc[3]))
}

/// Cadena RFC 6381 para HEVC a partir del hvcC (formato de Apple: hvc1.P.C.TL.B…).
pub fn hevc_codec(hvcc: &[u8]) -> Option<String> {
    if hvcc.len() < 13 {
        return None;
    }
    let space = ["", "A", "B", "C"][(hvcc[1] >> 6) as usize];
    let tier = if hvcc[1] & 0x20 != 0 { "H" } else { "L" };
    let profile = hvcc[1] & 0x1F;
    let compat = u32::from_be_bytes([hvcc[2], hvcc[3], hvcc[4], hvcc[5]]).reverse_bits();
    let level = hvcc[12];
    let mut s = format!("hvc1.{space}{profile}.{compat:X}.{tier}{level}");
    let constraints = &hvcc[6..12];
    let last = constraints.iter().rposition(|&b| b != 0).map(|i| i + 1).unwrap_or(0);
    for b in &constraints[..last] {
        s.push_str(&format!(".{b:X}"));
    }
    Some(s)
}

/// mp4a.40.N a partir del AudioSpecificConfig.
pub fn aac_codec(asc: &[u8]) -> String {
    let aot = asc.first().map(|b| b >> 3).filter(|&a| a > 0 && a < 31).unwrap_or(2);
    format!("mp4a.40.{aot}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srt_tags() {
        let t = to_vtt_text(SubKind::Srt, b"<font color=\"#fff\">Hola</font> <i>mundo</i> & co\r\n\r\nfin").unwrap();
        assert_eq!(t, "Hola <i>mundo</i> &amp; co\nfin");
    }

    #[test]
    fn ass_fields() {
        let t = to_vtt_text(SubKind::Ass, b"3,0,Default,,0,0,0,,{\\an8}{\\i1}Hola,{\\i0} amigo\\Nsegunda").unwrap();
        assert_eq!(t, "<i>Hola,</i> amigo\nsegunda");
    }

    #[test]
    fn times() {
        assert_eq!(vtt_time(3725.5), "01:02:05.500");
    }

    #[test]
    fn hevc_main10() {
        // Main 10, tier Main, nivel 5.0 (150), compat 0x20000000, constraint B0
        let mut h = vec![1u8, 0x02, 0x20, 0, 0, 0, 0xB0, 0, 0, 0, 0, 0, 150];
        h.resize(23, 0);
        assert_eq!(hevc_codec(&h).unwrap(), "hvc1.2.4.L150.B0");
    }

    #[test]
    fn languages() {
        assert_eq!(lang_tag("spa"), "es");
        assert_eq!(lang_name("es"), Some("Español"));
    }
}
