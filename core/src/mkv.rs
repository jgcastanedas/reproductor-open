//! Demuxer mínimo de Matroska (MKV) en Rust puro.
//!
//! Solo lee lo necesario para remuxear: cabecera, pistas, índice (Cues)
//! y bloques de los Clusters. No decodifica nada.

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

// IDs EBML / Matroska
const ID_EBML: u32 = 0x1A45_DFA3;
const ID_SEGMENT: u32 = 0x1853_8067;
const ID_SEEKHEAD: u32 = 0x114D_9B74;
const ID_SEEK: u32 = 0x4DBB;
const ID_SEEK_ID: u32 = 0x53AB;
const ID_SEEK_POSITION: u32 = 0x53AC;
const ID_INFO: u32 = 0x1549_A966;
const ID_TIMESTAMP_SCALE: u32 = 0x2A_D7B1;
const ID_DURATION: u32 = 0x4489;
const ID_TRACKS: u32 = 0x1654_AE6B;
const ID_TRACK_ENTRY: u32 = 0xAE;
const ID_TRACK_NUMBER: u32 = 0xD7;
const ID_TRACK_TYPE: u32 = 0x83;
const ID_CODEC_ID: u32 = 0x86;
const ID_CODEC_PRIVATE: u32 = 0x63A2;
const ID_DEFAULT_DURATION: u32 = 0x23_E383;
const ID_LANGUAGE: u32 = 0x22_B59C;
const ID_LANGUAGE_BCP47: u32 = 0x22_B59D;
const ID_NAME: u32 = 0x536E;
const ID_FLAG_DEFAULT: u32 = 0x88;
const ID_FLAG_ENABLED: u32 = 0xB9;
const ID_FLAG_FORCED: u32 = 0x55AA;
const ID_VIDEO: u32 = 0xE0;
const ID_PIXEL_WIDTH: u32 = 0xB0;
const ID_PIXEL_HEIGHT: u32 = 0xBA;
const ID_DISPLAY_WIDTH: u32 = 0x54B0;
const ID_DISPLAY_HEIGHT: u32 = 0x54BA;
const ID_AUDIO: u32 = 0xE1;
const ID_SAMPLING_FREQUENCY: u32 = 0xB5;
const ID_OUTPUT_SAMPLING_FREQUENCY: u32 = 0x78B5;
const ID_CHANNELS: u32 = 0x9F;
const ID_CONTENT_ENCODINGS: u32 = 0x6D80;
const ID_CONTENT_ENCODING: u32 = 0x6240;
const ID_CONTENT_COMPRESSION: u32 = 0x5034;
const ID_CONTENT_COMP_ALGO: u32 = 0x4254;
const ID_CONTENT_COMP_SETTINGS: u32 = 0x4255;
const ID_CONTENT_ENCRYPTION: u32 = 0x5035;
const ID_CLUSTER: u32 = 0x1F43_B675;
const ID_CLUSTER_TIMESTAMP: u32 = 0xE7;
const ID_SIMPLE_BLOCK: u32 = 0xA3;
const ID_BLOCK_GROUP: u32 = 0xA0;
const ID_BLOCK: u32 = 0xA1;
const ID_REFERENCE_BLOCK: u32 = 0xFB;
const ID_BLOCK_DURATION: u32 = 0x9B;
const ID_CUES: u32 = 0x1C53_BB6B;
const ID_CUE_POINT: u32 = 0xBB;
const ID_CUE_TIME: u32 = 0xB3;
const ID_CUE_TRACK_POSITIONS: u32 = 0xB7;
const ID_CUE_TRACK: u32 = 0xF7;
const ID_CUE_CLUSTER_POSITION: u32 = 0xF1;
const ID_TAGS: u32 = 0x1254_C367;
const ID_CHAPTERS: u32 = 0x1043_A770;
const ID_ATTACHMENTS: u32 = 0x1941_A469;

const LEVEL1_IDS: [u32; 8] = [
    ID_CLUSTER, ID_CUES, ID_TAGS, ID_CHAPTERS, ID_ATTACHMENTS, ID_SEEKHEAD, ID_INFO, ID_TRACKS,
];

pub const TRACK_VIDEO: u64 = 1;
pub const TRACK_AUDIO: u64 = 2;
pub const TRACK_SUBTITLE: u64 = 17;

