//! Convierte un MKV en una presentación HLS (fMP4) bajo demanda, sin recodificar.

use std::path::{Path, PathBuf};

use crate::audio::{self, AudioKind};
use crate::fmp4::AudioCodec;
use crate::subs::{self, SubKind};
use crate::fmp4::{self, Sample, TrackFragment, VideoCodec, VideoParams};
use crate::mkv::{self, Control, Event, Keyframe, MkvHeader, Track};
use crate::CoreError;

/// Duración objetivo de cada segmento HLS.
const TARGET_SEGMENT_SECS: f64 = 6.0;
/// Modo compatible: duración máxima de cada pieza. Los receptores AirPlay con poca memoria
/// (p. ej. un Roku Express) fallan con segmentos grandes (−12862 con uno de 9 MB).
const COMPAT_PART_SECS: f64 = 2.0;

impl From<mkv::MkvError> for CoreError {
    fn from(e: mkv::MkvError) -> Self {
        match e {
            mkv::MkvError::Io(e) => CoreError::Io { detail: e.to_string() },
            mkv::MkvError::Invalid(m) => CoreError::InvalidFile { detail: m },
        }
    }
}

pub fn video_supported(t: &Track) -> bool {
    !t.unsupported_encoding
        && !t.codec_private.is_empty()
        && (t.codec_id == "V_MPEG4/ISO/AVC" || t.codec_id == "V_MPEGH/ISO/HEVC")
}

pub fn audio_supported(t: &Track) -> bool {
    !t.unsupported_encoding && audio::kind_of(&t.codec_id).is_some()
}

struct AudioSel {
    track: u64,
    timescale: u32,
    samples_per_frame: u32,
    /// Modo compatible: el AC-3/E-AC-3 original se convierte a AAC estéreo.
    transcode: Option<Transcode>,
}

struct Transcode {
    codec: &'static str,
    channels: u32,
    /// Muestras por frame del audio original (1536 en AC-3).
    src_spf: u32,
}

/// Frames de audio original que se decodifican antes/después del segmento, para que
/// el decodificador y el codificador AAC ya estén "calientes" en los bordes.
const TRANSCODE_PREROLL_FRAMES: i64 = 8;
const TRANSCODE_POSTROLL_FRAMES: i64 = 4;
const AAC_FRAME: i64 = 1024;

#[derive(Clone, Debug)]
pub struct Segment {
    /// Inicio (ticks) = PTS del keyframe que abre el segmento.
    pub start: i64,
    /// Fin (ticks) = PTS del keyframe del siguiente segmento, o la duración total.
    pub end: i64,
    pub first_kf: usize,
    pub is_last: bool,
}

/// Pieza publicada en la lista HLS. Normalmente es un segmento entero; en modo
/// compatible, los segmentos largos se parten en piezas de ≤ COMPAT_PART_SECS cortando
/// en orden de decodificación (solo la primera pieza empieza con keyframe).
#[derive(Clone, Debug)]
struct Part {
    parent: usize,
    start: i64,
    end: i64,
    first_of_parent: bool,
    last_of_parent: bool,
    is_last: bool,
}

pub struct Media {
    path: PathBuf,
    header: MkvHeader,
    video_track: u64,
    audio: Option<AudioSel>,
    keyframes: Vec<Keyframe>,
    segments: Vec<Segment>,
    parts: Vec<Part>,
    /// Retardo de reordenamiento (frames B), en ticks.
    reorder: i64,
    video_frame_ticks: Option<i64>,
    init: Vec<u8>,
    subtitles: Vec<SubTrack>,
    /// Cadena RFC 6381 (se conserva para diagnóstico; no se publica en la master playlist).
    #[allow(dead_code)]
    codecs: String,
    bandwidth: u64,
    resolution: (u32, u32),
}

/// Pista de subtítulos de texto que se publica como WebVTT.
pub struct SubTrack {
    pub number: u64,
    kind: SubKind,
    language: String,
    name: String,
    forced: bool,
    default_duration: Option<i64>,
}

pub fn subtitle_supported(t: &Track) -> bool {
    !t.unsupported_encoding && subs::kind_of(&t.codec_id).is_some()
}

