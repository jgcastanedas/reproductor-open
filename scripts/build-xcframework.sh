#!/usr/bin/env bash
# Compila el núcleo Rust para iOS, simulador iOS y macOS, genera los bindings
# Swift con UniFFI y empaqueta todo en app/ReproductorCore.xcframework.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CORE="$ROOT/core"
APP="$ROOT/app"
GEN="$APP/Generated"
LIB=libreproductor_core

# rustup de Homebrew no instala los atajos de cargo en el PATH.
if ! command -v cargo >/dev/null 2>&1; then
  export PATH="$(dirname "$(rustup which cargo)"):$PATH"
fi

export IPHONEOS_DEPLOYMENT_TARGET=17.0
export MACOSX_DEPLOYMENT_TARGET=14.0

TARGETS=(aarch64-apple-ios aarch64-apple-ios-sim aarch64-apple-darwin)
for t in "${TARGETS[@]}"; do
  rustup target add "$t" >/dev/null 2>&1 || true
done

cd "$CORE"
for t in "${TARGETS[@]}"; do
  echo "▸ cargo build --release --target $t"
  cargo build --release --lib --target "$t"
done

echo "▸ Generando bindings Swift (UniFFI)"
rm -rf "$GEN"
mkdir -p "$GEN/headers"
cargo run --quiet --release --bin uniffi-bindgen -- generate \
  --library "target/aarch64-apple-darwin/release/$LIB.dylib" \
  --language swift --out-dir "$GEN/tmp"
mv "$GEN/tmp/"*.swift "$GEN/ReproductorCore.swift"
mv "$GEN/tmp/"*.h "$GEN/headers/"
mv "$GEN/tmp/"*.modulemap "$GEN/headers/module.modulemap"
rm -rf "$GEN/tmp"

echo "▸ Creando XCFramework"
rm -rf "$APP/ReproductorCore.xcframework"
args=()
for t in "${TARGETS[@]}"; do
  args+=(-library "target/$t/release/$LIB.a" -headers "$GEN/headers")
done
xcodebuild -create-xcframework "${args[@]}" -output "$APP/ReproductorCore.xcframework" >/dev/null

echo "✓ $APP/ReproductorCore.xcframework"