#[derive(Debug)]
pub enum MkvError {
    Io(io::Error),
    Invalid(String),
}

impl From<io::Error> for MkvError {
    fn from(e: io::Error) -> Self {
        MkvError::Io(e)
    }
}

impl std::fmt::Display for MkvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MkvError::Io(e) => write!(f, "error de lectura: {e}"),
            MkvError::Invalid(m) => write!(f, "MKV inválido: {m}"),
        }
    }
}

pub type Result<T> = std::result::Result<T, MkvError>;

fn invalid<T>(msg: impl Into<String>) -> Result<T> {
    Err(MkvError::Invalid(msg.into()))
}

#[derive(Debug, Clone, Default)]
pub struct Track {
    pub number: u64,
    pub kind: u64,
    pub codec_id: String,
    pub codec_private: Vec<u8>,
    pub default_duration_ns: Option<u64>,
    pub language: Option<String>,
    pub name: Option<String>,
    pub is_default: bool,
    pub forced: bool,
    pub enabled: bool,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub display_width: Option<u32>,
    pub display_height: Option<u32>,
    pub sample_rate: f64,
    pub output_sample_rate: Option<f64>,
    pub channels: u32,
    /// Bytes eliminados por "header stripping" (compresión algoritmo 3).
    pub header_strip: Option<Vec<u8>>,
    /// Compresión zlib (algoritmo 0), habitual en subtítulos de mkvmerge antiguos.
    pub zlib: bool,
    /// Compresión zlib/cifrado: no lo soportamos.
    pub unsupported_encoding: bool,
}

#[derive(Debug, Clone)]
pub struct MkvHeader {
    pub segment_data_start: u64,
    pub segment_end: u64,
    pub timestamp_scale: u64,
    pub duration_ticks: Option<f64>,
    pub tracks: Vec<Track>,
    pub cues_pos: Option<u64>,
    pub first_cluster_pos: Option<u64>,
}

impl MkvHeader {
    pub fn track(&self, number: u64) -> Option<&Track> {
        self.tracks.iter().find(|t| t.number == number)
    }
}

/// Lector con posición propia para evitar llamadas al sistema en cada byte.
pub struct EbmlReader {
    r: BufReader<File>,
    pos: u64,
    len: u64,
}

impl EbmlReader {
    pub fn open(path: &Path) -> Result<Self> {
        let f = File::open(path)?;
        let len = f.metadata()?.len();
        Ok(Self { r: BufReader::with_capacity(256 * 1024, f), pos: 0, len })
    }

