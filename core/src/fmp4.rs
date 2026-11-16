//! Escritor de MP4 fragmentado (CMAF/fMP4) para segmentos HLS.

pub const VIDEO_TRACK_ID: u32 = 1;
pub const AUDIO_TRACK_ID: u32 = 2;
pub const VIDEO_TIMESCALE: u32 = 90_000;

#[derive(Clone, Debug)]
pub enum VideoCodec {
    /// avcC (AVCDecoderConfigurationRecord)
    Avc(Vec<u8>),
    /// hvcC (HEVCDecoderConfigurationRecord)
    Hevc(Vec<u8>),
}

#[derive(Clone, Debug)]
pub struct VideoParams {
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub display_width: u32,
    pub display_height: u32,
}

#[derive(Clone, Debug)]
pub enum AudioCodec {
    /// AudioSpecificConfig
    Aac(Vec<u8>),
    /// Contenido de la caja dac3 (3 bytes)
    Ac3([u8; 3]),
    /// Contenido de la caja dec3
    Eac3(Vec<u8>),
}

#[derive(Clone, Debug)]
pub struct AudioParams {
    pub codec: AudioCodec,
    pub sample_rate: u32,
    pub channels: u16,
}

pub struct Sample {
    pub duration: u32,
    pub size: u32,
    pub keyframe: bool,
    pub cto: i32,
}

pub struct TrackFragment {
    pub track_id: u32,
    pub base_decode_time: u64,
    pub samples: Vec<Sample>,
    pub data: Vec<u8>,
    pub is_video: bool,
}

struct W(Vec<u8>);

impl W {
    fn new() -> Self {
        W(Vec::with_capacity(1024))
    }
    fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    fn u16(&mut self, v: u16) -> &mut Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    fn i32(&mut self, v: i32) -> &mut Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.0.extend_from_slice(b);
        self
    }
    fn zeros(&mut self, n: usize) -> &mut Self {
        self.0.extend(std::iter::repeat(0u8).take(n));
        self
    }
    fn open(&mut self, typ: &[u8; 4]) -> usize {
        let at = self.0.len();
        self.u32(0).bytes(typ);
        at
    }
    fn open_full(&mut self, typ: &[u8; 4], version: u8, flags: u32) -> usize {
        let at = self.open(typ);
        self.u32(((version as u32) << 24) | (flags & 0x00FF_FFFF));
        at
    }
    fn close(&mut self, at: usize) {
        let size = (self.0.len() - at) as u32;
        self.0[at..at + 4].copy_from_slice(&size.to_be_bytes());
    }
}

const MATRIX: [u32; 9] = [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000];

/// Segmento de inicialización (ftyp + moov).
pub fn init_segment(video: &VideoParams, audio: Option<&AudioParams>) -> Vec<u8> {
    let mut w = W::new();
    let ftyp = w.open(b"ftyp");
    w.bytes(b"iso6").u32(0).bytes(b"iso6").bytes(b"iso5").bytes(b"cmfc").bytes(b"mp41");
    w.close(ftyp);

    let moov = w.open(b"moov");
    let mvhd = w.open_full(b"mvhd", 0, 0);
    w.u32(0).u32(0).u32(1000).u32(0).u32(0x0001_0000).u16(0x0100).zeros(10);
    for m in MATRIX {
        w.u32(m);
    }
    w.zeros(24).u32(if audio.is_some() { 3 } else { 2 });
    w.close(mvhd);

    write_trak(&mut w, VIDEO_TRACK_ID, TrackKind::Video(video));
    if let Some(a) = audio {
        write_trak(&mut w, AUDIO_TRACK_ID, TrackKind::Audio(a));
    }

    let mvex = w.open(b"mvex");
    let mut ids = vec![VIDEO_TRACK_ID];
    if audio.is_some() {
        ids.push(AUDIO_TRACK_ID);
    }
    for id in ids {
        let trex = w.open_full(b"trex", 0, 0);
        w.u32(id).u32(1).u32(0).u32(0).u32(0);
        w.close(trex);
    }
    w.close(mvex);
    w.close(moov);
    w.0
}