impl Media {
    /// `compat`: modo para teles AirPlay de otras marcas (Roku, Samsung, LG…): piezas
    /// cortas y el audio Dolby Digital convertido a AAC estéreo.
    /// `subtitles`: incluir los subtítulos de texto (con ellos, la lista principal es una
    /// master playlist; algunas teles AirPlay de otras marcas no la aceptan).
    pub fn open(path: &Path, audio_track: Option<u64>, compat: bool, subtitles: bool) -> Result<Self, CoreError> {
        let (header, mut r) = mkv::read_header(path)?;

        let video = header
            .tracks
            .iter()
            .find(|t| t.kind == mkv::TRACK_VIDEO && t.enabled)
            .ok_or_else(|| CoreError::Unsupported { detail: "el archivo no tiene pista de video".into() })?
            .clone();
        if !video_supported(&video) {
            return Err(CoreError::Unsupported {
                detail: format!(
                    "el video es {} y AirPlay solo acepta H.264 o HEVC sin recodificar",
                    audio::friendly_codec(&video.codec_id)
                ),
            });
        }

        let audio_track = match audio_track {
            Some(n) => {
                let t = header
                    .track(n)
                    .ok_or_else(|| CoreError::InvalidFile { detail: format!("no existe la pista {n}") })?;
                if !audio_supported(t) {
                    return Err(CoreError::Unsupported {
                        detail: format!("el audio {} no es compatible con AirPlay", audio::friendly_codec(&t.codec_id)),
                    });
                }
                Some(t.clone())
            }
            None => {
                let candidates: Vec<&Track> =
                    header.tracks.iter().filter(|t| t.kind == mkv::TRACK_AUDIO && audio_supported(t)).collect();
                candidates.iter().find(|t| t.is_default).or(candidates.first()).map(|t| (*t).clone())
            }
        };

        // Índice de keyframes: Cues si existen; si no, recorrer el archivo.
        let mut keyframes = mkv::read_cues(&mut r, &header, video.number)?;
        if keyframes.len() < 2 {
            keyframes = mkv::scan_keyframes(&mut r, &header, video.number)?;
        }
        if keyframes.is_empty() {
            return Err(CoreError::InvalidFile { detail: "no se encontraron keyframes de video".into() });
        }

        // Primer frame de audio (para dac3/dec3) y reordenamiento de frames B.
        let first_cluster = header.first_cluster_pos.unwrap_or(header.segment_data_start);
        let audio_num = audio_track.as_ref().map(|t| t.number);
        let mut first_audio: Option<Vec<u8>> = None;
        let mut video_pts: Vec<i64> = vec![];
        mkv::walk_clusters(&mut r, &header, first_cluster, &|t| Some(t) == audio_num, &mut |ev| {
            if let Event::Block(b) = ev {
                if b.track == video.number && video_pts.len() < 300 {
                    video_pts.push(b.ts);
                } else if Some(b.track) == audio_num && first_audio.is_none() {
                    first_audio = b.frames.first().cloned();
                }
            }
            if video_pts.len() >= 300 && (audio_num.is_none() || first_audio.is_some()) {
                Control::Stop
            } else {
                Control::Continue
            }
        })?;
        let reorder = reorder_delay(&video_pts);

        let audio = match &audio_track {
            Some(t) => {
                let kind = audio::kind_of(&t.codec_id).unwrap_or(AudioKind::Aac);
                let frame = first_audio.unwrap_or_default();
                match audio::audio_params(t, kind, &frame) {
                    Some((params, spf)) if compat && kind != AudioKind::Aac => {
                        if crate::transcoder().is_none() {
                            return Err(CoreError::Unsupported {
                                detail: "el modo compatible necesita un conversor de audio y no hay ninguno instalado".into(),
                            });
                        }
                        let rate = params.sample_rate;
                        Some((
                            AudioSel {
                                track: t.number,
                                timescale: rate,
                                samples_per_frame: AAC_FRAME as u32,
                                transcode: Some(Transcode {
                                    codec: if kind == AudioKind::Ac3 { "ac-3" } else { "ec-3" },
                                    channels: t.channels.max(1),
                                    src_spf: spf,
                                }),
                            },
                            fmp4::AudioParams { codec: AudioCodec::Aac(audio::aac_lc_asc(rate, 2)), sample_rate: rate, channels: 2 },
                        ))
                    }
                    Some((params, spf)) => Some((
                        AudioSel { track: t.number, timescale: params.sample_rate, samples_per_frame: spf, transcode: None },
                        params,
                    )),
                    None => {
                        return Err(CoreError::Unsupported {
                            detail: format!("no se pudo leer la cabecera de audio {}", audio::friendly_codec(&t.codec_id)),
                        })
                    }
                }
            }
            None => None,
        };

        let codec = if video.codec_id == "V_MPEG4/ISO/AVC" {
            VideoCodec::Avc(video.codec_private.clone())
        } else {
            VideoCodec::Hevc(video.codec_private.clone())
        };
        let (w, h) = (video.pixel_width.max(1), video.pixel_height.max(1));
        let vparams = VideoParams {
            codec,
            width: w,
            height: h,
            display_width: video.display_width.unwrap_or(w),
            display_height: video.display_height.unwrap_or(h),
        };
        let init = fmp4::init_segment(&vparams, audio.as_ref().map(|(_, p)| p));

        let mut codecs = match &vparams.codec {
            VideoCodec::Avc(c) => subs::avc_codec(c),
            VideoCodec::Hevc(c) => subs::hevc_codec(c),
        }
        .unwrap_or_default();
        if let Some((_, p)) = &audio {
            codecs.push(',');
            codecs.push_str(&match &p.codec {
                AudioCodec::Aac(asc) => subs::aac_codec(asc),
                AudioCodec::Ac3(_) => "ac-3".into(),
                AudioCodec::Eac3(_) => "ec-3".into(),
            });
        }

        let scale = header.timestamp_scale as f64;
        let target = (TARGET_SEGMENT_SECS * 1e9 / scale) as i64;
        let last_kf = keyframes.last().map(|k| k.ts).unwrap_or(0);
        let total_end = header
            .duration_ticks
            .map(|d| d.ceil() as i64)
            .filter(|&d| d > last_kf)
            .unwrap_or(last_kf + (10.0 * 1e9 / scale) as i64);
        let segments = plan_segments(&keyframes, target, total_end);
        let part_len = if compat { Some((COMPAT_PART_SECS * 1e9 / scale) as i64) } else { None };
        let parts = plan_parts(&segments, part_len);
        let video_frame_ticks = video.default_duration_ns.map(|d| (d as f64 / scale).round() as i64).filter(|&d| d > 0);

        let mut used_names: Vec<String> = vec![];
        let subtitles = header
            .tracks
            .iter()
            .filter(|t| subtitles && t.kind == mkv::TRACK_SUBTITLE && subtitle_supported(t))
            .map(|t| {
                let language = t
                    .language
                    .as_deref()
                    .filter(|l| !l.is_empty() && *l != "und")
                    .map(subs::lang_tag)
                    .unwrap_or_else(|| "und".into());
                let base = t
                    .name
                    .clone()
                    .filter(|n| !n.trim().is_empty() && !n.to_lowercase().contains("www."))
                    .or_else(|| subs::lang_name(&language).map(|n| n.to_string()))
                    .unwrap_or_else(|| format!("Subtítulos {}", t.number));
                let base = if t.forced && !base.to_lowercase().contains("forz") { format!("{base} (forzados)") } else { base };
                // NAME debe ser único dentro del grupo.
                let mut name = base.clone();
                let mut i = 2;
                while used_names.contains(&name) {
                    name = format!("{base} {i}");
                    i += 1;
                }
                used_names.push(name.clone());
                SubTrack {
                    number: t.number,
                    kind: subs::kind_of(&t.codec_id).unwrap_or(SubKind::Srt),
                    language,
                    name,
                    forced: t.forced,
                    default_duration: t.default_duration_ns.map(|d| (d as f64 / scale).round() as i64),
                }
            })
            .collect();

        let duration_secs = (total_end as f64 * scale / 1e9).max(1.0);
        let file_len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let bandwidth = ((file_len as f64 * 8.0 / duration_secs) * 1.5) as u64;

        Ok(Media {
            path: path.to_path_buf(),
            header,
            video_track: video.number,
            audio: audio.map(|(a, _)| a),
            keyframes,
            segments,
            parts,
            reorder,
            video_frame_ticks,
            init,
            subtitles,
            codecs,
            bandwidth: bandwidth.max(100_000),
            resolution: (vparams.display_width, vparams.display_height),
        })
    }

