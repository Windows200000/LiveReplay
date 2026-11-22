<p align="center"><img src="assets/icon.png" width="96" alt="LiveReplay icon"></p>

# LiveReplay

Watch World of Tanks battles on an in-game style minimap or in a simplified 3D view: your saved replays, the battle you
are playing right now, and other players' battles in the browser.

Website: **https://wot.arne.my** · Download: [latest release](https://github.com/Windows200000/LiveReplay/releases/latest) · [newest build](https://github.com/Windows200000/LiveReplay/releases/tag/latest-build)

> LiveReplay is a fan project and is not affiliated with or endorsed by Wargaming. World of Tanks is a trademark of
> Wargaming. Nothing from the game is part of this repository: maps, models, textures, icons and game definitions are read
> from your own game installation at runtime.

## Features

**Replay library**
* All `.wotreplay` files of your game folder with player, tank, map, mode, date, damage, server and version
* Sort by any column, filter by values (with search), date ranges and damage ranges; right-click for quick filters
* Open in the viewer, watch in the game client, upload to the website, copy the web link, delete from the server

**Viewer (2D)**
* The game's own minimap picture, grid, bases, spawn points and random-event zones
* Tanks with class markers, team colours, labels, HP rings and numbers; spotted / last known / destroyed states
* Shots as ballistic arcs from the barrel to the impact point, hit flashes and damage numbers
* In-game style top bar (team HP, score, class icons), base capture bar (also blocked captures), battle clock
* Team lists, stats card per player (live stats up to that moment and the full battle result), tomato.gg link
* Playback 0.25x-16x, seek bar, adjustable display settings per view, UI scale

**Viewer (3D)**
* Terrain, buildings, rocks and water built from your game files (simplified and cached), textured ground and objects
* Tanks with the recorded hull orientation, turret and gun angles; shots, markers and labels as in 2D
* Orbit / pan / zoom camera, follow a tank, corner minimap (click or drag to move the camera)

**Live battles**
* Follow the battle you are playing right now (from the replay the game writes while you play)
* All tanks, also enemies that were hidden at the start and deaths out of view

**Website** (the same viewer in the browser)
* Battles uploaded by players, newest first, with a player filter; 2D and 3D
* Live battles of players who gave you a viewing code (valid up to one month)
* Several players' uploads of the same battle merged into one view
* Share links to an exact moment, view, selected tank and camera

## Download and use

