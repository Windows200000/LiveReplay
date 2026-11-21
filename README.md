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
* Limits (env, optional): `MAX_BATTLES_PER_DAY` (300 per uploader), `MAX_KEY_MB` (2048), `MAX_TOTAL_GB` (50).
* Privacy (GDPR): the policy is served at `/privacy` from the operator settings. No IP addresses or access logs, no cookies,
  no tracking. Uploaders consent in the app before the first upload and can download (`GET /api/me`) or delete
  (`DELETE /api/me`, `DELETE /api/battles/<id>`) their data from Settings. Removal request from a player who didn't upload:
  `curl -X DELETE -H "X-Livereplay-Admin: $ADMIN_TOKEN" https://<domain>/api/admin/players/<player name>` deletes every
  battle with that name (answer within one month).
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

#### Viewer

Nothing open.

#### Server (wot.arne.my)
Priorities: 1) minimise traffic, 2) minimise server-side resource usage.
- [ ] Deploy to wot.arne.my (`docker compose up -d --build` on a host with Docker; DNS to that host) — needs the user's server.
      Until then uploads from the app fail with a network error (uploading is off by default)

### Done
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