    fn secs(&self, ticks: i64) -> f64 {
        ticks as f64 * self.header.timestamp_scale as f64 / 1e9
    }

    pub fn init_segment(&self) -> &[u8] {
        &self.init
    }

    /// Lista principal: master playlist si hay subtítulos; si no, directamente la de video.
    pub fn playlist(&self) -> String {
        if self.subtitles.is_empty() {
            return self.video_playlist();
        }
        let mut out = String::from("#EXTM3U\n#EXT-X-VERSION:7\n");
        // En modo compatible las piezas intermedias no empiezan en keyframe.
        if self.parts.len() == self.segments.len() {
            out.push_str("#EXT-X-INDEPENDENT-SEGMENTS\n");
        }
        for s in &self.subtitles {
            out.push_str(&format!(
                "#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"subs\",NAME=\"{}\",LANGUAGE=\"{}\",DEFAULT=NO,AUTOSELECT={},FORCED={},URI=\"subs/{}.m3u8\"\n",
                s.name.replace('"', "'"),
                s.language,
                if s.forced || s.language != "und" { "YES" } else { "NO" },
                if s.forced { "YES" } else { "NO" },
                s.number
            ));
        }
        // Sin CODECS: los receptores AirPlay de otras marcas (Roku, Samsung…) descartan la
        // variante si creen no poder decodificar el audio (p. ej. "ac-3" en un Roku Express),
        // aunque luego lo envíen tal cual a la tele. Sin el atributo, prueban a reproducirla.
        out.push_str(&format!(
            "#EXT-X-STREAM-INF:BANDWIDTH={},RESOLUTION={}x{},SUBTITLES=\"subs\"\nvideo.m3u8\n",
            self.bandwidth, self.resolution.0, self.resolution.1
        ));
        out
    }

