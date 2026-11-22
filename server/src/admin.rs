//! The operator's tools (all need `ADMIN_TOKEN`): an overview, removing single battles, a
//! player's uploads, shared game data, and blocking uploader keys. `/admin` is a small page for
//! them (the token is typed in and kept only in that browser tab).

use super::*;

/// The admin page; its script is [`SCRIPT`] (the page's CSP allows no inline scripts).
pub const PAGE: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Admin - LiveReplay</title><link rel="icon" type="image/png" href="/favicon.png">
<meta name="robots" content="noindex">
<style>
body { margin: 0; background: #1b1c1e; color: #ddd; font: 14px/1.5 system-ui, sans-serif; }
main { max-width: 1000px; margin: 0 auto; padding: 20px 16px 60px; }
h1 { font-size: 22px; } h2 { font-size: 16px; margin-top: 26px; color: #fff; }
input, button { font: inherit; background: #2a2c30; color: #eee; border: 1px solid #444; border-radius: 4px; padding: 4px 8px; }
button { cursor: pointer; } button.danger { border-color: #a44; color: #f99; }
table { border-collapse: collapse; width: 100%; } td, th { text-align: left; padding: 3px 8px; border-bottom: 1px solid #333; }
code { background: #2a2c30; padding: 1px 4px; border-radius: 3px; } #out { white-space: pre-wrap; color: #8fd14f; }
.row { display: flex; gap: 8px; flex-wrap: wrap; align-items: center; margin: 6px 0; }
.muted { color: #888; }
</style></head><body><main>
<h1>LiveReplay admin</h1>
<div class="row"><input id="token" type="password" placeholder="ADMIN_TOKEN" size="40"><button id="load">Load overview</button></div>
<div id="out"></div>

<h2>Remove</h2>
<div class="row"><input id="battle" placeholder="battle id or link" size="30"><button id="del-battle" class="danger">Delete battle</button>
<button id="block-battle" class="danger">Block its uploader (and delete all they uploaded)</button></div>
<div class="row"><input id="player" placeholder="EU:playername (or just the name: every server)" size="30">
<button id="del-player" class="danger">Delete this player's uploads, free the name</button>
<label><input id="everywhere" type="checkbox"> also battles others uploaded that the player is in (objection)</label></div>
<div class="row"><button id="block-player" class="danger">Block the key that owns this name (and delete all it uploaded)</button></div>
<div class="row"><input id="asset" placeholder="asset name, e.g. map-10_hills" size="30"><button id="del-asset" class="danger">Delete asset (apps upload it again)</button>
<button id="del-assets" class="danger">Delete ALL shared game data</button></div>

<h2>Overview</h2>
<div id="overview" class="muted">Enter the token and load.</div>
</main><script src="/admin.js"></script></body></html>
"#;

pub const SCRIPT: &str = r#""use strict";
const $ = (id) => document.getElementById(id);
const tok = $("token");
try { tok.value = sessionStorage.getItem("lr-admin") || ""; } catch (e) {}
const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
async function call(method, path) {
  try { sessionStorage.setItem("lr-admin", tok.value); } catch (e) {}
  const r = await fetch(path, { method, headers: { "X-Livereplay-Admin": tok.value } });
  const t = await r.text();
  if (!r.ok) throw new Error(r.status + " " + t);
  return t ? JSON.parse(t) : null;
}
function say(s) { $("out").textContent = s; }
async function act(method, path, what) {
  if (!confirm(what + "?")) return;
  try { say(what + ": " + JSON.stringify(await call(method, path))); load(); } catch (e) { say(e.message); }
}
const battleId = () => $("battle").value.trim().replace(/.*[\/#=]/, "");
function table(head, rows) {
  return "<table><tr>" + head.map((h) => "<th>" + esc(h) + "</th>").join("") + "</tr>" +
    rows.map((r) => "<tr>" + r.map((c) => "<td>" + c + "</td>").join("") + "</tr>").join("") + "</table>";
}
async function load() {
  let o;
  try { o = await call("GET", "/api/admin/overview"); } catch (e) { say(e.message); return; }
  const mb = (b) => (b / 1048576).toFixed(1);
  let h = `<p>${o.battles} battles (${o.live} live, ${o.empty} without data), ${o.assets} assets, ${o.claims} player names, ` +
    `${o.codes} viewing codes, ${mb(o.bytes)} MB stored, ${o.blocked.length} blocked keys.</p>`;
  h += "<h2>Biggest uploaders</h2>" + table(["key", "player names", "battles", "new today", "MB", ""],
    o.uploaders.map((u) => [`<code>${esc(u.key)}</code>`, esc(u.names.join(", ")), u.battles, u.today, mb(u.bytes),
      `<button data-block="${esc(u.key)}" class="danger">block</button>`]));
  h += "<h2>Newest battles</h2>" + table(["id", "player", "map", "date", "key", "KB"],
    o.recent.map((b) => [`<a href="/battle/${esc(b.id)}">${esc(b.id)}</a>`, esc(b.realm + ":" + b.player), esc(b.map), esc(b.date), `<code>${esc(b.key)}</code>`, Math.round(b.size / 1024)]));
  h += "<h2>Contested game data</h2>" + (o.contested.length ? table(["asset", "copies (served first): hash, version, votes, MB"],
    o.contested.map((a) => [esc(a.name) + (a.pinned ? " (pinned)" : ""), a.copies.map((c) => `<code>${esc(c.hash)}</code> ${esc(c.version)} ${c.votes} votes ${mb(c.size)} ` +
      `<button data-pin="${esc(a.name)}" data-hash="${esc(c.hash)}">serve this one</button>`).join("<br>") +
      (a.pinned ? ` <button data-unpin="${esc(a.name)}">let votes decide</button>` : "")])) : "<p class=muted>none</p>");
  const day = (t) => new Date(t * 1000).toISOString().slice(0, 10);
  h += `<h2>Deleted by their uploader (${o.deleted_count}; hidden, erased after the retention time)</h2>` + (o.deleted.length ? table(["id", "player", "map", "date", "key", "deleted", "erased", ""],
    o.deleted.map((b) => [esc(b.id), esc(b.realm + ":" + b.player), esc(b.map), esc(b.date), `<code>${esc(b.key)}</code>`, day(b.deleted), day(b.erase),
      `<button data-restore="${esc(b.id)}">restore</button>`])) : "<p class=muted>none</p>");
  h += "<h2>Blocked keys</h2>" + (o.blocked.length ? table(["key", ""], o.blocked.map((k) => [`<code>${esc(k)}</code>`, `<button data-unblock="${esc(k)}">unblock</button>`])) : "<p class=muted>none</p>");
  $("overview").innerHTML = h;
}
$("load").addEventListener("click", load);
$("del-battle").addEventListener("click", () => act("DELETE", "/api/admin/battles/" + encodeURIComponent(battleId()), "Delete battle " + battleId()));
$("block-battle").addEventListener("click", () => act("POST", "/api/admin/block?battle=" + encodeURIComponent(battleId()), "Block the uploader of " + battleId()));
$("del-player").addEventListener("click", () => {
  const p = $("player").value.trim();
  act("DELETE", "/api/admin/players/" + encodeURIComponent(p) + ($("everywhere").checked ? "?everywhere=1" : ""), "Delete uploads of " + p);
});
$("block-player").addEventListener("click", () => act("POST", "/api/admin/block?player=" + encodeURIComponent($("player").value.trim()), "Block the owner of " + $("player").value.trim()));
$("del-asset").addEventListener("click", () => act("DELETE", "/api/admin/assets/" + encodeURIComponent($("asset").value.trim()), "Delete asset " + $("asset").value.trim()));
$("del-assets").addEventListener("click", () => act("DELETE", "/api/admin/assets", "Delete ALL shared game data"));
$("overview").addEventListener("click", (e) => {
  const b = e.target.closest("button");
  if (b && b.dataset.block) act("POST", "/api/admin/block?key=" + encodeURIComponent(b.dataset.block), "Block key " + b.dataset.block);
  if (b && b.dataset.pin) act("POST", "/api/admin/assets/" + encodeURIComponent(b.dataset.pin) + "/pin?hash=" + encodeURIComponent(b.dataset.hash), "Always serve this copy of " + b.dataset.pin);
  if (b && b.dataset.unpin) act("DELETE", "/api/admin/assets/" + encodeURIComponent(b.dataset.unpin) + "/pin", "Let votes decide " + b.dataset.unpin);
  if (b && b.dataset.restore) act("POST", "/api/admin/battles/" + encodeURIComponent(b.dataset.restore) + "/restore", "Restore battle " + b.dataset.restore);
  if (b && b.dataset.unblock) act("DELETE", "/api/admin/block/" + encodeURIComponent(b.dataset.unblock), "Unblock key " + b.dataset.unblock);
});
"#;

/// Shown key hashes are shortened (enough to tell them apart and to block one).
fn short(k: &str) -> String {
    k.chars().take(12).collect()
}

/// The full key hash that a shortened one (at least 8 characters) stands for, if exactly one.
fn find_key(st: &State, prefix: &str) -> Option<String> {
    if prefix.len() < 8 {
        return None;
    }
    let mut found: HashSet<&String> = HashSet::new();
    found.extend(st.battles.values().map(|s| &s.owner).filter(|k| k.starts_with(prefix)));
    found.extend(st.claims.values().filter(|k| k.starts_with(prefix)));
    found.extend(st.codes.values().map(|c| &c.owner).filter(|k| k.starts_with(prefix)));
    found.extend(st.blocked.iter().filter(|k| k.starts_with(prefix)));
    (found.len() == 1).then(|| found.into_iter().next().unwrap().clone())
}

#[derive(Serialize)]
struct Uploader {
    key: String,
    names: Vec<String>,
    battles: usize,
    today: usize,
    bytes: u64,
}

#[derive(Serialize)]
struct Recent {
    id: String,
    player: String,
    realm: String,
    map: String,
    date: String,
    key: String,
    size: u64,
}

#[derive(Serialize)]
struct Deleted {
    id: String,
    player: String,
    realm: String,
    map: String,
    date: String,
    key: String,
    /// Unix seconds it was deleted, and when it will be erased.
    deleted: u64,
    erase: u64,
}

#[derive(Serialize)]
struct CopyInfo {
    hash: String,
    version: String,
    votes: usize,
    size: u64,
}

#[derive(Serialize)]
struct Contested {
    name: String,
    pinned: bool,
    copies: Vec<CopyInfo>,
}

#[derive(Serialize)]
struct Overview {
    battles: usize,
    live: usize,
    empty: usize,
    assets: usize,
    claims: usize,
    codes: usize,
    bytes: u64,
    blocked: Vec<String>,
    uploaders: Vec<Uploader>,
    recent: Vec<Recent>,
    contested: Vec<Contested>,
    deleted_count: usize,
    deleted: Vec<Deleted>,
}

fn overview(st: &State) -> Overview {
    let t = now();
    let mut per: HashMap<String, Uploader> = HashMap::new();
    for s in st.battles.values() {
        let u = per.entry(s.owner.clone()).or_insert_with(|| Uploader { key: short(&s.owner), names: vec![], battles: 0, today: 0, bytes: 0 });
        u.battles += 1;
        u.today += usize::from(t.saturating_sub(s.created) < 86400);
        u.bytes += s.info.size + s.info.replay_size;
    }
    for c in st.assets.values().flat_map(|a| a.copies()) {
        let u = per.entry(c.owner.clone()).or_insert_with(|| Uploader { key: short(&c.owner), names: vec![], battles: 0, today: 0, bytes: 0 });
        u.bytes += c.size;
    }
    for (name, k) in &st.claims {
        if let Some(u) = per.get_mut(k) {
            u.names.push(name.clone());
        }
    }
    let bytes = per.values().map(|u| u.bytes).sum();
    let mut uploaders: Vec<Uploader> = per.into_values().collect();
    uploaders.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(b.today.cmp(&a.today)));
    uploaders.truncate(40);
    let mut recent: Vec<&Stored> = st.battles.values().collect();
    recent.sort_by(|a, b| b.created.cmp(&a.created));
    let recent = recent
        .into_iter()
        .take(40)
        .map(|s| Recent { id: s.info.id.clone(), player: s.info.player.clone(), realm: s.info.realm.clone(), map: s.info.map.clone(), date: s.info.date.clone(), key: short(&s.owner), size: s.info.size })
        .collect();
    let mut contested: Vec<Contested> = st
        .assets
        .iter()
        .filter(|(_, m)| !m.others.is_empty() || m.pinned)
        .map(|(name, m)| Contested {
            name: name.clone(),
            pinned: m.pinned,
            copies: m.copies().map(|c| CopyInfo { hash: short(&c.hash), version: c.version.clone(), votes: c.votes.len(), size: c.size }).collect(),
        })
        .collect();
    contested.sort_by(|a, b| a.name.cmp(&b.name));
    contested.truncate(200);
    let keep = env_num("DELETED_KEEP_DAYS", DELETED_KEEP_DAYS) * 86400;
    let mut deleted: Vec<&Stored> = st.deleted.values().collect();
    deleted.sort_by(|a, b| b.deleted.cmp(&a.deleted));
    let deleted_list = deleted
        .into_iter()
        .take(100)
        .map(|s| Deleted {
            id: s.info.id.clone(),
            player: s.info.player.clone(),
            realm: s.info.realm.clone(),
            map: s.info.map.clone(),
            date: s.info.date.clone(),
            key: short(&s.owner),
            deleted: s.deleted,
            erase: s.deleted + keep,
        })
        .collect();
    let mut blocked: Vec<String> = st.blocked.iter().map(|k| short(k)).collect();
    blocked.sort();
    Overview {
        battles: st.battles.len(),
        live: st.battles.values().filter(|s| is_live(s, t)).count(),
        empty: st.battles.values().filter(|s| s.info.size == 0).count(),
        assets: st.assets.len(),
        claims: st.claims.len(),
        codes: st.codes.len(),
        bytes,
        blocked,
        uploaders,
        recent,
        contested,
        deleted_count: st.deleted.len(),
        deleted: deleted_list,
    }
}

/// `realm:name` or `name` (every realm) -> (realm, lower-case name).
fn player_arg(raw: &str) -> (Option<String>, String) {
    match raw.trim().split_once(':') {
        Some((r, n)) => (Some(r.trim().to_ascii_uppercase()), n.trim().to_lowercase()),
        None => (None, raw.trim().to_lowercase()),
    }
}

pub fn route(app: &App, rq: &mut Request, method: &Method, parts: &[&str], q: &str) -> Reply {
    if !is_admin(rq) {
        return Reply::err(403, "admin token required");
    }
    let mut st = app.st();
    match (method, parts) {
        (Method::Get, ["overview"]) => Reply::json(&overview(&st)),
        (Method::Delete, ["battles", id]) => Reply::json(&app.remove_battle(&mut st, id)),
        // Undo a deletion by its uploader (within the retention time).
        (Method::Post, ["battles", id, "restore"]) => Reply::json(&app.restore_battle(&mut st, id)),
        (Method::Delete, ["players", name]) => {
            // The battles uploaded for this player name, whatever key sent them; with
            // `everywhere`, also other players' uploads the player is in.
            let (realm, name) = player_arg(&url_decode(name));
            let everywhere = query(q, "everywhere").is_some();
            let realm_ok = |s: &Stored| realm.as_ref().is_none_or(|r| s.info.realm.eq_ignore_ascii_case(r));
            let ids: Vec<String> = st
                .battles
                .values()
                .filter(|s| realm_ok(s) && (s.info.player.to_lowercase() == name || (everywhere && s.players.contains(&name))))
                .map(|s| s.info.id.clone())
                .collect();
            for id in &ids {
                app.remove_battle(&mut st, id);
            }
            // The name is free again: the next uploader of it claims it.
            st.claims.retain(|claim, _| claim.split_once(':').is_none_or(|(r, p)| p != name || realm.as_ref().is_some_and(|x| x != r)));
            let State { claims, claim_times, .. } = &mut *st;
            claim_times.retain(|c, _| claims.contains_key(c));
            st.dirty.claims = true;
            Reply::json(&ids)
        }
        (Method::Post, ["block"]) => {
            let key = if let Some(id) = query(q, "battle") {
                st.battles.get(&id).map(|s| s.owner.clone())
            } else if let Some(p) = query(q, "player") {
                let (realm, name) = player_arg(&p);
                let mut owners: Vec<&String> = st.claims.iter().filter(|(c, _)| c.split_once(':').is_some_and(|(r, n)| n == name && realm.as_ref().is_none_or(|x| x == r))).map(|(_, k)| k).collect();
                owners.sort();
                owners.dedup();
                (owners.len() == 1).then(|| owners[0].clone())
            } else {
                query(q, "key").and_then(|k| find_key(&st, &k))
            };
            let Some(key) = key else { return Reply::err(404, "no such uploader (or more than one: be more specific)") };
            let n = forget_uploader(app, &mut st, &key, true);
            st.blocked.insert(key.clone());
            st.dirty.blocked = true;
            println!("blocked uploader {}, removed {n} battles", short(&key));
            Reply::json(&n)
        }
        (Method::Delete, ["block", prefix]) => {
            let Some(key) = find_key(&st, prefix).filter(|k| st.blocked.contains(k)) else { return Reply::err(404, "not blocked") };
            st.blocked.remove(&key);
            st.dirty.blocked = true;
            Reply::json(&())
        }
        (Method::Post | Method::Delete, ["assets", name, "pin"]) if valid_asset_name(name) => {
            // Serve the copy whose hash starts with `hash` and keep it (votes change nothing);
            // DELETE: let votes decide again.
            let Some(mut m) = st.assets.get(*name).cloned() else { return Reply::err(404, "no such asset") };
            if *method == Method::Delete {
                m.pinned = false;
            } else {
                let prefix = query(q, "hash").unwrap_or_default();
                if prefix.len() < 8 {
                    return Reply::err(400, "hash (at least 8 characters) required");
                }
                if !m.current.hash.starts_with(&prefix) {
                    let Some(i) = m.others.iter().position(|c| c.hash.starts_with(&prefix)) else { return Reply::err(404, "no such copy") };
                    app.promote(name, &mut m, i);
                    if !m.current.hash.starts_with(&prefix) {
                        return Reply::err(500, "could not switch copies");
                    }
                }
                m.pinned = true;
            }
            app.save_asset(name, &m);
            st.assets.insert(name.to_string(), m);
            Reply::json(&())
        }
        (Method::Delete, ["assets", name]) if valid_asset_name(name) => {
            app.remove_asset(&mut st, name);
            Reply::json(&())
        }
        (Method::Delete, ["assets"]) => {
            // Reset the shared game data: the next uploaders' apps send it again.
            let names: Vec<String> = st.assets.keys().cloned().collect();
            for name in &names {
                app.remove_asset(&mut st, name);
            }
            Reply::json(&names.len())
        }
        _ => Reply::err(404, "not found"),
    }
}
