//! The privacy policy page (`/privacy`), filled in from the operator settings
//! (`OPERATOR_NAME`, `OPERATOR_CONTACT`, optional `OPERATOR_ADDRESS`).

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

pub fn page() -> String {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    let name = env("OPERATOR_NAME").unwrap_or_else(|| "the operator of this website".into());
    let contact = env("OPERATOR_CONTACT").unwrap_or_else(|| "(contact not configured: set OPERATOR_CONTACT)".into());
    let contact_html = if contact.contains('@') && !contact.contains(' ') {
        format!("<a href=\"mailto:{0}\">{0}</a>", esc(&contact))
    } else if contact.starts_with("http") {
        format!("<a href=\"{0}\">{0}</a>", esc(&contact))
    } else {
        esc(&contact)
    };
    let address = env("OPERATOR_ADDRESS").map(|a| format!("<br>{}", esc(&a))).unwrap_or_default();
    format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Privacy - LiveReplay</title><link rel="icon" type="image/png" href="/favicon.png">
<style>
body {{ margin: 0; background: #1b1c1e; color: #ddd; font: 15px/1.55 system-ui, sans-serif; }}
main {{ max-width: 760px; margin: 0 auto; padding: 24px 16px 60px; }}
h1 {{ font-size: 26px; }} h2 {{ font-size: 18px; margin-top: 28px; color: #fff; }}
a {{ color: #8fd14f; }} code {{ background: #2a2c30; padding: 1px 5px; border-radius: 4px; }}
</style></head><body><main>
<p><a href="/">&larr; LiveReplay</a></p>
<h1>Privacy policy</h1>

<h2>Who is responsible</h2>
<p>This website is run by {name}.{address}<br>Contact for anything about your data: {contact_html}</p>

<h2>What is stored</h2>
<ul>
<li><b>Battles uploaded by players</b> with the LiveReplay desktop app, as they appear in World of Tanks replay files:
map, time, game mode, the tanks and how they moved and fought, and for every player in the battle their
<b>player name, clan tag, Wargaming account ID, team, tank and the full battle results</b> from the replay's
results (damage, kills, XP, credits and the other per-player statistics the game records).</li>
<li><b>Uploaders:</b> a random key created by the app (stored only as a hash) and the player names that key uploads for.</li>
<li><b>Live viewing codes:</b> stored only as a hash, with the key that made them and their expiry date. Expired codes are deleted.</li>
<li><b>Game data</b> (map pictures, 3D models, icons) uploaded by the app from the player's game files. No personal data.</li>
</ul>

<h2>What is not stored</h2>
<p>No IP addresses or access logs, no cookies, no analytics or tracking, no accounts or passwords, no e-mail addresses.
The website loads nothing from other servers. Your browser keeps your display settings and the viewing codes you entered
in its local storage; codes are only sent to this server to show you the live battles they unlock.
A link to tomato.gg in a player's stats card leaves this site only when you click it.</p>

<h2>Why, and on what legal basis (GDPR)</h2>
<ul>
<li><b>Uploaders:</b> your consent (Art. 6(1)(a) GDPR). Uploading is off until you turn it on or upload a replay yourself.
You can withdraw consent at any time by deleting your data (below); this does not affect earlier processing.</li>
<li><b>Other players in an uploaded battle:</b> legitimate interest (Art. 6(1)(f) GDPR) in showing and analysing
World of Tanks battles, as the game itself shows these names and results to everyone in the battle and in replay files.
You can object at any time (below).</li>
</ul>
<p>Battles are public once they are over. While a battle is being played it is only visible with a viewing code
from the player who uploads it.</p>

<h2>How long</h2>
<p>Battles are kept until the uploader deletes them or a removal request is handled. Viewing codes are deleted when they expire
(at most 31 days) or are revoked.</p>

<h2>Your rights</h2>
<p>You have the right to access, rectification, erasure, restriction of processing, data portability and to object
(Art. 15-21 GDPR), and to complain to a data protection supervisory authority.</p>
<ul>
<li><b>Uploaders:</b> in the LiveReplay app, Settings &gt; Server: <i>Download my server data</i> and <i>Delete all my data
on the server</i>; single battles: right-click a replay &gt; <i>Delete from server</i>.</li>
<li><b>Players in someone else's upload:</b> write to {contact_html} with your player name and server (e.g. EU).
All battles containing your name are removed, usually within a few days and at the latest within one month.</li>
</ul>

<h2>Recipients</h2>
<p>None. Data is stored on the server that hosts this website and is not shared or sold.</p>

<p style="margin-top:36px;color:#888">LiveReplay is a fan project and is not affiliated with or endorsed by Wargaming.
World of Tanks is a trademark of Wargaming.</p>
</main></body></html>"#
    )
}