    pub fn video_playlist(&self) -> String {
        let mut out = String::new();
        out.push_str("#EXTM3U\n#EXT-X-VERSION:7\n");
        out.push_str(&format!("#EXT-X-TARGETDURATION:{}\n", self.target_duration()));
        out.push_str("#EXT-X-PLAYLIST-TYPE:VOD\n");
        if self.parts.len() == self.segments.len() {
            out.push_str("#EXT-X-INDEPENDENT-SEGMENTS\n");
        }
        out.push_str("#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-MAP:URI=\"init.mp4\"\n");
        for (i, p) in self.parts.iter().enumerate() {
            out.push_str(&format!("#EXTINF:{:.5},\nseg{}.m4s\n", self.secs(p.end - p.start), i));
        }
        out.push_str("#EXT-X-ENDLIST\n");
        out
    }

    fn target_duration(&self) -> u64 {
        let max = self.parts.iter().map(|p| self.secs(p.end - p.start)).fold(0.0, f64::max);
        max.ceil().max(1.0) as u64
    }

    pub fn subtitle_playlist(&self, track: u64) -> Option<String> {
        self.subtitles.iter().find(|s| s.number == track)?;
        let mut out = String::from("#EXTM3U\n#EXT-X-VERSION:7\n");
        out.push_str(&format!("#EXT-X-TARGETDURATION:{}\n", self.target_duration()));
        out.push_str("#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-MEDIA-SEQUENCE:0\n");
        for (i, p) in self.parts.iter().enumerate() {
            out.push_str(&format!("#EXTINF:{:.5},\n{}/seg{}.vtt\n", self.secs(p.end - p.start), track, i));
        }
        out.push_str("#EXT-X-ENDLIST\n");
        Some(out)
    }