1. Download the file for your system from the [latest release](https://github.com/Windows200000/LiveReplay/releases/latest)
   (Windows: `livereplay-windows-x64.exe` / `-arm64.exe`; Linux and macOS: `.tar.gz`, x64 or arm64) and run it.
   The [newest build](https://github.com/Windows200000/LiveReplay/releases/tag/latest-build) follows every change. The game folder is found automatically (otherwise set it in Settings).
2. Double-click a replay, or press **Live battle** while playing (replay recording must be on in the game settings).
3. To put battles on the website: Settings > Server, or right-click replays > Upload. Uploading is off until you agree.

Command line: `livereplay [<file.wotreplay> [--at <seconds after battle start>]] [--live] [--2d|--3d] [--play]`,
`livereplay --upload <files>` (upload without opening a window). Test options: `--pin`, `--settings`,
`--look x,z --cam yaw,pitch,dist`, `--link "<share link options>"`.

## Building

Rust (stable). `cargo build --release` builds the desktop app (`target/release/livereplay`). On Linux the egui/winit
libraries are needed (see `.github/workflows/build.yml`). The server and web viewer: see below.

Crates: `livereplay` (desktop app + shared library: replay decoder, game-file readers, viewer), `api` (HTTP API types),
`server`, `web` (WebAssembly viewer).

## Server and web viewer

The desktop app decodes replays (it has the game files) and sends a compact form of each battle to the server; the server
stores and serves it, and the web viewer (the same viewer compiled to WebAssembly) shows it in the browser.

* Run it (Docker): put a `.env` next to `docker-compose.yml` with `DOMAIN` (default `wot.arne.my`), `OPERATOR_NAME`,
  `OPERATOR_CONTACT` (e-mail or URL for data requests), optionally `OPERATOR_ADDRESS`, and `ADMIN_TOKEN` (long random
  string), point the domain's DNS at the machine, then `docker compose up -d --build`. Caddy gets the HTTPS certificate.
  Data lives in the `livereplay-data` volume, or in any host folder when `DATA_PATH` is set in `.env` (e.g. a separate drive:
  `DATA_PATH=/mnt/data/livereplay`; create it and `sudo chown -R 1000:1000` it first, the server runs as uid 1000). Back it up. Without Docker: `cargo build --release -p livereplay-server`,
  `sh web/build.sh`, then run `livereplay-server` (env `BIND`, `DATA_DIR`, `WEB_DIR`, `THREADS`, plus the ones above).
* Limits (env, optional). Uploader keys cost nothing to make, so the limits that matter are per visitor (network
  address; IPv6: rates per /64, daily counts per /48), counted in memory only with a salted hash, forgotten within a
  day: requests `RATE_PER_SEC` (25) / `RATE_BURST` (600), requests at once `MAX_IN_FLIGHT` (8; slow clients can't take
  every worker, `THREADS` default 64), expensive requests (a key's data, new battles, searches) 2/s (burst 30), new
  battles `IP_BATTLES_PER_DAY` (400), new player names `IP_NAMES_PER_DAY` (10), viewing codes `IP_CODES_PER_DAY` (60),
  new game-data names `IP_NEW_ASSETS_PER_DAY` (500), uploaded MB `IP_UPLOAD_MB_PER_DAY` (1024); JSON requests at most
  64 KB; at most 20 different wrong viewing codes per hour (then its codes are ignored, so codes
  can't be guessed). Per uploader key: `MAX_BATTLES_PER_DAY` (300), `MAX_KEY_MB` (2048), 10 player names; whole server:
  `MAX_TOTAL_GB` (50, battles + original replays + shared game data). Behind the proxy the visitor is the last
  `X-Forwarded-For` entry (only trusted from private / loopback addresses, i.e. Caddy). Battles that never got data are
  removed after an hour.
* Small machine: uploads go straight to disk and downloads are streamed from it (measured: 4 parallel 60 MB uploads and
  downloads, 9 MB peak memory); no large file is read or written while the shared state is locked; lists are made from
  a snapshot of the battle index (shared, not copied; made at most every 2 s, outside the lock) and public pages are
  cached. Measured with 100k battles: ~310 MB, a list page 6 ms, other requests answered in ~10 ms while 15 clients
  attack with made-up searches. Caddy receives small request bodies completely before passing them on. Container:
  1 GB memory, 1.5 CPUs.
* HTTP library: tiny_http 0.12 (unmaintained), vendored in `vendor/tiny_http` with fixes (`PATCHES.md`): a request
  claiming a huge `Content-Length` crashed the whole server (found in this review), unbounded header lines.
  Replacing it with a maintained server (hyper) is the longer-term fix.
* Security: the server never runs or interprets uploads. Battle data, original replays and game data are stored as opaque
  bytes and only served back as `application/octet-stream` with `nosniff` and a sandbox CSP; the small JSON descriptions
  are parsed into fixed types and every text in them is bounded and stripped of control characters; header values with
  control characters are never sent. Pages have a strict CSP (own scripts and the wasm only, no framing). Container:
  read-only, no capabilities, no privilege gain; Caddy (always the newest 2.x) limits request bodies (70 MB) and slow clients.
* Shared game data is voted on: the first upload is served at once; a different upload of the same asset is kept as a
  candidate (3 copies at most) and every uploader key has one vote per asset (its latest upload or vote). The copy with
  the most votes is served. Votes count only from keys that uploaded a battle with data, from at most 4 keys per visitor
  and day, and expire after 120 days (so new game data takes over after an update as players' apps vote for it). Only
  uploads whose vote counts are kept as candidates (4 copies at most), so made-up keys can't fill the places or push a
  real copy out; the operator can pin a copy on `/admin`.
* Operator page: `https://<domain>/admin` (type `ADMIN_TOKEN`): overview (biggest uploaders, newest battles, contested
  game data, blocked keys), delete a battle, delete a player's uploads and free the name (optionally every battle the
  player is in: objection from a player who didn't upload), block an uploader key (removes everything it uploaded; also
  by player name or battle), pin a copy of game data, delete one or all game data. Same as `curl` calls, see `api/src/lib.rs`.
* Privacy (GDPR): the policy is served at `/privacy` from the operator settings. No IP addresses or access logs, no cookies,
  no tracking. Uploaders consent in the app before the first upload and can download (`GET /api/me`) or delete
  (`DELETE /api/me`, `DELETE /api/battles/<id>`) their data from Settings. "This player name belongs to another
  uploader" (also shown with `OPERATOR_CONTACT`): the operator deletes that player's uploads on `/admin`, which frees the
  name for the next uploader.
* API: see `api/src/lib.rs`.
* Nothing from the game is part of the server or image. The minimap pictures, map layouts and icons the web viewer needs are
  uploaded by the desktop app from the player's own game files.

## Roadmap

What's planned, what's done, and the project rules. Kept up to date with every change.

### Rules
- No per-map fixes: every fix must be general and work on all maps, including ones never tested.
- Nothing from World of Tanks (textures, models, icons, defs) is ever committed — everything is read from the user's own client at runtime.
- Keep it lean: no whole other repos; only inspiration or licence-compatible snippets, credited in the README.
- After every new build, launch the new exe for the user (keep the current build running).
- Every push to main publishes all six desktop builds to the rolling pre-release `latest-build` (CI `publish` job;
  https://github.com/Windows200000/LiveReplay/releases/tag/latest-build). A `v*` tag makes a versioned release.
- Finish the client before the server.
- Keep this roadmap (in README.md) complete and current at all times, with every request the user made in the session (also those from before context compaction); work through it without stopping until done.

### Open

Nothing open.

### Done
- Releases from CI (user): a push to main whose Cargo.toml version has no release yet creates the tag `v<version>`
  and the release with all six builds (so a release is: raise the version, push to main)
- Button symbols monochrome again, as before the egui 0.37 upgrade (it made the monochrome emoji fonts optional, so
  "⬅ ▶ ⏸ ⏮" came out as colour emoji): egui's `monochrome_emoji_fonts` turned on
- Release 1.1.0 (user): everything below since 1.0.0 (security work, dependency upgrades, smooth live playback,
  self-updating lists, damage stats); installed apps update themselves to it
- Damage stats next to each team's HP bar (user): dealt, blocked (shells that hit without damage: bounced or not
  penetrated) and missed (shells that hit no tank); whole battle on top, last minute below; hover explains them. Only
  what the recording saw (more with merged viewpoints). Dealt is exact (every health loss by its cause); blocked and
  missed are estimates: such shells times the shooter's median damage per shell in this battle (else its tank type's,
  tier's, everyone's), since a replay doesn't say what a shell would have done. The top bar is a little taller for it;
  numbers (dealt, blocked, missed left to right on both sides, user) shorten to "4.9k" when narrow. Checked in the browser with `examples/synthlive.rs` (now also shoots)
- Website lists update themselves (user: "especially when a live battle ends"): the battle list refreshes quietly
  every 10 s (no spinner; a failed refresh keeps what is shown), the live list every 5 s (was 15), and when a live
  battle ends the battle list reloads at once (tested in a browser: in the list within 0.5 s of the end)
- Security review (user), all points: per-visitor limits instead of server-wide caps one person could use up
  ("split to ppl"); viewing codes can't be guessed; shared game data voted on (first upload served, the copy most
  apps' keys vote for wins; user's idea, with spam limits); operator page `/admin` (delete battles, a player's own
  uploads or every battle they are in, block keys, game data); admin player delete only removes that player's own
  uploads unless asked (user); uploads/downloads streamed from disk for the small shared server (2 OCPU / 4 GB);
  public lists cached, very high rate limit (user); Debian trixie images, Caddy always newest 2.x
- App: "Show my battles on the server" with Delete per battle (matched by player name, so also uploads from an
  older key; user); deleting all data gives the PC a new upload key and says whom to write to when the name is
  taken (the server's `OPERATOR_CONTACT`, also in the server's error message); the app votes for its own copy of map
  / 3D data (once per game version; map packs now built in a fixed order so copies compare equal)
- Live battles on the website no longer freeze and jump (user): playback stays just far enough behind the newest
  data that the next chunk arrives in time, learnt from how far apart chunks arrive, reached by playing a few percent
  faster/slower (no jumps); the app uploads every 2 s (was 4) and the website fetches every second (was 3), so the
  delay settles at a few seconds. Also for the desktop live view
- Second, independent security review (goal: confirm the fixes, find what is still lacking), each finding re-tested
  by attacking a local server with 100k battles: a request claiming a huge `Content-Length` crashed the whole server
  (tiny_http bug, older than these changes; fixed in a vendored copy); 16 MB JSON of empty strings took ~150 MB each
  (JSON now max 64 KB); slow senders could take all 16 workers (8 at once per visitor, 64 workers, Caddy buffers
  small bodies); made-up list searches held the lock 200 ms each (snapshot outside the lock, searches limited,
  page 118 -> 6 ms); asset candidates could be filled or evicted by made-up keys (only counted uploads kept, operator
  pin); IPv6 daily limits per /48; a later upload could replace a battle's first upload in lists (the first one
  stands for the group); temp file names of a battle's two files could collide (unique names)
- Live playback tested in a browser (headless Chromium, made-up live battle from `examples/synthlive.rs` sent like the
  app does): 0 frozen frames in 75 s once settled
- Dependencies current, `cargo audit` clean (user): egui/eframe 0.37 (glow kept for 3D), ureq 3, sha2 0.11,
  getrandom 0.4, blowfish 0.10, directories 6, brotli 9; self-update refuses releases without GitHub's SHA-256 digest
  and downloads from anywhere but this repo's releases; `web/build.sh` checks the wasm-bindgen download against
  pinned checksums
- No duplicates on manual upload (user): the same battle for the same player (same game battle id, or the same replay
  file) is one entry whatever uploader key sends it; the key that owns the player name takes the entry over (the
  duplicate found on wot.arne.my was one replay file uploaded by an old and a new key). Existing duplicates are merged
  when the server starts, keeping the most complete copy
- Deleting your own data removes every upload / viewpoint of your player names, also those sent by an older key of
  yours (user: "even if the ID changes"); "Download my server data" and deleting single battles cover them too.
  Tested on a local server: old key + new key, merge at start, manual upload reuses the entry, delete removes it
- A live battle no longer stays "live" on the website after it ended in the game (user): the app ends the live entry as
  soon as the game's temp file empties (back in the garage), measured 4 s; before, only the full final upload (or the
  server's 90 s timeout) ended it. If a recording ever contains the after-battle period, that ends it too (recordings
  checked so far stop just before it). The full upload with results follows as before
