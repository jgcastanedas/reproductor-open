# Reproductor AirPlay

Reproductor de video para **iPhone, iPad y Mac** que envía **MP4 y MKV** al Apple TV por AirPlay, sin recodificar.

## Cómo funciona

```
MKV ──► núcleo Rust (core/) ──► HLS fMP4 por HTTP en la IP de la red local ──► AVPlayer ──► AirPlay ──► Apple TV
MP4/MOV ───────────────────────────────────────────────────────────────────► AVPlayer ──► AirPlay ──► Apple TV
```

- AirPlay solo existe a través de los frameworks de Apple, así que la interfaz y la reproducción usan **SwiftUI + AVPlayer**.
- El **núcleo en Rust** (sin ffmpeg, Rust puro) hace el trabajo pesado:
  - `mkv.rs`: demuxer Matroska (cabecera, pistas, índice Cues, bloques con lacing y header stripping).
  - `fmp4.rs`: muxer MP4 fragmentado (init + segmentos `moof/mdat`).
  - `media.rs`: corta el video en segmentos HLS de unos 6 s, alineados a keyframes, y genera cada segmento **bajo demanda** (≈10 ms).
  - `server.rs`: servidor HTTP en `0.0.0.0` que anuncia la **IP de la LAN**. El Apple TV descarga los segmentos directamente del iPhone o el Mac (con `127.0.0.1` no funcionaría).
- Swift lo usa mediante **UniFFI** (`probe`, `StreamServer.publish`).

### Compatibilidad (sin recodificar)

| | Compatible | No compatible |
|---|---|---|
| Video | H.264, HEVC/H.265 (incl. HDR10) | AV1, VP9, MPEG-2 |
| Audio | AAC, Dolby Digital (AC-3), Dolby Digital Plus (E-AC-3) | DTS, TrueHD, FLAC, Opus |

Si el MKV trae varias pistas de audio, la app deja elegir entre ellas (prioriza español). Las pistas incompatibles aparecen deshabilitadas.

## Requisitos

- Xcode 16 o superior, [XcodeGen](https://github.com/yonaskolb/XcodeGen) y Rust (`rustup`).
- Mac con Apple Silicon. Las librerías se compilan solo para arm64.

## Compilar

```bash
./scripts/build-xcframework.sh   # Rust → ReproductorCore.xcframework + bindings Swift
cd app && xcodegen generate      # genera ReproductorAirPlay.xcodeproj
open ReproductorAirPlay.xcodeproj
```

Esquemas: **Reproductor-iOS** y **Reproductor-macOS**. Para instalarla en un iPhone, elige tu equipo en *Signing & Capabilities*.

Después de cambiar algo en `core/`, vuelve a ejecutar `./scripts/build-xcframework.sh`.

## Probar el núcleo sin la app

```bash
cd core
cargo test --lib
cargo run --release --bin reproductor-cli -- /ruta/pelicula.mkv [id_pista_audio]
```

Imprime una URL `http://192.168.x.x:PUERTO/…/index.m3u8`. Ábrela en Safari o en QuickTime (*Archivo › Abrir ubicación*) y envíala por AirPlay.

## Pendiente / ideas

- Subtítulos (SRT/ASS → WebVTT en el HLS).
- E-AC-3 7.1 con substreams dependientes (hoy se anuncia como 5.1).
- Recodificar el audio DTS/TrueHD a AAC.
