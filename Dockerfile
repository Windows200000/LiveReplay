# LiveReplay server + web viewer. No game data is in the image: clients upload what the web
# viewer needs (see README > Server).
FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
# wasm-bindgen-cli must match the wasm-bindgen version in Cargo.lock.
RUN apt-get update && apt-get install -y --no-install-recommends binaryen && rm -rf /var/lib/apt/lists/* \
 && rustup target add wasm32-unknown-unknown \
 && cargo install wasm-bindgen-cli --locked --version "$(grep -A1 '^name = "wasm-bindgen"$' Cargo.lock | sed -n 's/^version = "\(.*\)"/\1/p')"
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