    /// Segmento WebVTT `n` de la pista `track`: los cues que se solapan con el segmento.
    pub fn subtitle_segment(&self, track: u64, n: usize) -> Result<String, CoreError> {
        let sub = self
            .subtitles
            .iter()
            .find(|s| s.number == track)
            .ok_or_else(|| CoreError::InvalidFile { detail: format!("no hay subtítulos en la pista {track}") })?;
        let part = self
            .parts
            .get(n)
            .ok_or_else(|| CoreError::InvalidFile { detail: format!("segmento {n} fuera de rango") })?
            .clone();
        let seg = Segment { start: part.start, end: part.end, ..self.segments[part.parent].clone() };
        let (_, mut r) = mkv::read_header(&self.path)?;
        let fallback = sub.default_duration.filter(|&d| d > 0).unwrap_or((3.0 * 1e9 / self.header.timestamp_scale as f64) as i64);
        let mut cues: Vec<(i64, i64, String)> = vec![];
        mkv::walk_clusters(&mut r, &self.header, self.read_start(&seg), &|t| t == track, &mut |ev| {
            match ev {
                Event::Cluster { ts, .. } if ts > seg.end => return Control::Stop,
                Event::Block(b) if b.track == track => {
                    let end = b.ts + b.duration.filter(|&d| d > 0).unwrap_or(fallback);
                    if end > seg.start && b.ts < seg.end {
                        for f in &b.frames {
                            if let Some(text) = subs::to_vtt_text(sub.kind, f) {
                                cues.push((b.ts, end, text));
                            }
                        }
                    }
                }
                _ => {}
            }
            Control::Continue
        })?;
        cues.sort_by_key(|c| c.0);
        let mut out = String::from("WEBVTT\nX-TIMESTAMP-MAP=MPEGTS:0,LOCAL:00:00:00.000\n\n");
        for (start, end, text) in cues {
            // Misma línea de tiempo que el video (presentación desplazada +reorder).
            out.push_str(&format!(
                "{} --> {}\n{}\n\n",
                subs::vtt_time(self.secs(start + self.reorder)),
                subs::vtt_time(self.secs(end + self.reorder)),
                text
            ));
        }
        Ok(out)
    }

    /// MP4 autocontenido (init + el segmento que contiene `at_seconds`) para generar miniaturas.
    pub fn clip_at(&self, at_seconds: f64) -> Result<Vec<u8>, CoreError> {
        let n = self
            .parts
            .iter()
            .position(|p| p.first_of_parent && self.secs(self.segments[p.parent].end) > at_seconds)
            .unwrap_or(0);
        let mut out = self.init.clone();
        out.extend(self.segment(n)?);
        Ok(out)
    }

    /// Convierte el audio original del segmento a AAC con el conversor de la plataforma.
    ///
    /// Los frames AAC se alinean a una rejilla global de 1024 muestras (frame k = muestras
    /// [k·1024, (k+1)·1024) de la línea de tiempo), así que los segmentos, generados por
    /// separado, encajan sin huecos ni solapes: cada uno se queda con los k de su rango.
    fn transcode_segment(
        &self,
        a: &AudioSel,
        seg: &Part,
        frames: &[(i64, Vec<u8>)],
    ) -> Result<(u64, Vec<Vec<u8>>), CoreError> {
        let t = a.transcode.as_ref().expect("modo compatible");
        let transcoder = crate::transcoder()
            .ok_or_else(|| CoreError::Unsupported { detail: "no hay conversor de audio".into() })?;
        let rate = a.timescale;
        let first_sample = self.to_audio(frames[0].0 + self.reorder, rate) as i64;
        let out = transcoder
            .to_aac(t.codec.to_string(), rate, t.channels, first_sample, frames.iter().map(|f| f.1.clone()).collect())
            .map_err(|e| CoreError::Unsupported { detail: format!("no se pudo convertir el audio: {e}") })?;

        let start = self.to_audio(seg.start + self.reorder, rate) as i64;
        let k0 = (start + AAC_FRAME - 1).div_euclid(AAC_FRAME);
        let available_end = out.first_frame_index + out.packets.len() as i64;
        let k1 = if seg.is_last {
            available_end
        } else {
            let end = self.to_audio(seg.end + self.reorder, rate) as i64;
            (end + AAC_FRAME - 1).div_euclid(AAC_FRAME)
        };
        let from = (k0 - out.first_frame_index).max(0);
        let to = (k1 - out.first_frame_index).min(out.packets.len() as i64);
        if from >= to {
            return Ok(((k0.max(0) * AAC_FRAME) as u64, vec![]));
        }
        if from + out.first_frame_index != k0 || to + out.first_frame_index != k1 {
            crate::emit(format!(
                "audio AAC incompleto en el segmento: frames {}..{} de {}..{}",
                from + out.first_frame_index,
                to + out.first_frame_index,
                k0,
                k1
            ));
        }
        let base = ((from + out.first_frame_index) * AAC_FRAME) as u64;
        Ok((base, out.packets[from as usize..to as usize].to_vec()))
    }

