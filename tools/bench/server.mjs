// Test site for the browser comparison: one realistic page per "site", each on its own
// loopback address (127.0.0.N) so every tab is a separate site, like real browsing.
// Pages report load and visibility changes back here, so timing works the same in every browser.
import http from 'node:http';

export const PORT = 8780;
export const SITES = 60;
export const url = (i) => `http://127.0.0.${(i % SITES) + 2}:${PORT}/page?i=${i}`;

const words = 'lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor incididunt ut labore et dolore magna aliqua enim ad minim veniam quis nostrud exercitation ullamco laboris nisi aliquip ex ea commodo consequat'.split(' ');
const text = (seed, n) => { let s = seed, out = []; for (let k = 0; k < n; k++) { s = (s * 1103515245 + 12345) & 0x7fffffff; out.push(words[s % words.length]); } return out.join(' '); };

function page(i) {
  const cards = [];
  for (let k = 0; k < 120; k++) {
    const hue = (i * 37 + k * 11) % 360;
    cards.push(`<article class="card"><svg width="96" height="64" viewBox="0 0 96 64"><rect width="96" height="64" fill="hsl(${hue},60%,55%)"/><circle cx="${20 + k % 50}" cy="32" r="18" fill="hsl(${(hue + 180) % 360},70%,60%)"/></svg><h3>${text(i * 1000 + k, 5)}</h3><p>${text(i * 7000 + k, 60)}</p><a href="#c${k}">read more</a> <button>share</button></article>`);
  }
  return `<!doctype html><html><head><meta charset="utf-8"><title>Site ${i}</title>
<style>
body{margin:0;font:15px/1.5 system-ui,sans-serif;background:#f6f6f4;color:#222}
header{position:sticky;top:0;background:#1d3557;color:#fff;padding:14px 24px;display:flex;gap:18px;align-items:center}
header nav a{color:#cfe3ff;text-decoration:none;margin-right:14px}
main{display:grid;grid-template-columns:repeat(auto-fill,minmax(280px,1fr));gap:16px;padding:20px}
.card{background:#fff;border-radius:10px;box-shadow:0 1px 4px rgba(0,0,0,.12);padding:14px}
.card h3{margin:8px 0 4px;text-transform:capitalize}.card p{color:#555;font-size:13px}
button{border:0;background:#e63946;color:#fff;border-radius:6px;padding:4px 10px}
#feed li{padding:6px 0;border-bottom:1px solid #eee}
</style></head><body>
<header><b>Site ${i}</b><nav><a href="#">home</a><a href="#">news</a><a href="#">sports</a><a href="#">tech</a><a href="#">about</a></nav><span id="clock"></span></header>
<main>${cards.join('')}</main>
<section style="padding:20px"><h2>Live feed</h2><ul id="feed"></ul></section>
<script>
const ev = (type, extra = '') => fetch('/ev?i=${i}&type=' + type + '&vis=' + document.visibilityState + extra, { keepalive: true, cache: 'no-store' }).catch(() => {});
// Some script work, like a typical site: build a feed, keep a clock ticking.
const feed = document.getElementById('feed');
for (let k = 0; k < 400; k++) { const li = document.createElement('li'); li.textContent = 'update ' + k + ': ' + Math.random().toString(36).slice(2); feed.appendChild(li); }
const data = []; for (let k = 0; k < 20000; k++) data.push({ k, v: Math.sin(k) }); window.__data = data;
setInterval(() => { document.getElementById('clock').textContent = new Date().toLocaleTimeString(); }, 1000);
addEventListener('load', () => ev('load'));
document.addEventListener('visibilitychange', () => ev('vis'));
</script></body></html>`;
}

export function start(onEvent) {
  const servers = [];
  const handler = (req, res) => {
    const u = new URL(req.url, 'http://x');
    if (u.pathname === '/page') {
      const body = page(+u.searchParams.get('i') || 0);
      res.writeHead(200, { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'no-store' });
      return res.end(body);
    }
    if (u.pathname === '/ev') {
      onEvent({ t: performance.now(), i: +u.searchParams.get('i'), type: u.searchParams.get('type'), vis: u.searchParams.get('vis') });
      res.writeHead(204, { 'access-control-allow-origin': '*' });
      return res.end();
    }
    res.writeHead(404); res.end();
  };
  return Promise.all(Array.from({ length: SITES }, (_, k) => new Promise((ok, bad) => {
    const s = http.createServer(handler);
    s.on('error', bad);
    s.listen(PORT, `127.0.0.${k + 2}`, () => ok(s));
    servers.push(s);
  }))).then(() => ({ close: () => servers.forEach((s) => s.close()) }));
}
