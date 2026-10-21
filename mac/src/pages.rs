//! ULTRA's own pages, opened by typing ultra://<name> into the address bar (like Chrome's
//! chrome://settings). Some open one of ULTRA's panels; the rest are documents shown in the tab
//! itself (the webview gets the page as HTML; the tab keeps the ultra:// address).

/// Every page, for ultra://about and the address bar's suggestions: (name, what it shows).
pub const PAGES: &[(&str, &str)] = &[
    ("settings", "settings"),
    ("downloads", "downloads"),
    ("chat", "friends & chat"),
    ("history", "everywhere you've been, searchable"),
    ("memory", "what each tab is using; sleep the heavy ones"),
    ("about", "about ULTRA and its pages"),
    ("privacy", "privacy policy"),
    ("terms", "terms of use and chat rules"),
    ("licenses", "license and third-party notices"),
];

/// Where privacy requests and chat reports go (the same address as PRIVACY.md and TERMS.md).
pub const CONTACT: &str = "oliverbarnet441@gmail.com";

const PRIVACY: &str = include_str!("../../PRIVACY.md");
const TERMS: &str = include_str!("../../TERMS.md");
const NOTICES: &str = include_str!("../../THIRD_PARTY_NOTICES.md");
const LICENSE: &str = include_str!("../../LICENSE");

/// What was typed, as an ULTRA page address: "ultra://Settings/", "ultra:settings" ->
/// "ultra://settings"; "ultra://" -> about; "ultra://home" -> the new-tab page. None: not ours.
pub fn normalize(input: &str) -> Option<String> {
    let lower = input.trim().to_ascii_lowercase();
    let rest = lower.strip_prefix("ultra://").or_else(|| lower.strip_prefix("ultra:"))?;
    let name = rest.trim_start_matches('/').split(['/', '?', '#']).next().unwrap_or("");
    Some(match name {
        "" => "ultra://about".into(),
        "home" | "newtab" => crate::HOME.into(),
        "friends" => "ultra://chat".into(),
        "license" | "notices" | "credits" => "ultra://licenses".into(),
        n => format!("ultra://{n}"),
    })
}

/// Is this one of ULTRA's document pages (shown in the tab)?
pub fn is_page(url: &str) -> bool {
    url.starts_with("ultra://") && panel(url).is_none()
}

/// The panel an address opens instead of a page (the command that opens it).
pub fn panel(url: &str) -> Option<&'static str> {
    match url.strip_prefix("ultra://")? {
        "settings" => Some("settings"),
        "downloads" => Some("downloads"),
        "chat" => Some("friends"),
        _ => None,
    }
}

/// A secret for this run of ULTRA. It's written into the live pages (history, memory) and every
/// `page:` message must carry it: another document that ends up in the same tab never has it.
pub fn secret() -> &'static str {
    static SECRET: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SECRET.get_or_init(|| {
        use std::hash::{BuildHasher, Hasher};
        (0..2u8)
            .map(|i| {
                let mut h = std::collections::hash_map::RandomState::new().build_hasher();
                h.write_u8(i);
                h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
                format!("{:016x}", h.finish())
            })
            .collect()
    })
}