    /// Inicio (en segundos de presentación) del segmento con keyframe que contiene `seconds`.
    /// En modo compatible conviene empezar ahí: las piezas intermedias no traen keyframe.
    pub fn keyframe_start_at(&self, seconds: f64) -> f64 {
        let seg = self
            .segments
            .iter()
            .rev()
            .find(|s| self.secs(s.start + self.reorder) <= seconds)
            .or(self.segments.first());
        seg.map(|s| self.secs(s.start + self.reorder)).unwrap_or(0.0)
    }

    /// Cluster desde el que leer un segmento (el del keyframe anterior, para no perder
    /// audio o subtítulos que empiezan un poco antes del keyframe).
    fn read_start(&self, seg: &Segment) -> u64 {
        if seg.first_kf > 0 {
            self.keyframes[seg.first_kf - 1].cluster_pos
        } else {
            self.keyframes[0].cluster_pos
        }
    }

    fn to_90k(&self, ticks: i64) -> i64 {
        (ticks as i128 * self.header.timestamp_scale as i128 * 9 / 100_000) as i64
    }

    fn to_audio(&self, ticks: i64, rate: u32) -> u64 {
        let v = ticks as i128 * self.header.timestamp_scale as i128 * rate as i128 / 1_000_000_000;
        v.max(0) as u64
    }

