#!/bin/sh
# Builds the web viewer into ./web-dist (served by the server). Needs the wasm32 target and
# wasm-bindgen-cli matching Cargo.lock; uses wasm-opt when installed.
set -e
cd "$(dirname "$0")/.."
TARGET_DIR="${CARGO_TARGET_DIR:-target}"
cargo build --profile web --target wasm32-unknown-unknown -p livereplay-web
rm -rf web-dist && mkdir -p web-dist
wasm-bindgen --target web --no-typescript --out-dir web-dist "$TARGET_DIR/wasm32-unknown-unknown/web/livereplay_web.wasm"
if command -v wasm-opt >/dev/null; then
  wasm-opt -Oz --enable-bulk-memory --enable-nontrapping-float-to-int -o web-dist/livereplay_web_bg.wasm web-dist/livereplay_web_bg.wasm
fi
cp web/index.html web/favicon.png web-dist/
ls -l web-dist
