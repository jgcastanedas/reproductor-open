//! Servidor HTTP en la red local. El Apple TV descarga de aquí la lista HLS
//! y los segmentos, así que debe escuchar en la IP de la LAN (no en 127.0.0.1).

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

use tiny_http::{Header, Request, Response, StatusCode};

use crate::media::Media;
use crate::CoreError;

const WORKERS: usize = 4;

type Sessions = Arc<Mutex<HashMap<String, Arc<Media>>>>;

pub struct Server {
    http: Arc<tiny_http::Server>,
    port: u16,
    sessions: Sessions,
    workers: Mutex<Vec<JoinHandle<()>>>,
}

impl Server {
    pub fn start() -> Result<Self, CoreError> {
        let http = tiny_http::Server::http("0.0.0.0:0").map_err(|e| CoreError::Server { detail: e.to_string() })?;
        let port = http
            .server_addr()
            .to_ip()
            .map(|a| a.port())
            .ok_or_else(|| CoreError::Server { detail: "sin puerto TCP".into() })?;
        let http = Arc::new(http);
        let sessions: Sessions = Arc::default();
        let mut workers = vec![];
        for _ in 0..WORKERS {
            let http = http.clone();
            let sessions = sessions.clone();
            workers.push(std::thread::spawn(move || {
                while let Ok(req) = http.recv() {
                    handle(req, &sessions);
                }
            }));
        }
        Ok(Server { http, port, sessions, workers: Mutex::new(workers) })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Prepara un archivo y devuelve la URL de su lista HLS.
    pub fn publish(&self, path: &Path, audio_track: Option<u64>) -> Result<String, CoreError> {
        let media = Arc::new(Media::open(path, audio_track)?);
        let token = new_token(path);
        self.sessions.lock().unwrap().insert(token.clone(), media);
        Ok(format!("http://{}:{}/{}/index.m3u8", lan_ip(), self.port, token))
    }

    pub fn clear(&self) {
        self.sessions.lock().unwrap().clear();
    }

    pub fn stop(&self) {
        self.http.unblock();
        for h in self.workers.lock().unwrap().drain(..) {
            // Cada trabajador sale de recv() tras un unblock.
            self.http.unblock();
            let _ = h.join();
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn new_token(path: &Path) -> String {
    let mut h = DefaultHasher::new();
    path.hash(&mut h);
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0).hash(&mut h);
    COUNTER.fetch_add(1, Ordering::Relaxed).hash(&mut h);
    std::process::id().hash(&mut h);
    let a = h.finish();
    a.hash(&mut h);
    format!("{:016x}{:016x}", a, h.finish())
}

/// IPv4 de la red local (prioriza Wi-Fi/Ethernet: en0, en1…).
pub fn lan_ip() -> IpAddr {
    let Ok(ifaces) = if_addrs::get_if_addrs() else { return IpAddr::V4(Ipv4Addr::LOCALHOST) };
    let mut candidates: Vec<(u8, IpAddr)> = ifaces
        .into_iter()
        .filter(|i| !i.is_loopback())
        .filter_map(|i| match i.ip() {
            IpAddr::V4(v4) if v4.is_private() || v4.is_link_local() => {
                let rank = if i.name == "en0" {
                    0
                } else if i.name.starts_with("en") {
                    1
                } else if v4.is_private() {
                    2
                } else {
                    3
                };
                Some((rank, IpAddr::V4(v4)))
            }
            _ => None,
        })
        .collect();
    candidates.sort_by_key(|c| c.0);
    candidates.first().map(|c| c.1).unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST))
}

fn header(k: &str, v: &str) -> Header {
    Header::from_bytes(k.as_bytes(), v.as_bytes()).expect("cabecera válida")
}

fn handle(req: Request, sessions: &Sessions) {
    let url = req.url().split('?').next().unwrap_or("").to_string();
    let parts: Vec<&str> = url.trim_start_matches('/').splitn(2, '/').collect();
    let (token, file) = match parts.as_slice() {
        [t, f] => (*t, *f),
        _ => return respond_status(req, 404),
    };
    let Some(media) = sessions.lock().unwrap().get(token).cloned() else {
        return respond_status(req, 404);
    };

    let result: Result<(Vec<u8>, &str), CoreError> = if file == "index.m3u8" {
        Ok((media.playlist().into_bytes(), "application/vnd.apple.mpegurl"))
    } else if file == "init.mp4" {
        Ok((media.init_segment().to_vec(), "video/mp4"))
    } else if let Some(n) = file.strip_prefix("seg").and_then(|s| s.strip_suffix(".m4s")).and_then(|s| s.parse().ok()) {
        media.segment(n).map(|b| (b, "video/iso.segment"))
    } else {
        return respond_status(req, 404);
    };

    match result {
        Ok((body, ctype)) => respond_bytes(req, body, ctype),
        Err(e) => {
            eprintln!("[reproductor] error sirviendo {url}: {e}");
            respond_status(req, 500)
        }
    }
}

fn respond_status(req: Request, code: u16) {
    let _ = req.respond(Response::empty(StatusCode(code)));
}

fn respond_bytes(req: Request, body: Vec<u8>, ctype: &str) {
    let total = body.len();
    let range = req
        .headers()
        .iter()
        .find(|h| h.field.equiv("Range"))
        .and_then(|h| parse_range(h.value.as_str(), total));
    let is_head = req.method() == &tiny_http::Method::Head;
    let common = |r: Response<std::io::Cursor<Vec<u8>>>| {
        r.with_header(header("Content-Type", ctype))
            .with_header(header("Accept-Ranges", "bytes"))
            .with_header(header("Cache-Control", "no-cache"))
            .with_header(header("Access-Control-Allow-Origin", "*"))
    };
    let resp = match range {
        Some((a, b)) => {
            let slice = if is_head { vec![] } else { body[a..=b].to_vec() };
            common(Response::from_data(slice))
                .with_status_code(206)
                .with_header(header("Content-Range", &format!("bytes {a}-{b}/{total}")))
        }
        None => common(Response::from_data(if is_head { vec![] } else { body })),
    };
    let _ = req.respond(resp);
}

fn parse_range(v: &str, total: usize) -> Option<(usize, usize)> {
    let spec = v.trim().strip_prefix("bytes=")?;
    let (a, b) = spec.split_once('-')?;
    if total == 0 {
        return None;
    }
    let (start, end) = if a.is_empty() {
        let n: usize = b.parse().ok()?;
        (total.saturating_sub(n), total - 1)
    } else {
        let s: usize = a.parse().ok()?;
        let e: usize = if b.is_empty() { total - 1 } else { b.parse::<usize>().ok()?.min(total - 1) };
        (s, e)
    };
    (start <= end && start < total).then_some((start, end))
}