    /// Genera el segmento `n` (moof + mdat).
    pub fn segment(&self, n: usize) -> Result<Vec<u8>, CoreError> {
        let part = self
            .parts
            .get(n)
            .ok_or_else(|| CoreError::InvalidFile { detail: format!("segmento {n} fuera de rango") })?
            .clone();
        let seg = self.segments[part.parent].clone();
        let start_pos = self.read_start(&seg);
        let (_, mut r) = mkv::read_header(&self.path)?;

        let video_num = self.video_track;
        let audio_num = self.audio.as_ref().map(|a| a.track);
        let audio_slack = (3.0 * 1e9 / self.header.timestamp_scale as f64) as i64;

        let mut in_seg = false;
        let mut video_done = false;
        let mut v_pts: Vec<i64> = vec![];
        let mut v_key: Vec<bool> = vec![];
        let mut v_data: Vec<Vec<u8>> = vec![];
        let mut a_frames: Vec<(i64, Vec<u8>)> = vec![];
        // En modo compatible se leen unos frames de más a cada lado (ver TRANSCODE_PREROLL_FRAMES).
        let (a_from, a_to) = match self.audio.as_ref().and_then(|a| a.transcode.as_ref().map(|t| (a, t))) {
            Some((a, t)) => {
                let frame = (t.src_spf as f64 * 1e9 / a.timescale as f64 / self.header.timestamp_scale as f64).round() as i64;
                (part.start - TRANSCODE_PREROLL_FRAMES * frame, part.end + TRANSCODE_POSTROLL_FRAMES * frame)
            }
            None => (part.start, part.end),
        };
        let default_frame = self.video_frame_ticks.unwrap_or(0);

        mkv::walk_clusters(
            &mut r,
            &self.header,
            start_pos,
            &|t| t == video_num || Some(t) == audio_num,
            &mut |ev| {
                match ev {
                    Event::Cluster { ts, .. } => {
                        if video_done && ts > seg.end + audio_slack {
                            return Control::Stop;
                        }
                    }
                    Event::Block(b) if b.track == video_num => {
                        if video_done {
                            return Control::Continue;
                        }
                        for (i, f) in b.frames.iter().enumerate() {
                            let ts = b.ts + i as i64 * default_frame;
                            if !in_seg {
                                if b.keyframe && ts == seg.start {
                                    in_seg = true;
                                } else {
                                    continue;
                                }
                            } else if b.keyframe && !seg.is_last && ts == seg.end {
                                video_done = true;
                                break;
                            }
                            v_pts.push(ts);
                            v_key.push(b.keyframe && i == 0);
                            v_data.push(f.clone());
                        }
                    }
                    Event::Block(b) if Some(b.track) == audio_num => {
                        let a = self.audio.as_ref().unwrap();
                        let spf = a.transcode.as_ref().map(|t| t.src_spf).unwrap_or(a.samples_per_frame);
                        let frame_ticks = spf as f64 * 1e9 / a.timescale as f64 / self.header.timestamp_scale as f64;
                        for (i, f) in b.frames.iter().enumerate() {
                            let ts = b.ts + (i as f64 * frame_ticks).round() as i64;
                            if ts >= a_from && ts < a_to {
                                a_frames.push((ts, f.clone()));
                            }
                        }
                    }
                    _ => {}
                }
                Control::Continue
            },
        )?;

        let mut tracks = vec![];

        // --- Video: DTS = PTS ordenados (en orden de decodificación); presentación desplazada +reorder.
        if !v_pts.is_empty() {
            let mut sorted = v_pts.clone();
            sorted.sort_unstable();
            let dts: Vec<i64> = sorted.iter().map(|&t| self.to_90k(t)).collect();
            let next_dts = if seg.is_last { None } else { Some(self.to_90k(seg.end)) };
            // Pieza: frames (en orden de decodificación) cuyo DTS cae en [part.start, part.end).
            let from = if part.first_of_parent { 0 } else { sorted.partition_point(|&t| t < part.start) };
            let to = if part.last_of_parent { v_pts.len() } else { sorted.partition_point(|&t| t < part.end) };
            let mut samples = Vec::with_capacity(to.saturating_sub(from));
            let mut data = Vec::new();
            for i in from..to {
                let dur = if i + 1 < dts.len() {
                    dts[i + 1] - dts[i]
                } else {
                    match next_dts {
                        Some(nd) if nd > dts[i] => nd - dts[i],
                        _ => avg_duration(&dts).max(1),
                    }
                };
                let cto = self.to_90k(v_pts[i] + self.reorder) - dts[i];
                samples.push(Sample {
                    duration: dur.max(1) as u32,
                    size: v_data[i].len() as u32,
                    keyframe: v_key[i],
                    cto: cto as i32,
                });
                data.extend_from_slice(&v_data[i]);
            }
            if from < to {
                tracks.push(TrackFragment {
                    track_id: fmp4::VIDEO_TRACK_ID,
                    base_decode_time: dts[from].max(0) as u64,
                    samples,
                    data,
                    is_video: true,
                });
            }
        }

        // --- Audio: muestras consecutivas de duración fija.
        a_frames.sort_by_key(|f| f.0);
        let audio_packets: Option<(u64, Vec<Vec<u8>>)> = match &self.audio {
            Some(a) if a.transcode.is_some() && !a_frames.is_empty() => Some(self.transcode_segment(a, &part, &a_frames)?),
            Some(a) => {
                let kept: Vec<&(i64, Vec<u8>)> = a_frames.iter().filter(|f| f.0 >= part.start && f.0 < part.end).collect();
                kept.first().map(|first| {
                    (self.to_audio(first.0 + self.reorder, a.timescale), kept.iter().map(|f| f.1.clone()).collect())
                })
            }
            None => None,
        };
        if let (Some(a), Some((base, packets))) = (&self.audio, audio_packets) {
            let mut data = Vec::with_capacity(packets.iter().map(|d| d.len()).sum());
            let samples = packets
                .iter()
                .map(|d| {
                    data.extend_from_slice(d);
                    Sample { duration: a.samples_per_frame, size: d.len() as u32, keyframe: true, cto: 0 }
                })
                .collect();
            tracks.push(TrackFragment {
                track_id: fmp4::AUDIO_TRACK_ID,
                base_decode_time: base,
                samples,
                data,
                is_video: false,
            });
        }

        if tracks.is_empty() {
            return Err(CoreError::InvalidFile { detail: format!("el segmento {n} quedó vacío") });
        }
        Ok(fmp4::media_segment(n as u32 + 1, &tracks))
    }
}