- Settings save themselves, always (user): every change takes effect and is saved at once; folder fields when they are
  left (not a rescan per key press), also when the window is closed while typing. No Save button
- Builds outside Docker (user's server): `web/build.sh` fetches the right tools itself on Linux, once, into
  `target/tools`: the prebuilt wasm-bindgen matching Cargo.lock (no `cargo install wasm-bindgen-cli`, so no buf_redux /
  multipart warning) and binaryen 125 checked against pinned checksums (the distro's wasm-opt 108 was too old, so the
  viewer was not optimised). The Dockerfile uses the same script; the image's default is 16 server threads
- macOS and Linux (user: "do it for all platforms"): tray (macOS menu bar; Linux StatusNotifier via D-Bus, no GTK
  needed): the window's X keeps LiveReplay running, left click on the icon opens the window, its menu has only Quit;
  start with the computer into the tray (macOS LaunchAgent, Linux XDG autostart); one running copy (a second start
  shows the first window); self-update from the release's .tar.gz (size + SHA-256 + program type checked, the running
  file replaced). Wayland can't hide windows: there the X quits. Linux tested for real in WSL (X11): second start
  hands over, close -> hidden, 0 ms CPU in 15 s, start again -> shown, self-update replaced the program. macOS: builds
  and passes its tests in CI; not run on a real Mac
