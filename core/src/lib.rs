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

use std::path::Path;
use std::sync::Arc;

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
pub struct MediaInfo {
    pub duration_seconds: f64,
    pub video_codec: String,
    pub width: u32,
    pub height: u32,
    pub video_supported: bool,
    pub audio_tracks: Vec<AudioTrackInfo>,
    pub subtitle_count: u32,
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
        subtitle_count: h.tracks.iter().filter(|t| t.kind == 17).count() as u32,
    })
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
    pub fn publish(&self, path: String, audio_track: Option<u64>) -> Result<String, CoreError> {
        self.inner.publish(Path::new(&path), audio_track)
    }

    /// Olvida los archivos publicados (libera memoria del índice).
    pub fn clear(&self) {
        self.inner.clear();
    }

    pub fn port(&self) -> u16 {
        self.inner.port()
    }
}