fn avg_duration(dts: &[i64]) -> i64 {
    if dts.len() < 2 {
        return 3003; // ~29.97 fps
    }
    (dts[dts.len() - 1] - dts[0]) / (dts.len() as i64 - 1)
}

/// Cuánto se adelanta la decodificación respecto a la presentación (frames B).
fn reorder_delay(pts_decode_order: &[i64]) -> i64 {
    let mut sorted = pts_decode_order.to_vec();
    sorted.sort_unstable();
    sorted.iter().zip(pts_decode_order).map(|(s, p)| s - p).max().unwrap_or(0).max(0)
}

fn plan_parts(segments: &[Segment], max_len: Option<i64>) -> Vec<Part> {
    let mut parts = vec![];
    for (i, s) in segments.iter().enumerate() {
        let dur = (s.end - s.start).max(1);
        let count = match max_len {
            Some(m) if m > 0 => ((dur + m - 1) / m).max(1),
            _ => 1,
        };
        for k in 0..count {
            let start = s.start + dur * k / count;
            let end = if k + 1 == count { s.end } else { s.start + dur * (k + 1) / count };
            parts.push(Part {
                parent: i,
                start,
                end,
                first_of_parent: k == 0,
                last_of_parent: k + 1 == count,
                is_last: s.is_last && k + 1 == count,
            });
        }
    }
    parts
}

fn plan_segments(kfs: &[Keyframe], target: i64, total_end: i64) -> Vec<Segment> {
    let mut starts: Vec<usize> = vec![0];
    for (i, k) in kfs.iter().enumerate().skip(1) {
        let cur = kfs[*starts.last().unwrap()].ts;
        if k.ts - cur >= target {
            starts.push(i);
        }
    }
    let mut segs = vec![];
    for (j, &i) in starts.iter().enumerate() {
        let is_last = j + 1 == starts.len();
        let end = if is_last { total_end } else { kfs[starts[j + 1]].ts };
        segs.push(Segment { start: kfs[i].ts, end, first_kf: i, is_last });
    }
    segs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reorder_with_b_frames() {
        // Orden de decodificación I0 P3 B1 B2
        assert_eq!(reorder_delay(&[0, 3, 1, 2]), 1);
        assert_eq!(reorder_delay(&[0, 1, 2, 3]), 0);
    }

    #[test]
    fn parts_split_long_segments() {
        let segs = vec![
            Segment { start: 0, end: 5000, first_kf: 0, is_last: false },
            Segment { start: 5000, end: 6000, first_kf: 1, is_last: true },
        ];
        let p = plan_parts(&segs, Some(2000));
        let b: Vec<(i64, i64, bool, bool)> = p.iter().map(|p| (p.start, p.end, p.first_of_parent, p.is_last)).collect();
        assert_eq!(b, vec![(0, 1666, true, false), (1666, 3333, false, false), (3333, 5000, false, false), (5000, 6000, true, true)]);
        assert_eq!(plan_parts(&segs, None).len(), 2);
    }

    #[test]
    fn segments_group_keyframes() {
        let kfs: Vec<Keyframe> = [0, 2000, 4000, 6000, 8000, 12000, 13000]
            .iter()
            .map(|&ts| Keyframe { ts, cluster_pos: 0 })
            .collect();
        let s = plan_segments(&kfs, 6000, 20000);
        let bounds: Vec<(i64, i64)> = s.iter().map(|s| (s.start, s.end)).collect();
        assert_eq!(bounds, vec![(0, 6000), (6000, 12000), (12000, 20000)]);
    }
}
