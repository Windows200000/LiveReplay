# Changes to tiny_http 0.12.0

The upstream crate has had no release since 2022. Changes made here:

1. `src/util/equal_reader.rs`: when a request body is not read to the end, the rest was skipped
   by allocating a buffer as large as the remaining `Content-Length` at once. A request claiming
   a huge length (e.g. 10^15 bytes) crashed the whole server, and every rejected large upload
   cost its full size in memory. Now skipped through a fixed 64 KB buffer.
2. `src/client.rs`: request and header lines are limited to 16 KB and a request to 200 header
   lines (before, unbounded: memory per connection grew with whatever the client sent).
3. TLS features removed from `Cargo.toml` (not used: the reverse proxy does HTTPS).
