# LiveReplay server + web viewer. No game data is in the image: clients upload what the web
# viewer needs (see README > Server).
FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
# Tools as release binaries (x86_64 / aarch64), not compiled here:
# - wasm-bindgen, matching the version in Cargo.lock (compiling wasm-bindgen-cli pulls in old crates
#   that future Rust rejects: buf_redux, multipart);
# - binaryen: Debian's (111) breaks the wasm's table exports (the page never loads).
#   binaryen is checked against its pinned checksums (update them with BINARYEN).
ARG BINARYEN=125
ARG BINARYEN_SHA_x86_64=7c3bc16599c8274a04d34a504fe4be2047884f900e0e2da2f6fb9cd667183be4
ARG BINARYEN_SHA_aarch64=d0382de3c189a7cbb9fdda93e2966f4557f6a00f201e2c4937c27ca01cead4fc
RUN arch="$(uname -m)" \
 && wb="$(grep -A1 '^name = "wasm-bindgen"$' Cargo.lock | sed -n 's/^version = "\(.*\)"/\1/p')" \
 && curl -sSfL "https://github.com/wasm-bindgen/wasm-bindgen/releases/download/${wb}/wasm-bindgen-${wb}-${arch}-unknown-linux-musl.tar.gz" \
    | tar xz -C /opt && ln -s "/opt/wasm-bindgen-${wb}-${arch}-unknown-linux-musl/wasm-bindgen" /usr/local/bin/wasm-bindgen && wasm-bindgen --version \
 && curl -sSfL -o /tmp/binaryen.tgz "https://github.com/WebAssembly/binaryen/releases/download/version_${BINARYEN}/binaryen-version_${BINARYEN}-${arch}-linux.tar.gz" \
 && if [ "$arch" = x86_64 ]; then sha="$BINARYEN_SHA_x86_64"; else sha="$BINARYEN_SHA_aarch64"; fi \
 && echo "$sha  /tmp/binaryen.tgz" | sha256sum -c - \
 && tar xzf /tmp/binaryen.tgz -C /opt && rm /tmp/binaryen.tgz \
 && ln -s "/opt/binaryen-version_${BINARYEN}/bin/wasm-opt" /usr/local/bin/wasm-opt && wasm-opt --version \
 && rustup target add wasm32-unknown-unknown
COPY . .
RUN cargo build --release -p livereplay-server && sh web/build.sh

FROM debian:bookworm-slim
RUN useradd --uid 1000 --no-create-home livereplay && mkdir /data && chown livereplay /data
COPY --from=build /src/target/release/livereplay-server /usr/local/bin/livereplay-server
COPY --from=build /src/web-dist /srv/web
ENV DATA_DIR=/data WEB_DIR=/srv/web BIND=0.0.0.0:8080 THREADS=4
VOLUME /data
EXPOSE 8080
USER livereplay
CMD ["livereplay-server"]