- Start with the computer on by default (user) was built, then turned back to off: an unsigned program that puts
  itself into the Startup folder at its first start was quarantined by Windows Defender (`Trojan:Win32/Bearfoos.A!ml`,
  behaviour-based; the exe itself scans clean). Off by default, the app writes nothing at start and runs fine. Turning
  the setting on may trigger the same, so the setting says so. The real fix is code signing (e.g. SignPath, free for
  open-source projects) — the user's decision
- Uploads without results: until the game's final results arrive, the battle list shows the damage the recording
  saw; then the battle is sent again with the official final stats (user: "ideally still final stats")
- Battles that end without results: an upload without results is sent again once the game's results show up (checked
  every minute for 24 h; also works for recordings without header, whose tanks identify the battle); a recording that
  stops growing for 5 minutes (game crash, disconnect) is copied (the game reuses its temp file) and uploaded marked
  "interrupted"; if the app was closed during a battle, that battle is uploaded at the next start (its replay, or the
  stopped recording). A live upload that went quiet without being finished is shown as interrupted. Dates of such
  recordings come from the battle id (its start time). Tested on a local server with a separate settings folder
- Duplicate live entries from app restarts (before battles were recognised by their id) are removed when the server
  starts: same uploader, player and map, live data only, at least 3 tanks in common; the most complete one stays
- Multi-POV during a live battle verified with two uploaders sending the same battle live at the same time (local
  server): one live list entry with both names, the other viewpoint fetched in full once, then only what is new.
  Note (user): replays are recorded on each client, so with lag two viewpoints can disagree slightly; close enough
- Defender false positive (`Trojan:Win32/Bearfoos.A!ml`, from the registry Run entry) avoided: start-up via a Startup
  folder shortcut, the exe scans clean. Code signing or a report to Microsoft would be the user's to do
- Website, 3D data missing on the server: a popup says so and the view goes back to 2D. If a player's app is uploading
  it right now (the app announces its 3D files first; the server tracks upload progress, `GET /api/asset-status`), the
  view shows how much has arrived in percent and switches to 3D when it is complete, unless the user went back to 2D
- Identical battles recognised (user request; their idea: same players at the same second): by the game's
  `arenaUniqueID`, read from the replay's first packet (checked: equals the results' arenaUniqueID). The same
  uploader sending the same battle again (app restart, the finished replay of a live upload) continues its entry,
  never a second one; other players' uploads of it form a group (also while live). Older uploads without the id: same
  realm, map, day and shared tanks
- Multi-POV: every uploader's viewpoint can be merged in, also during a live battle (with a viewing code; the other
  viewpoints are fetched incrementally like the main one; new uploaders are picked up every 30 s)
- Lists show such a battle once, with all uploaders' names (finished list and live list)
- Live list: an entry stops being live 90 s after its last update (was 30 minutes); the app's first live upload after a
  (re)start replaces the entry's data instead of adding to it
- Windows tray: the window's X keeps LiveReplay in the tray (setting, on by default); left click on the tray icon opens
  the window, right click has only Quit. In the tray the window is minimised and has no taskbar button, so eframe
  never spins on a window it can't redraw: 0 ms CPU over 15 s. The tray icon has its own thread. Starting LiveReplay
  again while it runs brings the running one to the front (one copy only)
