#!/bin/sh
# Builds the web viewer into ./web-dist (served by the server). Needs the wasm32 target
# (`rustup target add wasm32-unknown-unknown`).
#
# Tools: wasm-bindgen matching Cargo.lock and wasm-opt from binaryen 125+. On Linux, missing or
# wrong versions are downloaded once (official release binaries, binaryen checked against pinned
# checksums) into $TARGET_DIR/tools; nothing is compiled for them (`cargo install wasm-bindgen-cli`
# pulls in old crates future Rust rejects). Elsewhere they must be on PATH.
set -e
cd "$(dirname "$0")/.."
TARGET_DIR="${CARGO_TARGET_DIR:-target}"
TOOLS="$TARGET_DIR/tools"
BINARYEN=125
SHA_x86_64=7c3bc16599c8274a04d34a504fe4be2047884f900e0e2da2f6fb9cd667183be4
SHA_aarch64=d0382de3c189a7cbb9fdda93e2966f4557f6a00f201e2c4937c27ca01cead4fc
arch="$(uname -m)"
linux=false
if [ "$(uname -s)" = Linux ] && { [ "$arch" = x86_64 ] || [ "$arch" = aarch64 ]; }; then linux=true; fi

# wasm-bindgen: exactly the version in Cargo.lock.
wb="$(grep -A1 '^name = "wasm-bindgen"$' Cargo.lock | sed -n 's/^version = "\(.*\)"/\1/p')"
WASM_BINDGEN=wasm-bindgen
if ! wasm-bindgen --version 2>/dev/null | grep -q " $wb\$"; then
  if $linux; then
    dir="$TOOLS/wasm-bindgen-$wb-$arch-unknown-linux-musl"
    if [ ! -x "$dir/wasm-bindgen" ]; then
      mkdir -p "$TOOLS"
      curl -sSfL "https://github.com/wasm-bindgen/wasm-bindgen/releases/download/$wb/wasm-bindgen-$wb-$arch-unknown-linux-musl.tar.gz" | tar xz -C "$TOOLS"
    fi
    WASM_BINDGEN="$dir/wasm-bindgen"
  else
    echo "needs wasm-bindgen $wb on PATH" >&2
    exit 1
  fi
fi

# wasm-opt: binaryen 125 or newer. Older ones (e.g. Debian's 111) point the externref table
# export at the function table, and the page fails at start-up.
WASM_OPT=wasm-opt
v=$(wasm-opt --version 2>/dev/null | sed -n 's/.*version \([0-9]*\).*/\1/p')
if [ "${v:-0}" -lt "$BINARYEN" ]; then
  WASM_OPT=""
  if $linux; then
    dir="$TOOLS/binaryen-version_$BINARYEN"
    if [ ! -x "$dir/bin/wasm-opt" ]; then
      mkdir -p "$TOOLS"
      curl -sSfL -o "$TOOLS/binaryen.tgz" "https://github.com/WebAssembly/binaryen/releases/download/version_$BINARYEN/binaryen-version_$BINARYEN-$arch-linux.tar.gz"
      if [ "$arch" = x86_64 ]; then sha=$SHA_x86_64; else sha=$SHA_aarch64; fi
      echo "$sha  $TOOLS/binaryen.tgz" | sha256sum -c - >/dev/null
      tar xzf "$TOOLS/binaryen.tgz" -C "$TOOLS" && rm "$TOOLS/binaryen.tgz"
    fi
    WASM_OPT="$dir/bin/wasm-opt"
  else
    echo "wasm-opt ${v:-missing} is too old (needs $BINARYEN+): not optimising" >&2
  fi
fi

cargo build --profile web --target wasm32-unknown-unknown -p livereplay-web
rm -rf web-dist && mkdir -p web-dist
"$WASM_BINDGEN" --target web --no-typescript --out-dir web-dist "$TARGET_DIR/wasm32-unknown-unknown/web/livereplay_web.wasm"
if [ -n "$WASM_OPT" ]; then
  "$WASM_OPT" -Oz --enable-bulk-memory --enable-nontrapping-float-to-int -o web-dist/livereplay_web_bg.wasm web-dist/livereplay_web_bg.wasm
fi
cp web/index.html web/main.js web/favicon.png web-dist/
ls -l web-dist
