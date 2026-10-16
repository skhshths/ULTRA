// Reader view: lays a clean, readable copy of the page's article over the page (the page itself
// is left untouched, so running this again simply takes the overlay away).
// Called by ULTRA as `(READER_JS)(accent)`; reports 'reader:none' when it finds no article.
(acc) => {
  const old = document.getElementById('ultra-reader');
  if (old) { old.remove(); document.documentElement.style.removeProperty('overflow'); return; }
  const send = (m) => { try { window.ipc.postMessage(m); } catch (_) {} };
  const textLen = (el) => (el.innerText || '').trim().length;
  // The article: an obvious container if the page has one, else the block with the most paragraph text.
  const pick = () => {
    for (const q of ['article', '[itemprop="articleBody"]', 'main article', '[role="main"]', 'main', '.post-content', '.entry-content', '.article-body']) {
      const el = [...document.querySelectorAll(q)].sort((a, b) => textLen(b) - textLen(a))[0];
      if (el && el.querySelectorAll('p').length >= 3 && textLen(el) > 600) return el;
    }
    const score = new Map();
    for (const p of document.querySelectorAll('p')) {
      const n = (p.innerText || '').trim().length;
      if (n < 40 || !p.parentElement) continue;
      score.set(p.parentElement, (score.get(p.parentElement) || 0) + n);
    }
    let best = null, top = 0;
    for (const [el, n] of score) if (n > top) { best = el; top = n; }
    return top > 600 ? best : null;
  };
  const src = pick();
  if (!src) { send('reader:none'); return; }
  // Rebuild the content from safe pieces only: text, headings, lists, quotes, code, images, links.
  const KEEP = new Set(['P', 'H1', 'H2', 'H3', 'H4', 'H5', 'H6', 'UL', 'OL', 'LI', 'BLOCKQUOTE', 'PRE', 'CODE', 'FIGURE', 'FIGCAPTION',
    'IMG', 'A', 'EM', 'STRONG', 'B', 'I', 'BR', 'HR', 'TABLE', 'THEAD', 'TBODY', 'TR', 'TD', 'TH', 'SUP', 'SUB', 'SPAN', 'DIV', 'SECTION']);
  const SKIP = 'nav,aside,footer,form,button,script,style,noscript,iframe,svg,[aria-hidden="true"],.share,.social,.related,.comments,.ad,.ads,.advert';
  const clean = (node, out) => {
    for (const c of node.childNodes) {
      if (c.nodeType === 3) { out.appendChild(document.createTextNode(c.textContent)); continue; }
      if (c.nodeType !== 1 || c.matches(SKIP)) continue;
      const style = getComputedStyle(c);
      if (style.display === 'none' || style.visibility === 'hidden') continue;
      if (!KEEP.has(c.tagName)) { clean(c, out); continue; }
      const tag = c.tagName === 'SPAN' || c.tagName === 'SECTION' ? 'span' : c.tagName === 'DIV' ? 'div' : c.tagName.toLowerCase();
      const el = document.createElement(tag);
      if (c.tagName === 'IMG') {
        const s = c.currentSrc || c.src;
        if (!/^https?:/i.test(s) || c.naturalWidth < 80) continue;
        el.src = s; el.alt = c.alt || '';
      } else if (c.tagName === 'A' && /^https?:/i.test(c.href)) el.href = c.href;
      clean(c, el);
      if (c.tagName !== 'IMG' && c.tagName !== 'BR' && c.tagName !== 'HR' && !el.textContent.trim() && !el.querySelector('img')) continue;
      out.appendChild(el);
    }
    return out;
  };
  const h1 = document.querySelector('h1');
  const title = (h1 && h1.innerText.trim()) || document.title;
  const body = clean(src, document.createElement('div'));
  const first = body.querySelector('h1');
  if (first && first.textContent.trim() === title) first.remove();
  const words = (body.innerText || body.textContent || '').trim().split(/\s+/).length;
  const host = document.createElement('div');
  host.id = 'ultra-reader';
  host.style.cssText = 'position:fixed;inset:0;z-index:2147483646;overflow:auto;background:#141414';
  const root = host.attachShadow({ mode: 'open' });
  root.innerHTML = `<style>
    :host { all: initial; }
    .wrap { max-width: 42em; margin: 0 auto; padding: 56px 28px 120px; color: #ddd; font: 19px/1.7 Georgia, 'Iowan Old Style', serif; }
    .meta { font: 12px/1.4 ui-monospace, Menlo, Consolas, monospace; color: #888; letter-spacing: .04em; margin-bottom: 10px; }
    .x { position: fixed; top: 14px; right: 18px; font: 12px ui-monospace, Menlo, Consolas, monospace; color: #888; cursor: pointer; border: 1px solid #333; padding: 4px 9px; background: #141414; }
    .x:hover { color: ${acc}; border-color: ${acc}; }
    h1.t { font: 700 34px/1.25 -apple-system, 'Segoe UI', sans-serif; color: #f2f2f2; margin: 0 0 28px; }
    h1, h2, h3, h4 { font-family: -apple-system, 'Segoe UI', sans-serif; color: #eee; line-height: 1.3; margin: 1.6em 0 .5em; }
    a { color: ${acc}; } img { max-width: 100%; height: auto; display: block; margin: 1.2em auto; border-radius: 4px; }
    blockquote { border-left: 3px solid ${acc}; margin: 1.2em 0; padding: 0 1em; color: #bbb; }
    pre, code { font: 14px/1.5 ui-monospace, Menlo, Consolas, monospace; background: #1e1e1e; } pre { padding: 12px; overflow: auto; }
    figcaption { font-size: 14px; color: #888; text-align: center; } table { border-collapse: collapse; } td, th { border: 1px solid #333; padding: 4px 8px; }
    ::selection { background: ${acc}; color: #141414; }
  </style><div class="x" title="close reader view (f9)">✕ reader</div>
  <div class="wrap"><div class="meta"></div><h1 class="t"></h1><div class="body"></div></div>`;
  root.querySelector('.meta').textContent = [location.hostname.replace(/^www\./, ''), `${Math.max(1, Math.round(words / 230))} min read`].filter(Boolean).join(' · ');
  root.querySelector('.t').textContent = title;
  root.querySelector('.body').appendChild(body);
  root.querySelector('.x').onclick = () => { host.remove(); document.documentElement.style.removeProperty('overflow'); };
  host.addEventListener('keydown', (e) => { if (e.key === 'Escape') { host.remove(); document.documentElement.style.removeProperty('overflow'); } });
  document.documentElement.appendChild(host);
  document.documentElement.style.setProperty('overflow', 'hidden', 'important');
  host.tabIndex = -1; host.focus();
}