enum TrackKind<'a> {
    Video(&'a VideoParams),
    Audio(&'a AudioParams),
}

fn write_trak(w: &mut W, id: u32, kind: TrackKind) {
    let trak = w.open(b"trak");
    let tkhd = w.open_full(b"tkhd", 0, 0x3);
    w.u32(0).u32(0).u32(id).u32(0).u32(0).zeros(8).u16(0).u16(0);
    match &kind {
        TrackKind::Audio(_) => w.u16(0x0100),
        TrackKind::Video(_) => w.u16(0),
    };
    w.u16(0);
    for m in MATRIX {
        w.u32(m);
    }
    match &kind {
        TrackKind::Video(v) => w.u32(v.display_width << 16).u32(v.display_height << 16),
        TrackKind::Audio(_) => w.u32(0).u32(0),
    };
    w.close(tkhd);

    let mdia = w.open(b"mdia");
    let timescale = match &kind {
        TrackKind::Video(_) => VIDEO_TIMESCALE,
        TrackKind::Audio(a) => a.sample_rate,
    };
    let mdhd = w.open_full(b"mdhd", 0, 0);
    w.u32(0).u32(0).u32(timescale).u32(0).u16(0x55C4).u16(0); // "und"
    w.close(mdhd);

    let hdlr = w.open_full(b"hdlr", 0, 0);
    w.u32(0);
    match &kind {
        TrackKind::Video(_) => w.bytes(b"vide").zeros(12).bytes(b"VideoHandler\0"),
        TrackKind::Audio(_) => w.bytes(b"soun").zeros(12).bytes(b"SoundHandler\0"),
    };
    w.close(hdlr);

    let minf = w.open(b"minf");
    match &kind {
        TrackKind::Video(_) => {
            let vmhd = w.open_full(b"vmhd", 0, 1);
            w.u16(0).u16(0).u16(0).u16(0);
            w.close(vmhd);
        }
        TrackKind::Audio(_) => {
            let smhd = w.open_full(b"smhd", 0, 0);
            w.u16(0).u16(0);
            w.close(smhd);
        }
    }
    let dinf = w.open(b"dinf");
    let dref = w.open_full(b"dref", 0, 0);
    w.u32(1);
    let url = w.open_full(b"url ", 0, 1);
    w.close(url);
    w.close(dref);
    w.close(dinf);

    let stbl = w.open(b"stbl");
    let stsd = w.open_full(b"stsd", 0, 0);
    w.u32(1);
    match &kind {
        TrackKind::Video(v) => write_video_entry(w, v),
        TrackKind::Audio(a) => write_audio_entry(w, a),
    }
    w.close(stsd);
    for typ in [b"stts", b"stsc", b"stco"] {
        let b = w.open_full(typ, 0, 0);
        w.u32(0);
        w.close(b);
    }
    let stsz = w.open_full(b"stsz", 0, 0);
    w.u32(0).u32(0);
    w.close(stsz);
    w.close(stbl);
    w.close(minf);
    w.close(mdia);
    w.close(trak);
}

fn write_video_entry(w: &mut W, v: &VideoParams) {
    let (typ, cfg_typ, cfg) = match &v.codec {
        VideoCodec::Avc(c) => (b"avc1", b"avcC", c),
        VideoCodec::Hevc(c) => (b"hvc1", b"hvcC", c),
    };
    let e = w.open(typ);
    w.zeros(6).u16(1).zeros(16);
    w.u16(v.width as u16).u16(v.height as u16);
    w.u32(0x0048_0000).u32(0x0048_0000).u32(0).u16(1);
    w.zeros(32).u16(0x0018).u16(0xFFFF);
    let c = w.open(cfg_typ);
    w.bytes(cfg);
    w.close(c);
    // Relación de aspecto de píxel (para video anamórfico).
    let h_spacing = v.display_width as u64 * v.height as u64;
    let v_spacing = v.display_height as u64 * v.width as u64;
    if h_spacing != v_spacing && h_spacing > 0 && v_spacing > 0 {
        let g = gcd(h_spacing, v_spacing);
        let pasp = w.open(b"pasp");
        w.u32((h_spacing / g) as u32).u32((v_spacing / g) as u32);
        w.close(pasp);
    }
    w.close(e);
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

fn write_audio_entry(w: &mut W, a: &AudioParams) {
    let typ = match a.codec {
        AudioCodec::Aac(_) => b"mp4a",
        AudioCodec::Ac3(_) => b"ac-3",
        AudioCodec::Eac3(_) => b"ec-3",
    };
    let e = w.open(typ);
    w.zeros(6).u16(1).zeros(8);
    w.u16(a.channels.max(1)).u16(16).u16(0).u16(0);
    w.u32(a.sample_rate.min(65535) << 16);
    match &a.codec {
        AudioCodec::Aac(asc) => {
            let esds = w.open_full(b"esds", 0, 0);
            // ES_Descriptor
            let dsi_len = 5 + asc.len();
            let dcd_len = 13 + dsi_len;
            let es_len = 3 + 5 + dcd_len + 5 + 1;
            w.u8(0x03);
            desc_len(w, es_len);
            w.u16(0).u8(0);
            // DecoderConfigDescriptor
            w.u8(0x04);
            desc_len(w, dcd_len);
            w.u8(0x40).u8(0x15).u8(0).u16(0).u32(0).u32(0);
            // DecoderSpecificInfo
            w.u8(0x05);
            desc_len(w, asc.len());
            w.bytes(asc);
            // SLConfigDescriptor
            w.u8(0x06);
            desc_len(w, 1);
            w.u8(0x02);
            w.close(esds);
        }
        AudioCodec::Ac3(d) => {
            let b = w.open(b"dac3");
            w.bytes(d);
            w.close(b);
        }
        AudioCodec::Eac3(d) => {
            let b = w.open(b"dec3");
            w.bytes(d);
            w.close(b);
        }
    }
    w.close(e);
}

fn desc_len(w: &mut W, len: usize) {
    w.u8(0x80 | ((len >> 21) & 0x7F) as u8)
        .u8(0x80 | ((len >> 14) & 0x7F) as u8)
        .u8(0x80 | ((len >> 7) & 0x7F) as u8)
        .u8((len & 0x7F) as u8);
}

/// Un fragmento media (moof + mdat) con una o dos pistas.
pub fn media_segment(sequence: u32, tracks: &[TrackFragment]) -> Vec<u8> {
    let tracks: Vec<&TrackFragment> = tracks.iter().filter(|t| !t.samples.is_empty()).collect();
    let moof_len = build_moof(sequence, &tracks, &vec![0; tracks.len()]).len();
    let mut offsets = vec![];
    let mut off = moof_len as i32 + 8;
    for t in &tracks {
        offsets.push(off);
        off += t.data.len() as i32;
    }
    let mut out = build_moof(sequence, &tracks, &offsets);
    let mdat_len: usize = 8 + tracks.iter().map(|t| t.data.len()).sum::<usize>();
    out.reserve(mdat_len);
    out.extend_from_slice(&(mdat_len as u32).to_be_bytes());
    out.extend_from_slice(b"mdat");
    for t in &tracks {
        out.extend_from_slice(&t.data);
    }
    out
}

fn build_moof(sequence: u32, tracks: &[&TrackFragment], offsets: &[i32]) -> Vec<u8> {
    let mut w = W::new();
    let moof = w.open(b"moof");
    let mfhd = w.open_full(b"mfhd", 0, 0);
    w.u32(sequence);
    w.close(mfhd);
    for (t, off) in tracks.iter().zip(offsets) {
        let traf = w.open(b"traf");
        let tfhd = w.open_full(b"tfhd", 0, 0x02_0000); // default-base-is-moof
        w.u32(t.track_id);
        w.close(tfhd);
        let tfdt = w.open_full(b"tfdt", 1, 0);
        w.u64(t.base_decode_time);
        w.close(tfdt);
        let flags = if t.is_video { 0x000F01 } else { 0x000301 };
        let trun = w.open_full(b"trun", 1, flags);
        w.u32(t.samples.len() as u32).i32(*off);
        for s in &t.samples {
            w.u32(s.duration).u32(s.size);
            if t.is_video {
                w.u32(if s.keyframe { 0x0200_0000 } else { 0x0101_0000 }).i32(s.cto);
            }
        }
        w.close(trun);
        w.close(traf);
    }
    w.close(moof);
    w.0
}