/// The page's HTML.
pub fn html(url: &str) -> String {
    let name = url.strip_prefix("ultra://").unwrap_or("");
    let mut script = "";
    let (title, body) = match name {
        "history" => {
            script = HISTORY_JS;
            ("History", HISTORY_BODY.to_string())
        }
        "memory" => {
            script = MEMORY_JS;
            ("Memory", MEMORY_BODY.to_string())
        }
        "privacy" => ("Privacy Policy", markdown(PRIVACY)),
        "terms" => ("Terms of Use", markdown(TERMS)),
        "licenses" => (
            "Licenses",
            format!(
                "{}<h2 id=\"gpl\">GNU General Public License, version 3</h2><p>ULTRA's own license.</p><pre>{}</pre>",
                markdown(NOTICES),
                escape(LICENSE)
            ),
        ),
        "about" => ("About ULTRA", about()),
        other => (
            "Not found",
            format!("<h1>ultra://{} isn't a page</h1><p>These are:</p>{}", escape(other), list()),
        ),
    };
    let key = if script.is_empty() { String::new() } else { format!("const KEY = '{}';", secret()) };
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>{title}</title><style>
:root {{ color-scheme: dark; }}
body {{ margin: 0; background: #141414; color: #d6d6d6; font: 15px/1.6 system-ui, -apple-system, "Segoe UI", sans-serif; }}
main {{ max-width: 760px; margin: 0 auto; padding: 48px 28px 80px; }}
h1 {{ font-size: 26px; color: #f2f2f2; margin: 0 0 6px; }}
h2 {{ font-size: 19px; color: #ececec; margin: 34px 0 8px; }}
h3 {{ font-size: 16px; color: #e6e6e6; margin: 24px 0 6px; }}
a {{ color: #9fe3d0; }}
code {{ background: #222; padding: 1px 5px; border-radius: 4px; font-size: 13px; }}
pre {{ background: #1b1b1b; border: 1px solid #262626; border-radius: 6px; padding: 12px; overflow-x: auto; font-size: 12px; line-height: 1.45; white-space: pre-wrap; }}
pre code {{ background: none; padding: 0; }}
table {{ border-collapse: collapse; width: 100%; margin: 12px 0; font-size: 14px; }}
th, td {{ border: 1px solid #2c2c2c; padding: 6px 9px; text-align: left; vertical-align: top; }}
th {{ background: #1c1c1c; color: #eee; }}
.pages a {{ font-family: ui-monospace, Consolas, monospace; }}
.muted {{ color: #8a8a8a; }}
{APP_CSS}</style></head><body><main>{body}</main><script>
// Links to other ULTRA pages: ULTRA opens them (a page can't navigate to ultra:// itself).
document.addEventListener('click', (e) => {{
  const a = e.target.closest('a[href^="ultra://"]');
  if (a) {{ e.preventDefault(); window.ipc.postMessage('ultra:' + a.getAttribute('href').slice(8)); }}
}});
{key}{script}</script></body></html>"#
    )
}

/// The live pages (history, memory): they ask with `page:{...}` messages and ULTRA answers by
/// calling their `ultraData(...)`.
const APP_CSS: &str = r#"
.bar { display: flex; gap: 8px; flex-wrap: wrap; align-items: center; margin: 18px 0 8px; }
input[type=search] { flex: 1; min-width: 200px; background: #1b1b1b; border: 1px solid #2c2c2c; border-radius: 6px; color: #eee; padding: 8px 11px; font: inherit; outline: none; }
input[type=search]:focus { border-color: #9fe3d0; }
button { background: #1d1d1d; border: 1px solid #2f2f2f; border-radius: 6px; color: #ccc; padding: 6px 11px; font: inherit; font-size: 13px; cursor: pointer; }
button:hover { border-color: #9fe3d0; color: #fff; }
button:disabled { opacity: .35; cursor: default; border-color: #2f2f2f; }
.day { font-size: 13px; color: #9a9a9a; text-transform: uppercase; letter-spacing: .06em; margin: 26px 0 6px; }
.item { display: grid; grid-template-columns: 52px 1fr auto; gap: 10px; align-items: center; padding: 5px 6px; border-radius: 6px; }
.item:hover { background: #1b1b1b; }
.item .time { color: #777; font-size: 12px; font-variant-numeric: tabular-nums; }
.item .what { min-width: 0; }
.item .what a { color: #e2e2e2; text-decoration: none; display: block; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.item .what a:hover { color: #9fe3d0; }
.item .host { color: #777; font-size: 12px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.item .x { padding: 2px 8px; opacity: 0; }
.item:hover .x { opacity: 1; }
.cards { display: grid; grid-template-columns: repeat(auto-fit, minmax(150px, 1fr)); gap: 10px; margin: 18px 0; }
.card { background: #1a1a1a; border: 1px solid #262626; border-radius: 8px; padding: 12px 14px; }
.card b { display: block; font-size: 24px; color: #f2f2f2; font-weight: 600; font-variant-numeric: tabular-nums; }
.card span { color: #8a8a8a; font-size: 13px; }
.tabs td { vertical-align: middle; }
.tabs td.num, .tabs th.num { text-align: right; font-variant-numeric: tabular-nums; white-space: nowrap; }
.tabs .name { cursor: pointer; color: #e2e2e2; max-width: 360px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.tabs .name:hover { color: #9fe3d0; }
.st { font-size: 12px; padding: 1px 7px; border-radius: 10px; background: #222; color: #aaa; white-space: nowrap; }
.st.visible { background: #17332c; color: #9fe3d0; }
.st.asleep { background: #1c1c1c; color: #666; }
.bar-mem { height: 4px; background: #9fe3d0; border-radius: 2px; opacity: .7; margin-top: 3px; margin-left: auto; }
"#;

const HISTORY_BODY: &str = r#"<h1>History</h1><p class="muted" id="count"></p>
<div class="bar"><input type="search" id="q" placeholder="search your history" autofocus>
<button data-clear="3600">clear last hour</button><button data-clear="86400">last day</button><button data-clear="604800">last week</button><button data-clear="all">everything</button></div>
<div id="list"></div><p class="muted" id="more"></p>"#;

const HISTORY_JS: &str = r#"
const send = (o) => window.ipc.postMessage('page:' + JSON.stringify(Object.assign({ k: KEY }, o)));
let items = [], now = 0;
const $ = (id) => document.getElementById(id);
const el = (tag, cls, text) => { const e = document.createElement(tag); if (cls) e.className = cls; if (text != null) e.textContent = text; return e; };
const host = (u) => { try { return new URL(u).host || u; } catch (_) { return u; } };
const dayName = (t) => {
  const d = new Date(t * 1000), today = new Date(now * 1000);
  const start = (x) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const diff = Math.round((start(today) - start(d)) / 86400000);
  if (diff === 0) return 'today';
  if (diff === 1) return 'yesterday';
  return d.toLocaleDateString(undefined, { weekday: 'long', month: 'long', day: 'numeric', year: d.getFullYear() === today.getFullYear() ? undefined : 'numeric' });
};
function render() {
  const q = $('q').value.trim().toLowerCase();
  const hits = q ? items.filter((v) => (v.t + ' ' + v.u).toLowerCase().includes(q)) : items;
  const shown = hits.slice(0, 600);
  const list = $('list');
  list.replaceChildren();
  let day = '';
  for (const v of shown) {
    const dn = dayName(v.l);
    if (dn !== day) { day = dn; list.append(el('div', 'day', dn)); }
    const row = el('div', 'item');
    row.append(el('div', 'time', new Date(v.l * 1000).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })));
    const what = el('div', 'what');
    const a = el('a', null, v.t || v.u);
    if (/^(https?|file|ultra):/i.test(v.u)) a.href = v.u;
    a.title = v.u;
    what.append(a, el('div', 'host', host(v.u) + (v.n > 1 ? ' · ' + v.n + ' visits' : '')));
    const x = el('button', 'x', '×');
    x.title = 'remove from history';
    x.onclick = () => send({ a: 'forget', u: v.u });
    row.append(what, x);
    list.append(row);
  }
  $('count').textContent = items.length ? items.length + ' pages' : 'nothing here yet';
  $('more').textContent = hits.length > shown.length ? 'showing the latest ' + shown.length + ' of ' + hits.length + ' · search to narrow it down' : (q && !hits.length ? 'nothing matches' : '');
}
let got = false;
window.ultraData = (d) => { got = true; items = d.items; now = d.now; render(); };
$('q').addEventListener('input', render);
document.querySelectorAll('[data-clear]').forEach((b) => { b.onclick = () => {
  const c = b.dataset.clear;
  if (c === 'all' && !confirm('Clear all of your history?')) return;
  send({ a: 'clear', secs: c === 'all' ? 0 : +c });
}; });
// Ask until ULTRA answers (a request sent while the page is still loading can go unanswered).
const hello = () => { if (!got) { send({ a: 'ready' }); setTimeout(hello, 400); } };
hello();
"#;

const MEMORY_BODY: &str = r#"<h1>Memory</h1><p class="muted" id="note"></p>
<div class="cards"><div class="card"><b id="total">…</b><span>ULTRA in all</span></div><div class="card"><b id="cpu">…</b><span>CPU</span></div>
<div class="card"><b id="awake">…</b><span>tabs awake</span></div><div class="card"><b id="asleep">…</b><span>asleep (using nothing)</span></div></div>
<div class="bar"><button id="trim">sleep every hidden tab</button></div>
<table class="tabs"><thead><tr><th>tab</th><th>state</th><th class="num" id="memh">memory</th><th class="num" id="cpuh">CPU</th><th class="num">idle</th><th></th></tr></thead><tbody id="rows"></tbody></table>"#;

const MEMORY_JS: &str = r#"
const send = (o) => window.ipc.postMessage('page:' + JSON.stringify(Object.assign({ k: KEY }, o)));
const $ = (id) => document.getElementById(id);
const el = (tag, cls, text) => { const e = document.createElement(tag); if (cls) e.className = cls; if (text != null) e.textContent = text; return e; };
const mb = (x) => x == null ? '–' : x >= 1024 ? (x / 1024).toFixed(1) + ' GB' : Math.round(x) + ' MB';
const ago = (s) => s < 60 ? 'now' : s < 3600 ? Math.round(s / 60) + ' min' : s < 86400 ? Math.round(s / 3600) + ' h' : Math.round(s / 86400) + ' d';
window.ultraData = (d) => {
  const proc = d.per === 'process';
  $('total').textContent = d.total ? mb(d.total) : '…';
  $('cpu').textContent = d.total ? d.cpu.toFixed(1) + '%' : '…';
  const awake = d.tabs.filter((t) => t.s !== 'asleep').length;
  $('awake').textContent = awake;
  $('asleep').textContent = d.tabs.length - awake;
  $('note').textContent = proc
    ? "each tab runs in its own process, and its numbers are that process's. sleeping a tab frees all of it; it reloads when you come back."
    : "tabs share the engine's processes, so the per-tab number is the page's own JavaScript memory. sleeping a tab frees everything it uses; it reloads when you come back.";
  $('memh').textContent = proc ? 'memory' : 'JS memory';
  $('cpuh').style.display = proc ? '' : 'none';
  const max = Math.max(1, ...d.tabs.map((t) => t.mb || 0));
  const rows = $('rows');
  rows.replaceChildren();
  for (const t of d.tabs) {
    const tr = el('tr');
    const name = el('td');
    const n = el('div', 'name', (t.audio ? '♪ ' : '') + (t.t || t.u));
    n.title = t.u;
    n.onclick = () => send({ a: 'show', id: t.id });
    name.append(n, el('div', 'muted', 'workspace ' + t.ws));
    const st = el('td');
    st.append(el('span', 'st ' + t.s, t.s));
    const m = el('td', 'num', mb(t.mb));
    if (t.mb) { const b = el('div', 'bar-mem'); b.style.width = Math.max(4, t.mb / max * 100) + '%'; m.append(b); }
    const c = el('td', 'num', t.cpu == null ? '–' : t.cpu.toFixed(1) + '%');
    c.style.display = proc ? '' : 'none';
    const act = el('td');
    const b = el('button', null, 'sleep');
    b.disabled = t.s !== 'awake';
    b.title = t.s === 'visible' ? "it's on screen" : t.s === 'asleep' ? 'already asleep' : 'free its memory now';
    b.onclick = () => send({ a: 'sleep', id: t.id });
    act.append(b);
    tr.append(name, st, m, c, el('td', 'num', t.s === 'visible' ? '–' : ago(t.idle)), act);
    rows.append(tr);
  }
};
$('trim').onclick = () => send({ a: 'trim' });
send({ a: 'ready' });
setInterval(() => { if (document.visibilityState === 'visible') send({ a: 'refresh' }); }, 2000);
"#;

fn about() -> String {
    format!(
        "<h1>{name}</h1><p class=\"muted\">{version}</p>\
         <p>A fast, minimal, tiling web browser. Free software under the \
         <a href=\"ultra://licenses\">GNU General Public License v3</a>.</p>\
         <h2>ULTRA's pages</h2><p>Type any of these into the address bar.</p>{list}",
        name = escape(crate::edition::NAME),
        version = escape(&crate::updater::version()),
        list = list()
    )
}

fn list() -> String {
    let rows: String = PAGES
        .iter()
        .map(|(n, d)| format!("<tr><td><a href=\"ultra://{n}\">ultra://{n}</a></td><td>{d}</td></tr>"))
        .collect();
    format!("<table class=\"pages\">{rows}</table>")
}

/// Markdown to HTML, with the repository's links between documents pointing at ULTRA's pages.
fn markdown(md: &str) -> String {
    use pulldown_cmark::{html, Options, Parser};
    let mut out = String::new();
    html::push_html(&mut out, Parser::new_ext(md, Options::ENABLE_TABLES));
    out.replace("href=\"PRIVACY.md\"", "href=\"ultra://privacy\"")
        .replace("href=\"TERMS.md\"", "href=\"ultra://terms\"")
        .replace("href=\"THIRD_PARTY_NOTICES.md\"", "href=\"ultra://licenses\"")
        .replace("href=\"LICENSE\"", "href=\"ultra://licenses#gpl\"")
}

/// Percent-encode text for a URL query value.
pub fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        assert_eq!(normalize("ultra://Settings/").as_deref(), Some("ultra://settings"));
        assert_eq!(normalize("ultra:privacy").as_deref(), Some("ultra://privacy"));
        assert_eq!(normalize("ultra://").as_deref(), Some("ultra://about"));
        assert_eq!(normalize("ultra://home").as_deref(), Some(crate::HOME));
        assert_eq!(normalize("https://ultra.example"), None);
        assert_eq!(panel("ultra://settings"), Some("settings"));
        assert!(is_page("ultra://terms") && !is_page("ultra://settings") && !is_page("https://x.com"));
    }

    #[test]
    fn documents_render() {
        let p = html("ultra://privacy");
        assert!(p.contains("<title>Privacy Policy</title>") && p.contains("<table>"));
        assert!(html("ultra://terms").contains("href=\"ultra://privacy\""));
        assert!(html("ultra://licenses").contains("GNU GENERAL PUBLIC LICENSE"));
        assert!(html("ultra://<x>").contains("ultra://&lt;x&gt;"));
        assert_eq!(url_encode("ULTRA chat report: @bob
hi&bye"), "ULTRA%20chat%20report%3A%20%40bob%0Ahi%26bye");
        assert!(html("ultra://history").contains("window.ultraData") && html("ultra://memory").contains("sleep every hidden tab"));
        assert!(html("ultra://history").contains(secret()) && !html("ultra://privacy").contains(secret()) && secret().len() == 32);
    }
}
