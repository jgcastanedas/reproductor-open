//! Núcleo en Rust del reproductor AirPlay.
//!
//! - `probe`: describe un archivo MKV (códecs, pistas de audio, compatibilidad).
//! - `StreamServer`: servidor HTTP local que remuxea MKV → HLS fMP4 sin recodificar,
//!   para que AVPlayer lo reproduzca y lo envíe al Apple TV por AirPlay.

mod audio;
mod fmp4;
mod media;
mod mkv;
mod server;
mod subs;

use std::path::Path;
use std::sync::{Arc, RwLock};

uniffi::setup_scaffolding!();

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum CoreError {
    #[error("{detail}")]
    Io { detail: String },
    #[error("{detail}")]
    InvalidFile { detail: String },
    #[error("{detail}")]
    Unsupported { detail: String },
    #[error("{detail}")]
    Server { detail: String },
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct AudioTrackInfo {
    pub id: u64,
    pub codec: String,
    pub language: Option<String>,
    pub name: Option<String>,
    pub channels: u32,
    pub is_default: bool,
    /// Se puede enviar por AirPlay sin recodificar (AAC, AC-3, E-AC-3).
    pub supported: bool,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct SubtitleTrackInfo {
    pub id: u64,
    pub codec: String,
    pub language: Option<String>,
    pub name: Option<String>,
    pub forced: bool,
    /// Subtítulos de texto (SRT, ASS, WebVTT). Los de imagen (PGS, VobSub) aún no.
    pub supported: bool,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct MediaInfo {
    pub duration_seconds: f64,
    pub video_codec: String,
    pub width: u32,
    pub height: u32,
    pub video_supported: bool,
    pub audio_tracks: Vec<AudioTrackInfo>,
    pub subtitle_tracks: Vec<SubtitleTrackInfo>,
}

/// Lee la cabecera de un MKV (rápido: no recorre el archivo).
#[uniffi::export]
pub fn probe(path: String) -> Result<MediaInfo, CoreError> {
    let (h, _) = mkv::read_header(Path::new(&path))?;
    let video = h.tracks.iter().find(|t| t.kind == mkv::TRACK_VIDEO);
    let duration_seconds = h.duration_ticks.map(|d| d * h.timestamp_scale as f64 / 1e9).unwrap_or(0.0);
    Ok(MediaInfo {
        duration_seconds,
        video_codec: video.map(|v| audio::friendly_codec(&v.codec_id)).unwrap_or_else(|| "ninguno".into()),
        width: video.map(|v| v.pixel_width).unwrap_or(0),
        height: video.map(|v| v.pixel_height).unwrap_or(0),
        video_supported: video.map(media::video_supported).unwrap_or(false),
        audio_tracks: h
            .tracks
            .iter()
            .filter(|t| t.kind == mkv::TRACK_AUDIO)
            .map(|t| AudioTrackInfo {
                id: t.number,
                codec: audio::friendly_codec(&t.codec_id),
                language: t.language.clone().filter(|l| !l.is_empty() && l != "und"),
                name: t.name.clone().filter(|n| !n.is_empty()),
                channels: t.channels,
                is_default: t.is_default,
                supported: media::audio_supported(t),
            })
            .collect(),
        subtitle_tracks: h
            .tracks
            .iter()
            .filter(|t| t.kind == mkv::TRACK_SUBTITLE)
            .map(|t| SubtitleTrackInfo {
                id: t.number,
                codec: subs::friendly_codec(&t.codec_id),
                language: t.language.clone().filter(|l| !l.is_empty() && l != "und"),
                name: t.name.clone().filter(|n| !n.is_empty()),
                forced: t.forced,
                supported: media::subtitle_supported(t),
            })
            .collect(),
    })
}

/// Fragmento MP4 corto alrededor de `at_seconds` (sin audio elegido explícitamente),
/// para que AVAssetImageGenerator saque una miniatura de un MKV.
#[uniffi::export]
pub fn thumbnail_clip(path: String, at_seconds: f64) -> Result<Vec<u8>, CoreError> {
    media::Media::open(Path::new(&path), None, false)?.clip_at(at_seconds)
}

/// Recibe las líneas de diagnóstico del núcleo (peticiones HTTP, errores).
/// En Swift se implementa con `Logger` para verlas en Consola / `log show`.
#[uniffi::export(with_foreign)]
pub trait LogSink: Send + Sync {
    fn log(&self, line: String);
}

static LOG_SINK: RwLock<Option<Arc<dyn LogSink>>> = RwLock::new(None);

/// Instala el receptor de registros (uno por proceso).
#[uniffi::export]
pub fn set_log_sink(sink: Arc<dyn LogSink>) {
    *LOG_SINK.write().unwrap() = Some(sink);
}

pub(crate) fn emit(line: String) {
    match LOG_SINK.read().unwrap().as_ref() {
        Some(sink) => sink.log(line),
        None => eprintln!("[reproductor] {line}"),
    }
}

/// Audio convertido a AAC por la plataforma.
#[derive(Debug, Clone, uniffi::Record)]
pub struct TranscodedAudio {
    /// Índice global del primer paquete en la rejilla de 1024 muestras:
    /// el paquete i cubre las muestras [(first_frame_index + i)·1024, …+1024).
    pub first_frame_index: i64,
    /// Paquetes AAC-LC estéreo (access units sin cabecera ADTS).
    pub packets: Vec<Vec<u8>>,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum TranscodeError {
    #[error("{detail}")]
    Failed { detail: String },
}

impl From<uniffi::UnexpectedUniFFICallbackError> for TranscodeError {
    fn from(e: uniffi::UnexpectedUniFFICallbackError) -> Self {
        TranscodeError::Failed { detail: e.reason }
    }
}

/// Conversor de audio Dolby Digital → AAC que aporta la plataforma (AudioToolbox en Apple).
/// Se usa en el modo compatible para teles de otras marcas, que no decodifican AC-3.
#[uniffi::export(with_foreign)]
pub trait AudioTranscoder: Send + Sync {
    /// `frames`: frames AC-3/E-AC-3 consecutivos (`codec` = "ac-3" o "ec-3").
    /// `first_sample`: posición, en muestras de la línea de tiempo global, del primer frame.
    /// Debe devolver AAC alineado a la rejilla global de 1024 muestras.
    fn to_aac(
        &self,
        codec: String,
        sample_rate: u32,
        channels: u32,
        first_sample: i64,
        frames: Vec<Vec<u8>>,
    ) -> Result<TranscodedAudio, TranscodeError>;
}

static TRANSCODER: RwLock<Option<Arc<dyn AudioTranscoder>>> = RwLock::new(None);

#[uniffi::export]
pub fn set_audio_transcoder(transcoder: Arc<dyn AudioTranscoder>) {
    *TRANSCODER.write().unwrap() = Some(transcoder);
}

pub(crate) fn transcoder() -> Option<Arc<dyn AudioTranscoder>> {
    TRANSCODER.read().unwrap().clone()
}

/// Servidor HLS en la red local. Crear uno al iniciar la app y reutilizarlo.
#[derive(uniffi::Object)]
pub struct StreamServer {
    inner: server::Server,
}

#[uniffi::export]
impl StreamServer {
    #[uniffi::constructor]
    pub fn new() -> Result<Arc<Self>, CoreError> {
        Ok(Arc::new(Self { inner: server::Server::start()? }))
    }

    /// Prepara el MKV y devuelve la URL HLS para AVPlayer.
    /// `audio_track`: id de `AudioTrackInfo`, o nil para elegir automáticamente.
    /// Puede tardar unos segundos si el archivo no trae índice (Cues).
    /// `compat`: modo para teles AirPlay de otras marcas (sin subtítulos, audio en AAC).
    pub fn publish(&self, path: String, audio_track: Option<u64>, compat: bool) -> Result<String, CoreError> {
        self.inner.publish(Path::new(&path), audio_track, compat)
    }

    /// Inicio del keyframe anterior a `seconds` en una URL publicada (para retomar en modo
    /// compatible sin empezar en una pieza sin keyframe).
    pub fn keyframe_start(&self, hls_url: String, seconds: f64) -> f64 {
        self.inner.keyframe_start(&hls_url, seconds)
    }

    /// Olvida los archivos publicados (libera memoria del índice).
    pub fn clear(&self) {
        self.inner.clear();
    }

    pub fn port(&self) -> u16 {
        self.inner.port()
    }
}