    pub fn pos(&self) -> u64 {
        self.pos
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn seek(&mut self, pos: u64) -> Result<()> {
        if pos == self.pos {
            return Ok(());
        }
        let delta = pos as i64 - self.pos as i64;
        if delta.abs() < 128 * 1024 {
            self.r.seek_relative(delta)?;
        } else {
            self.r.seek(SeekFrom::Start(pos))?;
        }
        self.pos = pos;
        Ok(())
    }

    fn skip(&mut self, n: u64) -> Result<()> {
        self.seek(self.pos + n)
    }

    fn u8(&mut self) -> Result<u8> {
        let mut b = [0u8; 1];
        self.r.read_exact(&mut b)?;
        self.pos += 1;
        Ok(b[0])
    }

    fn bytes(&mut self, n: usize) -> Result<Vec<u8>> {
        let mut v = vec![0u8; n];
        self.r.read_exact(&mut v)?;
        self.pos += n as u64;
        Ok(v)
    }

    /// ID EBML (conserva los bits marcadores). Devuelve None en fin de archivo.
    fn read_id(&mut self) -> Result<Option<u32>> {
        if self.pos >= self.len {
            return Ok(None);
        }
        let first = self.u8()?;
        let n = first.leading_zeros() as usize + 1;
        if n > 4 {
            return invalid(format!("ID EBML inválido en {}", self.pos - 1));
        }
        let mut id = first as u32;
        for _ in 1..n {
            id = (id << 8) | self.u8()? as u32;
        }
        Ok(Some(id))
    }

    /// Tamaño EBML. None = tamaño desconocido.
    fn read_size(&mut self) -> Result<Option<u64>> {
        let (v, n, all_ones) = self.read_vint()?;
        let _ = n;
        Ok(if all_ones { None } else { Some(v) })
    }

    fn read_vint(&mut self) -> Result<(u64, usize, bool)> {
        let first = self.u8()?;
        let n = first.leading_zeros() as usize + 1;
        if n > 8 {
            return invalid(format!("entero EBML inválido en {}", self.pos - 1));
        }
        let mask = if n == 8 { 0 } else { 0xFFu8 >> n };
        let mut v = (first & mask) as u64;
        for _ in 1..n {
            v = (v << 8) | self.u8()? as u64;
        }
        let all_ones = v == (1u64 << (7 * n)) - 1;
        Ok((v, n, all_ones))
    }

    fn header(&mut self) -> Result<Option<(u32, Option<u64>, u64)>> {
        let Some(id) = self.read_id()? else { return Ok(None) };
        let size = self.read_size()?;
        Ok(Some((id, size, self.pos)))
    }

    fn uint(&mut self, size: u64) -> Result<u64> {
        if size > 8 {
            return invalid("entero demasiado largo");
        }
        let mut v = 0u64;
        for _ in 0..size {
            v = (v << 8) | self.u8()? as u64;
        }
        Ok(v)
    }

    fn float(&mut self, size: u64) -> Result<f64> {
        match size {
            0 => Ok(0.0),
            4 => Ok(f32::from_bits(self.uint(4)? as u32) as f64),
            8 => Ok(f64::from_bits(self.uint(8)?)),
            _ => invalid("float con tamaño inválido"),
        }
    }

    fn string(&mut self, size: u64) -> Result<String> {
        let b = self.bytes(size as usize)?;
        let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
        Ok(String::from_utf8_lossy(&b[..end]).into_owned())
    }
}

fn need_size(size: Option<u64>, what: &str) -> Result<u64> {
    size.ok_or_else(|| MkvError::Invalid(format!("{what} con tamaño desconocido")))
}

/// Lee la cabecera del archivo: Info, Tracks y la posición de Cues / primer Cluster.
pub fn read_header(path: &Path) -> Result<(MkvHeader, EbmlReader)> {
    let mut r = EbmlReader::open(path)?;
    let Some((id, size, _)) = r.header()? else { return invalid("archivo vacío") };
    if id != ID_EBML {
        return invalid("no es un archivo Matroska/WebM");
    }
    r.skip(need_size(size, "cabecera EBML")?)?;

    // Buscar el Segment (puede haber Void antes).
    let (seg_start, seg_end) = loop {
        let Some((id, size, start)) = r.header()? else { return invalid("sin Segment") };
        if id == ID_SEGMENT {
            let end = size.map(|s| start + s).unwrap_or(r.len()).min(r.len());
            break (start, end);
        }
        r.skip(need_size(size, "elemento")?)?;
    };

    let mut h = MkvHeader {
        segment_data_start: seg_start,
        segment_end: seg_end,
        timestamp_scale: 1_000_000,
        duration_ticks: None,
        tracks: vec![],
        cues_pos: None,
        first_cluster_pos: None,
    };
    let mut info_seen = false;
    let mut tracks_seen = false;
    let mut seek_info: Option<u64> = None;
    let mut seek_tracks: Option<u64> = None;

    while r.pos() < seg_end {
        let el_pos = r.pos();
        let Some((id, size, start)) = r.header()? else { break };
        match id {
            ID_SEEKHEAD => {
                let end = start + need_size(size, "SeekHead")?;
                for (sid, spos) in parse_seekhead(&mut r, end)? {
                    let abs = seg_start + spos;
                    match sid {
                        ID_CUES => h.cues_pos = Some(abs),
                        ID_INFO => seek_info = Some(abs),
                        ID_TRACKS => seek_tracks = Some(abs),
                        _ => {}
                    }
                }
                r.seek(end)?;
            }
            ID_INFO => {
                parse_info(&mut r, start + need_size(size, "Info")?, &mut h)?;
                info_seen = true;
            }
            ID_TRACKS => {
                h.tracks = parse_tracks(&mut r, start + need_size(size, "Tracks")?)?;
                tracks_seen = true;
            }
            ID_CUES => {
                h.cues_pos = Some(el_pos);
                r.skip(need_size(size, "Cues")?)?;
            }
            ID_CLUSTER => {
                h.first_cluster_pos = Some(el_pos);
                break;
            }
            _ => r.skip(need_size(size, "elemento de nivel 1")?)?,
        }
    }

    // Info / Tracks después de los clusters (raro, pero posible).
    if !info_seen {
        if let Some(p) = seek_info {
            r.seek(p)?;
            if let Some((ID_INFO, Some(s), start)) = r.header()? {
                parse_info(&mut r, start + s, &mut h)?;
            }
        }
    }
    if !tracks_seen {
        let Some(p) = seek_tracks else { return invalid("sin pistas (Tracks)") };
        r.seek(p)?;
        match r.header()? {
            Some((ID_TRACKS, Some(s), start)) => h.tracks = parse_tracks(&mut r, start + s)?,
            _ => return invalid("Tracks no encontrado"),
        }
    }
    if h.first_cluster_pos.is_none() {
        return invalid("el archivo no tiene datos de video (Clusters)");
    }
    Ok((h, r))
}

fn parse_seekhead(r: &mut EbmlReader, end: u64) -> Result<Vec<(u32, u64)>> {
    let mut out = vec![];
    while r.pos() < end {
        let Some((id, size, start)) = r.header()? else { break };
        let size = need_size(size, "Seek")?;
        if id == ID_SEEK {
            let (mut sid, mut spos) = (0u32, None);
            while r.pos() < start + size {
                let Some((cid, csize, _)) = r.header()? else { break };
                let csize = need_size(csize, "Seek hijo")?;
                match cid {
                    ID_SEEK_ID => sid = r.uint(csize)? as u32,
                    ID_SEEK_POSITION => spos = Some(r.uint(csize)?),
                    _ => r.skip(csize)?,
                }
            }
            if let Some(p) = spos {
                out.push((sid, p));
            }
        }
        r.seek(start + size)?;
    }
    Ok(out)
}

fn parse_info(r: &mut EbmlReader, end: u64, h: &mut MkvHeader) -> Result<()> {
    while r.pos() < end {
        let Some((id, size, start)) = r.header()? else { break };
        let size = need_size(size, "Info hijo")?;
        match id {
            ID_TIMESTAMP_SCALE => h.timestamp_scale = r.uint(size)?.max(1),
            ID_DURATION => h.duration_ticks = Some(r.float(size)?),
            _ => {}
        }
        r.seek(start + size)?;
    }
    Ok(())
}

fn parse_tracks(r: &mut EbmlReader, end: u64) -> Result<Vec<Track>> {
    let mut tracks = vec![];
    while r.pos() < end {
        let Some((id, size, start)) = r.header()? else { break };
        let size = need_size(size, "TrackEntry")?;
        if id == ID_TRACK_ENTRY {
            tracks.push(parse_track_entry(r, start + size)?);
        }
        r.seek(start + size)?;
    }
    Ok(tracks)
}

fn parse_track_entry(r: &mut EbmlReader, end: u64) -> Result<Track> {
    let mut t = Track { is_default: true, enabled: true, channels: 1, sample_rate: 8000.0, ..Default::default() };
    let mut bcp47: Option<String> = None;
    while r.pos() < end {
        let Some((id, size, start)) = r.header()? else { break };
        let size = need_size(size, "TrackEntry hijo")?;
        match id {
            ID_TRACK_NUMBER => t.number = r.uint(size)?,
            ID_TRACK_TYPE => t.kind = r.uint(size)?,
            ID_CODEC_ID => t.codec_id = r.string(size)?,
            ID_CODEC_PRIVATE => t.codec_private = r.bytes(size as usize)?,
            ID_DEFAULT_DURATION => t.default_duration_ns = Some(r.uint(size)?),
            ID_LANGUAGE => t.language = Some(r.string(size)?),
            ID_LANGUAGE_BCP47 => bcp47 = Some(r.string(size)?),
            ID_NAME => t.name = Some(r.string(size)?),
            ID_FLAG_DEFAULT => t.is_default = r.uint(size)? != 0,
            ID_FLAG_ENABLED => t.enabled = r.uint(size)? != 0,
            ID_FLAG_FORCED => t.forced = r.uint(size)? != 0,
            ID_VIDEO => {
                let vend = start + size;
                while r.pos() < vend {
                    let Some((vid, vsize, vstart)) = r.header()? else { break };
                    let vsize = need_size(vsize, "Video hijo")?;
                    match vid {
                        ID_PIXEL_WIDTH => t.pixel_width = r.uint(vsize)? as u32,
                        ID_PIXEL_HEIGHT => t.pixel_height = r.uint(vsize)? as u32,
                        ID_DISPLAY_WIDTH => t.display_width = Some(r.uint(vsize)? as u32),
                        ID_DISPLAY_HEIGHT => t.display_height = Some(r.uint(vsize)? as u32),
                        _ => {}
                    }
                    r.seek(vstart + vsize)?;
                }
            }
            ID_AUDIO => {
                let aend = start + size;
                while r.pos() < aend {
                    let Some((aid, asize, astart)) = r.header()? else { break };
                    let asize = need_size(asize, "Audio hijo")?;
                    match aid {
                        ID_SAMPLING_FREQUENCY => t.sample_rate = r.float(asize)?,
                        ID_OUTPUT_SAMPLING_FREQUENCY => t.output_sample_rate = Some(r.float(asize)?),
                        ID_CHANNELS => t.channels = r.uint(asize)? as u32,
                        _ => {}
                    }
                    r.seek(astart + asize)?;
                }
            }
            ID_CONTENT_ENCODINGS => parse_content_encodings(r, start + size, &mut t)?,
            _ => {}
        }
        r.seek(start + size)?;
    }
    if bcp47.is_some() {
        t.language = bcp47;
    }
    Ok(t)
}

fn parse_content_encodings(r: &mut EbmlReader, end: u64, t: &mut Track) -> Result<()> {
    while r.pos() < end {
        let Some((id, size, start)) = r.header()? else { break };
        let size = need_size(size, "ContentEncoding")?;
        if id == ID_CONTENT_ENCODING {
            while r.pos() < start + size {
                let Some((cid, csize, cstart)) = r.header()? else { break };
                let csize = need_size(csize, "ContentEncoding hijo")?;
                match cid {
                    ID_CONTENT_COMPRESSION => {
                        let (mut algo, mut settings) = (0u64, vec![]);
                        while r.pos() < cstart + csize {
                            let Some((zid, zsize, zstart)) = r.header()? else { break };
                            let zsize = need_size(zsize, "ContentCompression hijo")?;
                            match zid {
                                ID_CONTENT_COMP_ALGO => algo = r.uint(zsize)?,
                                ID_CONTENT_COMP_SETTINGS => settings = r.bytes(zsize as usize)?,
                                _ => {}
                            }
                            r.seek(zstart + zsize)?;
                        }
                        match algo {
                            0 => t.zlib = true,
                            3 => t.header_strip = Some(settings),
                            _ => t.unsupported_encoding = true,
                        }
                    }
                    ID_CONTENT_ENCRYPTION => t.unsupported_encoding = true,
                    _ => {}
                }
                r.seek(cstart + csize)?;
            }
        }
        r.seek(start + size)?;
    }
    Ok(())
}

/// Puntos de acceso (keyframes) del índice Cues para una pista.
pub fn read_cues(r: &mut EbmlReader, h: &MkvHeader, track: u64) -> Result<Vec<Keyframe>> {
    let Some(pos) = h.cues_pos else { return Ok(vec![]) };
    r.seek(pos)?;
    let Some((ID_CUES, Some(size), start)) = r.header()? else { return Ok(vec![]) };
    let end = start + size;
    let mut out = vec![];
    while r.pos() < end {
        let Some((id, size, cstart)) = r.header()? else { break };
        let size = need_size(size, "CuePoint")?;
        if id == ID_CUE_POINT {
            let mut time = None;
            let mut cluster = None;
            while r.pos() < cstart + size {
                let Some((pid, psize, pstart)) = r.header()? else { break };
                let psize = need_size(psize, "CuePoint hijo")?;
                match pid {
                    ID_CUE_TIME => time = Some(r.uint(psize)? as i64),
                    ID_CUE_TRACK_POSITIONS => {
                        let (mut trk, mut cpos) = (0u64, None);
                        while r.pos() < pstart + psize {
                            let Some((qid, qsize, qstart)) = r.header()? else { break };
                            let qsize = need_size(qsize, "CueTrackPositions hijo")?;
                            match qid {
                                ID_CUE_TRACK => trk = r.uint(qsize)?,
                                ID_CUE_CLUSTER_POSITION => cpos = Some(r.uint(qsize)?),
                                _ => {}
                            }
                            r.seek(qstart + qsize)?;
                        }
                        if trk == track && cluster.is_none() {
                            cluster = cpos;
                        }
                    }
                    _ => {}
                }
                r.seek(pstart + psize)?;
            }
            if let (Some(t), Some(c)) = (time, cluster) {
                out.push(Keyframe { ts: t, cluster_pos: h.segment_data_start + c });
            }
        }
        r.seek(cstart + size)?;
    }
    out.sort_by_key(|k| k.ts);
    out.dedup_by_key(|k| k.ts);
    Ok(out)
}

#[derive(Debug, Clone, Copy)]
pub struct Keyframe {
    /// Marca de tiempo en "ticks" (unidades de TimestampScale).
    pub ts: i64,
    /// Posición absoluta del Cluster que contiene el keyframe.
    pub cluster_pos: u64,
}

pub struct Block {
    pub track: u64,
    pub ts: i64,
    pub keyframe: bool,
    /// Frames (con lacing ya separado). Vacío si no se pidieron datos.
    pub frames: Vec<Vec<u8>>,
    /// BlockDuration (ticks), solo en BlockGroup. Lo usan los subtítulos.
    pub duration: Option<i64>,
}

pub enum Event<'a> {
    Cluster { pos: u64, ts: i64 },
    Block(&'a Block),
}

#[derive(PartialEq, Eq)]
pub enum Control {
    Continue,
    Stop,
}

/// Recorre Clusters a partir de `start`, llamando a `cb` con cada cluster y bloque.
/// `want_data(track)` indica si hay que leer los datos de esa pista.
pub fn walk_clusters(
    r: &mut EbmlReader,
    h: &MkvHeader,
    start: u64,
    want_data: &dyn Fn(u64) -> bool,
    cb: &mut dyn FnMut(Event) -> Control,
) -> Result<()> {
    r.seek(start)?;
    let end = h.segment_end;
    while r.pos() < end {
        let el_pos = r.pos();
        let Some((id, size, data_start)) = r.header()? else { break };
        if id != ID_CLUSTER {
            match size {
                Some(s) => {
                    r.seek(data_start + s)?;
                    continue;
                }
                None => break,
            }
        }
        let cluster_end = size.map(|s| data_start + s).unwrap_or(end).min(end);
        let mut cluster_ts: i64 = 0;
        let mut announced = false;
        while r.pos() < cluster_end {
            let child_pos = r.pos();
            let Some((cid, csize, cstart)) = r.header()? else { break };
            if size.is_none() && LEVEL1_IDS.contains(&cid) {
                // Cluster de tamaño desconocido: termina al encontrar otro elemento de nivel 1.
                r.seek(child_pos)?;
                break;
            }
            let csize = need_size(csize, "hijo de Cluster")?;
            match cid {
                ID_CLUSTER_TIMESTAMP => {
                    cluster_ts = r.uint(csize)? as i64;
                    if !announced {
                        announced = true;
                        if cb(Event::Cluster { pos: el_pos, ts: cluster_ts }) == Control::Stop {
                            return Ok(());
                        }
                    }
                }
                ID_SIMPLE_BLOCK => {
                    let b = read_block(r, h, csize, cluster_ts, want_data, true)?;
                    if let Some(b) = b {
                        if cb(Event::Block(&b)) == Control::Stop {
                            return Ok(());
                        }
                    }
                }
                ID_BLOCK_GROUP => {
                    let gend = cstart + csize;
                    let mut block = None;
                    let mut has_ref = false;
                    let mut duration = None;
                    while r.pos() < gend {
                        let Some((gid, gsize, gstart)) = r.header()? else { break };
                        let gsize = need_size(gsize, "hijo de BlockGroup")?;
                        match gid {
                            ID_BLOCK => block = read_block(r, h, gsize, cluster_ts, want_data, false)?,
                            ID_REFERENCE_BLOCK => has_ref = true,
                            ID_BLOCK_DURATION => duration = Some(r.uint(gsize)? as i64),
                            _ => {}
                        }
                        r.seek(gstart + gsize)?;
                    }
                    if let Some(mut b) = block {
                        b.keyframe = !has_ref;
                        b.duration = duration;
                        if cb(Event::Block(&b)) == Control::Stop {
                            return Ok(());
                        }
                    }
                }
                _ => {}
            }
            r.seek(cstart + csize)?;
        }
        if size.is_some() {
            r.seek(cluster_end)?;
        }
    }
    Ok(())
}

fn read_block(
    r: &mut EbmlReader,
    h: &MkvHeader,
    size: u64,
    cluster_ts: i64,
    want_data: &dyn Fn(u64) -> bool,
    simple: bool,
) -> Result<Option<Block>> {
    let start = r.pos();
    let end = start + size;
    let (track, _, _) = r.read_vint()?;
    let rel = ((r.u8()? as u16) << 8 | r.u8()? as u16) as i16;
    let flags = r.u8()?;
    let ts = cluster_ts + rel as i64;
    let keyframe = simple && flags & 0x80 != 0;
    let lacing = (flags >> 1) & 0x03;

    if !want_data(track) {
        r.seek(end)?;
        return Ok(Some(Block { track, ts, keyframe, frames: vec![], duration: None }));
    }

    let mut sizes: Vec<u64> = vec![];
    if lacing == 0 {
        sizes.push(end - r.pos());
    } else {
        let n = r.u8()? as usize + 1;
        match lacing {
            1 => {
                // Xiph
                for _ in 0..n - 1 {
                    let mut s = 0u64;
                    loop {
                        let b = r.u8()?;
                        s += b as u64;
                        if b != 255 {
                            break;
                        }
                    }
                    sizes.push(s);
                }
            }
            3 => {
                // EBML
                let (first, _, _) = r.read_vint()?;
                sizes.push(first);
                let mut prev = first as i64;
                for _ in 1..n - 1 {
                    let (raw, len, _) = r.read_vint()?;
                    let bias = (1i64 << (7 * len - 1)) - 1;
                    prev += raw as i64 - bias;
                    if prev < 0 {
                        return invalid("lacing EBML inválido");
                    }
                    sizes.push(prev as u64);
                }
            }
            2 => {
                // Tamaño fijo
                let total = end - r.pos();
                for _ in 0..n - 1 {
                    sizes.push(total / n as u64);
                }
            }
            _ => unreachable!(),
        }
        let used: u64 = sizes.iter().sum();
        let remaining = (end - r.pos()).checked_sub(used);
        match remaining {
            Some(rem) => sizes.push(rem),
            None => return invalid("tamaños de lacing inconsistentes"),
        }
    }

    let strip = h.track(track).and_then(|t| t.header_strip.clone());
    let zlib = h.track(track).is_some_and(|t| t.zlib);
    let mut frames = Vec::with_capacity(sizes.len());
    for s in &sizes {
        let mut data = r.bytes(*s as usize)?;
        if zlib {
            data = miniz_oxide::inflate::decompress_to_vec_zlib(&data)
                .map_err(|e| MkvError::Invalid(format!("datos zlib inválidos: {e:?}")))?;
        }
        frames.push(match &strip {
            Some(prefix) if !prefix.is_empty() => {
                let mut v = Vec::with_capacity(prefix.len() + data.len());
                v.extend_from_slice(prefix);
                v.extend_from_slice(&data);
                v
            }
            _ => data,
        });
    }
    r.seek(end)?;
    Ok(Some(Block { track, ts, keyframe, frames, duration: None }))
}

/// Sin Cues: recorre todo el archivo leyendo solo cabeceras de bloque.
pub fn scan_keyframes(r: &mut EbmlReader, h: &MkvHeader, track: u64) -> Result<Vec<Keyframe>> {
    let mut out = vec![];
    let mut cluster_pos = 0u64;
    let start = h.first_cluster_pos.unwrap_or(h.segment_data_start);
    walk_clusters(r, h, start, &|_| false, &mut |ev| {
        match ev {
            Event::Cluster { pos, .. } => cluster_pos = pos,
            Event::Block(b) => {
                if b.track == track && b.keyframe {
                    out.push(Keyframe { ts: b.ts, cluster_pos });
                }
            }
        }
        Control::Continue
    })?;
    out.sort_by_key(|k| k.ts);
    out.dedup_by_key(|k| k.ts);
    Ok(out)
}