- Windows: start with Windows, into the tray (setting, on by default since the user asked): a shortcut in the Startup folder. A registry
  Run entry made Defender flag the exe (`Trojan:Win32/Bearfoos.A!ml`, a machine-learning false positive); the
  shortcut version scans clean
- New versions: notice in the app and in the tray tooltip; self-update on Windows (download checked against GitHub's
  size and SHA-256, exe swapped, old one removed at the next start); both on by default, both in Settings
- Battles are always uploaded live (when uploading is on). The optional "during the battle" setting (off by default,
  user) is only for the map's 3D data (several MB, once per map): off = it is uploaded after the battle; on = when the
  battle starts, on its own thread so live updates go on (tested: 3D data on the server while the battle was live)
- Settings: "Install new versions automatically" (user's wording)
- Display settings start collapsed; the website links to the latest release ("Download the app")
- Security review of the server (independent review + fixes, each checked by an attack test): nothing uploaded is ever run
  or interpreted. Fixed: header injection through the asset version (could have served a page on the site's origin),
  anyone overwriting shared game data (now only its first uploader; admin reset), unbounded description fields and
  server-wide caps (battles/day, names, codes, assets; assets count toward storage), a slow same-battle search under the
  lock, name squatting by case/realm spelling (names compared case-insensitively), admin token compared in constant time,
  a panic no longer stops workers; CSP / HSTS / no-referrer headers, the page's script moved out of the HTML; Caddy body
  limit and timeouts; container read-only without capabilities; binaryen download checked against pinned checksums
- Asset versions without non-breaking spaces (a header can't carry them, so the app re-uploaded map data after every battle)
- Live uploads end properly: the game leaves an empty `temp.wotreplay` after a battle, which now counts as "no battle", so
  the finished replay, its original file and the 3D packs are uploaded (3D was missing on the server: 404s)
- Server: a finished replay that belongs to the uploader's live entry (same player and map, within 10 min) completes that
  entry instead of making a second one; the admin player delete also frees the name
- Settings file written atomically and re-read on failure; an unreadable one is set aside, never overwritten (a failed
  read could make a new upload key: "this player name belongs to another uploader")
- CI actions on current versions (checkout v7, upload-artifact v7, download-artifact v8)
- No compiler warnings (desktop, examples, web): float literals typed for the coming f32-fallback error; web-only
  dependencies only in the wasm build. Docker uses the prebuilt wasm-bindgen instead of compiling wasm-bindgen-cli
  (which pulled in buf_redux / multipart, rejected by future Rust)
- Web viewer stuck on loading in every browser (TypeError in `__wbindgen_init_externref_table`): the Docker image's
  Debian binaryen 111 pointed the externref table export at the function table; now binaryen 125 from its releases,
  and `web/build.sh` skips wasm-opt older than 125
- Release 1.0.0; CI publishes all six desktop builds (Windows/Linux/macOS, x64 + arm64) to `latest-build` on every push,
  and a versioned release for each `v*` tag
- The battle in progress is read from its packets (the game writes no header until the end): LIVE row and live upload work
- Roadmap: the on-hold baked material textures dropped (user)
- 2D map zoom: mouse wheel zooms around the pointer (up to 10x), right- or middle-drag pans, double-click resets; grid labels,
  markers, labels, bases and circles follow (also the 3D view's corner minimap)
- Replay list: the battle being played shows as a highlighted LIVE row at the top (player, tank, map, mode); click to watch it live
- UI scale slider fixed: it no longer jumps while dragging (only real mouse moves count) and snaps back into place on release
- Deaths of tanks out of view (learned from the battle-wide death record at the moment they die) no longer show as one big damage number at the last known position: damage numbers only for tanks in view; team-list health bars of tanks out of view are faded (health when last seen: replays carry no health updates out of view, checked: `onVehicleHealthChanged` / team health bars are not recorded)
- Damaged / destroyed modules and crew above the name labels (alive tanks only, 2D and 3D), with the game's crit icons
  (orange outline = damaged, red = destroyed; crew tinted red). What replays contain: track hits for every tank in view
  (shown until the tank drives again), and enemy modules / crew when the recording player got that information
  (`showOtherVehicleDamagedDevices`). Module numbering is read from the game (`vehicle.xml` extras). Own-tank module / crew
  states are not recorded in replays at all (checked every message and packet type)
- Web viewer download: Brotli-compressed by the server (1.22 MB instead of 1.52 MB gzipped; wasm-opt also runs in the Docker build)
- eframe/egui 0.28 -> 0.29 (winit 0.30): macOS builds on current stable Rust again (the old objc2 0.4 is gone)
- Labels never overlap for alive tanks (2D and 3D): own tank first, then allies, then enemies; a label that doesn't fit next
  to its marker moves to the nearest free spot with a thin line to its tank (markers may overlap)
- Uploads keep everything: the original .wotreplay is stored next to the compact battle (private: only the uploader can
  download it, deleted with the battle, counted in the storage limits; privacy policy updated)
- Server data can live in a host folder (`DATA_PATH` in `.env`, e.g. on another drive) instead of the Docker volume
- UI scale is a slider again and applies live: while dragging, the slider stays where it was on screen at its original size and
  everything else rescales; also Ctrl + mouse wheel and Ctrl +/- (viewer and replay list); sliders show their filled bar again
- Display settings: a "Display settings" button in the top bar (next to 2D / 3D / Minimap) shows or hides rows above the view:
  "3D" and "Minimap" rows in 3D, a "2D" row in 2D (toggles, HP text, marker size and brightness sliders), plus the UI scale
- Upload errors are short ("can't reach wot.arne.my ...") and the status line is cut to one line (full text on hover)
- Third-party licences: all dependencies are permissive (MIT / Apache-2.0 / BSD / Zlib ...); THIRD_PARTY_LICENSES.md lists
  every crate with its licence and notice files; linked from the app's Settings, the website, and shipped with the release
- First start asks once: "Upload your battles?" (grey "Don't upload" left, blue "Upload" right; Upload turns on live uploads);
  the per-upload consent dialog uses the same buttons
- Icon emblem traced from the World of Tanks logo shape (outer rim, top bar, pointed stem, notched wings; within 1 px of the
  reference), with the metal shading, film strip and LIVE badge
- 3D: right-click a tank on the corner minimap to follow it (like right-clicking it in the 3D view)
- Smaller geometry for the 3D cache and web packs: a triangle corner reuses its vertex's last texture coordinate with a one-byte
  flag (UVs were 2/3 of the data): Mines web pack 9.1 -> 7.5 MB (with the extra cover objects: 9.0 MB)
- Live Oaks (and all maps): game-mode object masks verified against the bases (Standard's two team-base objects carry only the
  Standard bit, Encounter/Assault bases only theirs), so the town houses marked "all modes except Standard" are correctly absent in
  Standard battles; smaller cover objects (cars, walls, boats, kiosks: Environment objects from 5 m instead of 8 m) are now shown
  on every map (+11-13% triangles)
- CI (`.github/workflows/build.yml`): desktop builds + tests on Windows/Linux/macOS x64 and arm64 (binaries as artifacts),
  server tests, wasm build and the Docker image on every push to main — Docker image, server, web, Linux and Windows jobs pass
- Replays left before the battle ended get their results: the app archives the game's cached battle results
  (`%APPDATA%\Wargaming.net\WorldOfTanksattle_results`, the game deletes them after about a day) and learns their
  positional layout per game version by pairing them with replays that have a results block (same values in the same places,
  including each field's default); checked on 22 battles: 5236/5236 fields identical. Viewer stats cards, deaths, winner and the
  replay list's damage column use them. Works only for battles the app saw the results of (it archives at start, on rescans and
  every minute while running)
- 3D cache on disk smaller: geometry quantised + delta-coded (as in the web packs), ground image stored once per map and shared
  by all game modes (uncompressed DDS, loads faster) — e.g. Mines 40 MB per mode -> 9 MB per mode + 27 MB ground once
- Project: renamed to LiveReplay; all upstream (GPL-3.0) code and text replaced (app, scanner, settings, launcher, tank names
  from the client, README, icon, CI); licence CC BY-NC-SA 4.0; published as a fresh repository with a clean history:
  https://github.com/Windows200000/LiveReplay
- Icon: own drawing in the style of the classic World of Tanks replays icon (metal shield with a T emblem, film strip) plus a red LIVE badge, drawn by `examples/makeicon.rs`;
  exe icon, window icon and website favicon
- GDPR: privacy policy at `/privacy` (controller and contact from the server settings; what is stored and not; legal bases:
  consent for uploaders, legitimate interest for other players; retention; rights); consent dialog before the first upload
  (replay or live); Settings: privacy link, "Download my server data" (JSON to Downloads), "Delete all my data on the server"
  (asks twice); right-click a replay > "Delete from server"; operator removal of every battle with a player's name
  (admin token); expired codes deleted; no IP logging (Caddy without access log). Full battle results kept (user: don't trim stats)
- Share links: right-click the map (2D, or 3D off a tank) > "Copy link to this moment and view" — time, 2D/3D, selected tank,
  followed tank, 3D camera (`?t=&v=3d&sel=&follow=&cam=`); web viewer and uploaded replays in the desktop app ("Copy link" button on the web too)
- Merged viewpoints: the server groups uploads of the same battle (same realm, map and day, 3+ shared tank ids); the web viewer
  shows "Also recorded by:" toggles and merges them (aligned on battle start; a tank counts as seen if anyone saw it; one yellow
  view-range circle per uploader; `vp=` in links)
- 3D in the browser: the desktop app uploads compact 3D packs after a battle (never while playing): terrain, map objects for the
  mode, tanks, map-event prefabs — geometry quantised + delta-coded, ground 2048 px, object textures 64 px per tile
  (35-110 MB on desktop -> 5-15 MB per map, ~23 KB per tank); loaded only when 3D is switched on
- Server abuse limits: battles per key per day (300), storage per key (2 GB) and in total (50 GB), player names per key (10); env-overridable
- Stored data stays readable when fields change: frames and packs are format byte + deflate(CBOR, fields by name) (+2.5% size)
- Web viewer: loading progress ("waiting for: ...") and readable errors instead of an endless spinner
- Server (`server/`, Docker + Caddy for HTTPS): small Rust binary, no database server, no game code. Stores battles as the
  desktop client sends them, serves them as is (finished battles immutable + cached forever), JSON lists gzipped, web files compressed once at startup
- Compact battle format (`src/replay/wire.rs`): tracks quantised (cm, ms, 1e-4 rad) and delta-coded, deflated —
  e.g. a 2.2 MB replay -> 325 KB; live battles grow by small frames (only new data + 2 s overlap)
- Map data for the web viewer (map definition + the game's own minimap DDS, ~110 KB per map) and stat icons are uploaded by
  clients once per map/client version from their own game files — nothing from the game is in the repository or image
- Desktop: "Upload the battles I play, live" (Settings, off by default): sent every 4 s while playing, replaced by the full
  replay (with results) when the battle ends. Past replays: right-click > Upload / Upload all shown; "Copy web link"
- Live viewing codes: made in the desktop app (1 day / 1 week / 1 month), shown with copy/revoke. The server keeps only a hash
  of each code; a code unlocks the live battles of the key that made it. Without a code a live battle is invisible (list and data);
  once finished it is public. The browser remembers entered codes and forgets expired/revoked ones
- Web viewer (`web/`, the desktop viewer compiled to WebAssembly): battle list (newest first, player filter, pages),
  live battles for your codes (updates every 3 s, switches to the full replay when it ends), viewer at `/b/<id>`,
  "Copy link to this moment" (`?t=` seconds after battle start)
- First upload for a player name claims it for that PC's key (others can't upload as that player)
- Test tools: `livereplay --upload <files>` (no window, prints links; `LIVEREPLAY_SERVER` for a test server),
  `--settings`, `--link "<share link options>"`, `examples/fakelive.rs` (plays a replay to a server as a live battle),
  `examples/fakeview.rs` (second viewpoint of a battle), `examples/wiresize.rs`, `examples/webpacksize.rs`, `examples/wirecheck.rs`
- 3D performance: map objects drawn indexed with shared vertices, 28 bytes/vertex (no stored normals: flat shading from screen-space derivatives) — about 5x less GPU memory (e.g. Ruinberg 123 MB -> ~23 MB, Steel Hunter city 509 -> ~96 MB)
- Mode column names from the client's own texts (`#menu:bonusType/N`, `#arenas:type/<mode>/name`): e.g. type 20 = "Skirmish · Encounter"; built-in table only as fallback
- Max HP of tanks the replay hasn't described yet (enemies known from info records): hull + strongest turret from the client's vehicle data
- Battle-wide tank state decoded from the state-update packets (type 0x37), matched by structure so it works across client builds: kill records (victim, killer; checked against 1567 deaths in 150 replays, 0 wrong outside Steel Hunter) and enemy info records (id, player name, tank; first spotting by anyone). Live battles now show every death, also out of view, and every enemy once spotted by anyone (skirmishes); finished replays also fall back to the results block
- Finished replays: deaths the stream never showed (out of view) come from the results block (lifetime, killer)
- Bases: white capture circle in 2D and on the 3D ground (radius estimated per battle from when tanks entered it, 50 m default); while capped it tints in the capturing team's colour with a progress arc, and an in-game style bar shows under the top bar (team-coloured progress, tank + number of cappers, "Base capture (37)", time left); blocked captures (both teams inside) show greyed as "Capture blocked (37)" with the defenders' count
- 3D: click or drag on the corner minimap to move the camera there; the minimap shows the camera as a translucent white dot with a view cone
- 3D: follow a tank (right-click it, or "Follow tank" in its stats panel); panning the camera stops following
- Own replay parser in Rust (decrypt, defs from installed client, pickle)
- In-game style 2D minimap: real minimap texture, grid, bases/flags, numbered spawn diamonds, correct battle clock
- Vector class markers (crisp), in-game colours; despotted = faded but filled (keeps last-seen HP); dead = grey
- Labels: player / tank name checkboxes (player on top); HP ring setting; HP text above marker
- Hover stats up to that moment; click to pin (follows tank, click elsewhere closes); tomato.gg link
- Card-style stats panel in a clean tomato.gg-like look (own design, nothing taken from it), stats with the client's own battle-result icons (loaded at runtime)
- Shots (fading lines), white hit flash, stacking/fading damage numbers
- Own tank drawn like others + white ring in 2D and 3D, never faded as "out of view"; yellow 564 m circle (2D and painted on the ground in 3D)
- Live battle mode (temp.wotreplay), half-finished replays
- Replay list: sorting, Mode column, row/cell hover, right-click filter/sort, column filters (value lists, date calendar with range drag, damage range slider + manual entry), select anywhere
- Team side panels: alive first, then class; dead greyed with empty HP bar; hover highlight synced with map
- In-game top bar: HP difference, team HP bars, score, class icons (dead dimmed); 2D above, 3D overlaid
- 3D view (toggle, lightweight when off): cached simplified terrain/tanks/buildings, game-mode object masks (e.g. Steppes' extra hills/rocks from other modes),
  flat translucent blue water planes, detailed baked ground texture, tilted hulls, smooth turret traverse, corner minimap,
  left-drag rotate / right-drag move / wheel zoom
- Random events (only WG random events, not player-destroyed objects): announcement read from `onRandomEvent`; danger-zone
  minimap layer from the announcement until it happens, then the aftermath layer (variant nearest the event); 3D states
  switch at the exact time; pre-placed wrecks (Himmelsdorf train) hidden until the crash, never shown if it didn't happen.
  Verified on Ruinberg (bombing) and Himmelsdorf (airship, train)
- UI scale presets (the old slider rescaled itself while dragging), dark theme
- Live battle button in the replay list; viewer "Go live" follow mode
- 3D cache re-parses automatically after a game update (keyed by client version + format)
- 3D: terrain cropped to the battle area (no stretched map edges); Lost City sandstorm streaks removed
- 3D markers share all 2D marker options (HP ring/text, labels, damage numbers, hit flash, own-tank ring)
- Enemy colour red instead of purple; HP text off / current / max / current-of-max; map brightness per view
- Settings window close button fixed; README credits for referenced projects
- Buildings/objects textured through a per-map texture atlas (model UVs); rocks use their tiled-atlas base texture
- Ground = the client's pre-blended full-res `global_AM` image (no custom bake)
- Terrain handles 129-sample chunks (Westfeld etc. were stair-stepped)
- Display menu: toggle buttons, separate 2D and 3D settings
- Top bar in the in-game layout (HP bars + numbers by the score, class icons below, alive first, HP diff far left), scaled to the view so every icon fits
- Small windows: team lists capped at 25% width with truncated names, map keeps the space
- Shots start at the real muzzle, follow the exact ballistic arc (tracer velocity + gravity; arty arcs), end at the recorded impact point on the tank/ground
- 3D hulls use the replay's recorded yaw/pitch/roll (tanks on their side show correctly); terrain slope only as fallback
- 3D gun elevation from `gunAnglesPacked`, calibrated per tank from its shots (~0.5° error); shot lines start at the barrel tip
- 3D ground: one baked image per map (map colour + ground-texture detail, up to 8192 px / 8 px per m), DXT1 with
  mips, uploaded compressed; building textures 256 px per texture in a right-sized atlas
- 3D "Textures" toggle: off = grey ground/objects, faint blue water
- Stats icons: the client's ring-style statistic icons (prebattle highlights)
- Old 3D cache folders (previous game versions / formats) are deleted automatically
- Stats icons: the client's glowing battle-efficiency icons, recoloured per stat (damage/assist/spotted/shots green,
  kills red, received orange crosshair, shots seen green shell, blocked blue, XP yellow, lifetime grey); drawn icons get a matching glow
- Shots drawn like the game: leave the barrel tip on a ballistic arc (real gravity, reference flight time) fitted to
  land exactly on the recorded impact point (2D and 3D)
- Replay filters: click-only-this when all shown, empty = all, search focused when a menu opens
- Test flag `--pin` (own stats panel pinned at start)
- Simple non-repeating textures (user's choice): ground = game ground textures mixed by the real blend weights, filtered, sampled twice at different scale/rotation (no tile grid), large-scale colour from the overview; every object textured from the game (classic / tiled / atlas materials, plain .dds atlases too)
- Rocks (all maps): atlas files indexed, plain atlas images cropped to their first tile, stone texture projected by world position, coloured by the overview, baked random brightness variation + darker down/side faces as fake detail shading
- 3D ground baked like the terrain shader: per-chunk blend weights (`terrain2/blend_textures`) mix each ground texture with its own projection (`terrain2/layers`); overview colour only as fallback
- Replay list headers aligned with columns, funnel filter icon (user confirmed)
- 3D textures for tiled PBS materials (most buildings: `albedoHeightTile0`); invisible collision shapes (`s_` without shader) no longer drawn
- Test flags: `--2d`, `--play` (plus existing `--at`, `--3d`, `--look`)

## Licence

LiveReplay is licensed under [CC BY-NC-SA 4.0](LICENSE) (attribution, non-commercial, share-alike).

Built with [egui / eframe](https://github.com/emilk/egui) and other open-source libraries under permissive licences
(MIT, Apache-2.0, BSD, Zlib, ...): they stay under their own licences, listed with their notices in
[THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md) (regenerate with `python scripts/third_party_licenses.py`).
