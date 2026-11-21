# LiveReplay server + web viewer. No game data is in the image: clients upload what the web
# viewer needs (see README > Server).
FROM rust:1-bookworm AS build
WORKDIR /src
# The web build fetches its tools itself (web/build.sh: prebuilt wasm-bindgen, binaryen 125 with
# pinned checksums), the same way as on any Linux server.
RUN rustup target add wasm32-unknown-unknown
COPY . .
RUN cargo build --release -p livereplay-server && sh web/build.sh

FROM debian:bookworm-slim
RUN useradd --uid 1000 --no-create-home livereplay && mkdir /data && chown livereplay /data
COPY --from=build /src/target/release/livereplay-server /usr/local/bin/livereplay-server
COPY --from=build /src/web-dist /srv/web
ENV DATA_DIR=/data WEB_DIR=/srv/web BIND=0.0.0.0:8080 THREADS=16
VOLUME /data
EXPOSE 8080
USER livereplay
CMD ["livereplay-server"]
