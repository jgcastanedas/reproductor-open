//! Prueba del núcleo desde la terminal del Mac:
//!   cargo run --release --bin reproductor-cli -- pelicula.mkv [id_pista_audio]
//! Abre la URL que imprime en Safari o QuickTime (Archivo > Abrir ubicación) y envíala por AirPlay.

use reproductor_core::{probe, StreamServer};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("uso: reproductor-cli <archivo.mkv> [id_pista_audio]");
        std::process::exit(2);
    };
    let audio: Option<u64> = args.next().and_then(|a| a.parse().ok());

    match probe(path.clone()) {
        Ok(info) => {
            println!("Video: {} {}x{} (compatible: {})", info.video_codec, info.width, info.height, info.video_supported);
            println!("Duración: {:.1} s · subtítulos: {}", info.duration_seconds, info.subtitle_count);
            for a in &info.audio_tracks {
                println!(
                    "  audio #{}: {} {}ch {} {}{}",
                    a.id,
                    a.codec,
                    a.channels,
                    a.language.clone().unwrap_or_default(),
                    a.name.clone().unwrap_or_default(),
                    if a.supported { "" } else { "  (no compatible)" }
                );
            }
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }

    let server = StreamServer::new().expect("no se pudo iniciar el servidor");
    let t = std::time::Instant::now();
    match server.publish(path, audio) {
        Ok(url) => {
            println!("Listo en {:.2?}\n{url}\nCtrl+C para salir.", t.elapsed());
            loop {
                std::thread::park();
            }
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
