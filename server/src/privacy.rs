//! The privacy policy (`/privacy`) and the Impressum (`/impressum`, in English), filled in from the
//! operator settings (`OPERATOR_NAME`, `OPERATOR_CONTACT`, `OPERATOR_ADDRESS`, optional
//! `OPERATOR_PHONE`).

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
<li><b>The original replay file</b> of each uploaded battle, as the game wrote it (it also contains the battle chat and the
uploader's own detailed results). It is kept so that features can later show more of the battle; it is not shown or given
to anyone except the uploader, and is deleted with the battle.</li>
<li><b>Uploaders:</b> a random key created by the app (stored only as a hash) and the player names that key uploads for.</li>
<li><b>Live viewing codes:</b> stored only as a hash, with the key that made them and their expiry date. Expired codes are deleted.</li>
<li><b>Game data</b> (map pictures, 3D models, icons) uploaded by the app from the player's game files, with the hashed
keys of the apps that sent or confirmed each copy (so that the copy most apps agree on is shown). Votes expire after 120 days.</li>
</ul>

<h2>What is not stored</h2>
<p>No IP addresses or access logs, no cookies, no analytics or tracking, no accounts or passwords, no e-mail addresses.
To stop abuse (too many requests, mass uploads, guessing viewing codes), the server counts requests per network address
in memory only: the address is replaced by a salted hash that changes at every restart, never written to disk, and the
counts are forgotten after at most a day (legitimate interest, Art. 6(1)(f) GDPR, in keeping the service available).
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
on the server</i> (also gives the app a new upload key); single battles: <i>Show my battles on the server</i> &gt;
<i>Delete</i>, or right-click a replay &gt; <i>Delete from server</i>.</li>
<li><b>Players in someone else's upload:</b> write to {contact_html} with your player name and server (e.g. EU).
All battles containing your name are removed, usually within a few days and at the latest within one month.</li>
<li><b>Someone else uploads under your player name</b> (the app says the name "belongs to another uploader"): write to
{contact_html}; their uploads under your name are removed and the name is freed for you.</li>
</ul>

<h2>Recipients</h2>
<p>None. Data is stored on the server that hosts this website and is not shared or sold.</p>

<h2>The LiveReplay desktop app</h2>
<p>The app sends data to this server only when uploading is turned on (see above). To tell you about new versions and
install them, it asks GitHub (GitHub, Inc., USA) for the latest release a few times a day and downloads it from there;
GitHub receives your IP address for this (<a href="https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement">GitHub's privacy statement</a>).
You can turn this off in the app: Settings &gt; App &gt; <i>Tell me about new versions</i>. The links to GitHub on this
website (<i>Download the app</i>, <i>Licences</i>) contact GitHub only when you click them.</p>

<p style="margin-top:36px;color:#888">LiveReplay is a fan project and is not affiliated with or endorsed by Wargaming.
World of Tanks is a trademark of Wargaming. <a href="/impressum">Impressum</a></p>
</main></body></html>"#
    )
}

/// Impressum (§ 5 DDG), in English like the rest of the site.
pub fn legal() -> String {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    let name = env("OPERATOR_NAME").map(|n| esc(&n)).unwrap_or_else(|| "(name not configured: set OPERATOR_NAME)".into());
    let address = env("OPERATOR_ADDRESS")
        .map(|a| esc(&a).replace(", ", "<br>"))
        .unwrap_or_else(|| "(postal address not configured: set OPERATOR_ADDRESS)".into());
    let contact = env("OPERATOR_CONTACT").unwrap_or_else(|| "(contact not configured: set OPERATOR_CONTACT)".into());
    let contact_html = if contact.contains('@') && !contact.contains(' ') {
        format!("E-mail: <a href=\"mailto:{0}\">{0}</a>", esc(&contact))
    } else if contact.starts_with("http") {
        format!("Contact: <a href=\"{0}\">{0}</a>", esc(&contact))
    } else {
        format!("Contact: {}", esc(&contact))
    };
    let phone = env("OPERATOR_PHONE").map(|p| format!("<br>Phone: {}", esc(&p))).unwrap_or_default();
    format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Impressum - LiveReplay</title><link rel="icon" type="image/png" href="/favicon.png">
<style>
body {{ margin: 0; background: #1b1c1e; color: #ddd; font: 15px/1.55 system-ui, sans-serif; }}
main {{ max-width: 760px; margin: 0 auto; padding: 24px 16px 60px; }}
h1 {{ font-size: 26px; }} h2 {{ font-size: 18px; margin-top: 28px; color: #fff; }}
a {{ color: #8fd14f; }}
</style></head><body><main>
<p><a href="/">&larr; LiveReplay</a></p>
<h1>Impressum</h1>
<p style="color:#999">Information according to &sect; 5 DDG</p>

<h2>Operator</h2>
<p>{name}<br>{address}</p>

<h2>Contact</h2>
<p>{contact_html}{phone}</p>

<h2>Responsible for the content</h2>
<p>{name} (address above)</p>

<h2>Notes</h2>
<p>LiveReplay is a non-commercial fan project. It is not affiliated with or endorsed by Wargaming; World of Tanks is a
trademark of Wargaming. Battles on this website are uploaded by players; to have battles with your player name removed,
see the <a href="/privacy">privacy policy</a>. The source code is open:
<a href="https://github.com/Windows200000/LiveReplay">github.com/Windows200000/LiveReplay</a>.</p>
</main></body></html>"#
    )
}
