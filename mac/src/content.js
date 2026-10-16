// Injected into every page: forwards pane focus, type-to-search and SPA url changes, plus the
// in-page halves of glance, distraction hiding and picture-in-picture.
// Messages carry no page-controlled data beyond one typed character, a glance link (shown to the
// user, http/https only) and a CSS selector the user picked; ULTRA reads the real url from the engine.
(() => {
  if (window.__ultra) return;
  window.__ultra = 1;
  let zapping = false; // the zap picker is open (see below)
  const send = (m) => { try { window.ipc.postMessage(m); } catch (_) {} };
  // True when the user is typing into something on the page (incl. inside shadow DOM).
  const typing = () => {
    let a = document.activeElement;
    while (a && a.shadowRoot && a.shadowRoot.activeElement) a = a.shadowRoot.activeElement;
    for (let el = a; el; el = el.parentElement) {
      if (el.isContentEditable) return true;
      const t = el.tagName;
      if (t === 'INPUT' || t === 'TEXTAREA' || t === 'SELECT') return true;
    }
    return false;
  };
  // ULTRA's ctrl/alt shortcuts are caught natively before the page, so they work in every frame.
  // Type-to-search: a bare letter/number jumps to the address bar, but only as a last resort.
  // It listens in the bubble phase, so the page's own single-key shortcuts (YouTube's f/k/j/l,
  // GitHub's t, ...) run first, and it steps aside if the page used the key, while anything is
  // fullscreen, or on pages with video, where single keys are player controls.
  addEventListener('keydown', (e) => {
    if (!window.__ultraType || e.defaultPrevented || e.ctrlKey || e.metaKey || e.altKey || e.isComposing) return;
    if (e.key.length !== 1 || !/[\p{L}\p{N}]/u.test(e.key) || typing()) return;
    if (document.fullscreenElement || document.querySelector('video')) return;
    e.preventDefault();
    send('type:' + e.key);
  });
  addEventListener('pointerdown', () => send('focus'), true);
  addEventListener('focus', () => send('focus'));
  const rep = () => send('url');
  for (const f of ['pushState', 'replaceState']) {
    const o = history[f];
    history[f] = function () { const r = o.apply(this, arguments); rep(); return r; };
  }
  addEventListener('popstate', rep);
  addEventListener('hashchange', rep);

  // ── glance: ctrl/cmd+shift+click a link previews it in a floating pane ──
  addEventListener('click', (e) => {
    if (!(e.ctrlKey || e.metaKey) || !e.shiftKey || e.altKey || e.button !== 0) return;
    const a = e.target.closest && e.target.closest('a[href]');
    if (!a || !/^https?:/i.test(a.href)) return;
    e.preventDefault(); e.stopImmediatePropagation();
    send('glance:' + a.href);
  }, true);
  // Inside a glance (ULTRA sets __ultraGlance): esc closes it, ctrl/cmd+enter keeps it as a split.
  addEventListener('keydown', (e) => {
    if (!window.__ultraGlance || e.isComposing) return;
    if (e.key === 'Escape' && !e.defaultPrevented && !document.fullscreenElement && !zapping) { e.preventDefault(); send('esc'); }
    else if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); send('keep'); }
  });

  // ── picture-in-picture: ctrl/cmd+alt+p, handled here so it counts as a user gesture ──
  const bestVideo = () => {
    const vids = [...document.querySelectorAll('video')].filter((v) => v.readyState > 0);
    const area = (v) => { const r = v.getBoundingClientRect(); return r.width * r.height; };
    return vids.filter((v) => !v.paused).sort((a, b) => area(b) - area(a))[0] || vids.sort((a, b) => area(b) - area(a))[0];
  };
  window.__ultraPip = () => {
    try {
      if (document.pictureInPictureElement) { document.exitPictureInPicture(); return; }
      const v = bestVideo();
      if (!v) { send('pip:none'); return; }
      v.disablePictureInPicture = false;
      if (v.requestPictureInPicture) v.requestPictureInPicture().catch(() => send('pip:fail'));
      else if (v.webkitSetPresentationMode) v.webkitSetPresentationMode(v.webkitPresentationMode === 'picture-in-picture' ? 'inline' : 'picture-in-picture');
      else send('pip:fail');
    } catch (_) { send('pip:fail'); }
  };
  addEventListener('keydown', (e) => {
    if (!(e.ctrlKey || e.metaKey) || !e.altKey || e.shiftKey || e.code !== 'KeyP') return;
    // AltGr is ctrl+alt on Windows: if it typed a character (not p), it isn't our shortcut.
    if (!['p', 'P', 'π', '∏'].includes(e.key)) return;
    e.preventDefault(); e.stopImmediatePropagation();
    window.__ultraPip();
  }, true);

  // ── hide distractions: cookie banners & consent overlays (setting), plus elements the user zapped ──
  // Well-known consent tools only, so ordinary page content is never caught.
  const BANNERS = [
    '#onetrust-consent-sdk', '#onetrust-banner-sdk', '#CybotCookiebotDialog', '#CybotCookiebotDialogBodyUnderlay',
    '.qc-cmp2-container', '#qc-cmp2-container', '.fc-consent-root', '#didomi-host', '#usercentrics-root',
    '#truste-consent-track', '.truste_overlay', '.truste_box_overlay', '[id^="sp_message_container_"]',
    '.cc-window.cc-banner', '#cookie-law-info-bar', '.cky-consent-container', '.cky-overlay',
    '#cmplz-cookiebanner-container', '#moove_gdpr_cookie_info_bar', '.osano-cm-window', '#iubenda-cs-banner',
    '#termly-code-snippet-support', '#cookiescript_injected', '.cookie-consent-banner', '#cookie-banner', '#gdpr-cookie-notice',
  ];
  const style = (id, css) => {
    let s = document.getElementById(id);
    if (!css) { if (s) s.remove(); return; }
    // Scripts run before the page has an <html>; the style is added once it exists.
    const root = document.head || document.documentElement;
    if (!root) { document.addEventListener('readystatechange', () => style(id, css), { once: true }); return; }
    if (!s) { s = document.createElement('style'); s.id = id; root.appendChild(s); }
    if (s.textContent !== css) s.textContent = css;
  };
  // Consent overlays often lock scrolling; give it back, but only once a known banner was found.
  const unlockScroll = () => {
    if (!document.body) return;
    if (!BANNERS.some((q) => document.querySelector(q))) return;
    for (const el of [document.documentElement, document.body]) {
      if (el && getComputedStyle(el).overflow === 'hidden') el.style.setProperty('overflow', 'auto', 'important');
    }
  };
  window.__ultraDeclutterApply = () => {
    if (!window.__ultraDeclutter) { style('ultra-declutter', ''); return; }
    style('ultra-declutter', BANNERS.join(',') + '{display:none!important}');
    unlockScroll();
  };
  if (window.__ultraDeclutter) {
    window.__ultraDeclutterApply();
    addEventListener('DOMContentLoaded', window.__ultraDeclutterApply);
    addEventListener('load', () => { window.__ultraDeclutterApply(); setTimeout(unlockScroll, 1500); });
  }
  // Zapped elements for this site (ULTRA re-sends them after every load).
  window.__ultraZapApply = (sels) => {
    const ok = (sels || []).filter((q) => { try { document.querySelector(q); return true; } catch (_) { return false; } });
    style('ultra-zap', ok.length ? ok.join(',') + '{display:none!important}' : '');
  };
  // Zap picker: hover highlights, click hides that element for good on this site, esc cancels.
  const selectorFor = (el) => {
    const unique = (q) => { try { return document.querySelectorAll(q).length === 1; } catch (_) { return false; } };
    if (el.id && unique('#' + CSS.escape(el.id))) return '#' + CSS.escape(el.id);
    const parts = [];
    for (let n = el; n && n.nodeType === 1 && n !== document.body && n !== document.documentElement; n = n.parentElement) {
      if (n.id && unique('#' + CSS.escape(n.id))) { parts.unshift('#' + CSS.escape(n.id)); break; }
      let p = n.tagName.toLowerCase();
      const cls = [...n.classList].filter((c) => /^[a-zA-Z][\w-]{0,40}$/.test(c)).slice(0, 2);
      if (cls.length) p += '.' + cls.map((c) => CSS.escape(c)).join('.');
      // Position only when the tag and classes don't already single it out (positions break when a site reorders).
      const sib = n.parentElement ? [...n.parentElement.children].filter((c) => c.tagName === n.tagName) : [];
      if (sib.length > 1 && !unique([p, ...parts].join(' > '))) p += `:nth-of-type(${sib.indexOf(n) + 1})`;
      parts.unshift(p);
      if (unique(parts.join(' > '))) break;
    }
    return parts.join(' > ');
  };
  window.__ultraZapPick = () => {
    if (zapping) return;
    zapping = true;
    const box = document.createElement('div');
    box.style.cssText = 'position:fixed;z-index:2147483647;pointer-events:none;border:2px solid #ff6b5f;background:rgba(255,107,95,.15);transition:all .05s;display:none';
    const tip = document.createElement('div');
    tip.textContent = 'click an element to hide it on this site · esc to cancel';
    tip.style.cssText = 'position:fixed;z-index:2147483647;top:8px;left:50%;transform:translateX(-50%);background:#141414;color:#eee;font:12px monospace;padding:6px 10px;border:1px solid #ff6b5f;pointer-events:none';
    document.documentElement.append(box, tip);
    let cur = null;
    const move = (e) => {
      const el = document.elementFromPoint(e.clientX, e.clientY);
      if (!el || el === box || el === tip || el === document.body || el === document.documentElement) return;
      cur = el;
      const r = el.getBoundingClientRect();
      Object.assign(box.style, { display: 'block', left: r.left + 'px', top: r.top + 'px', width: r.width + 'px', height: r.height + 'px' });
    };
    const done = () => {
      zapping = false; box.remove(); tip.remove();
      removeEventListener('mousemove', move, true); removeEventListener('click', click, true); removeEventListener('keydown', key, true);
    };
    const click = (e) => {
      e.preventDefault(); e.stopImmediatePropagation();
      if (cur) { const q = selectorFor(cur); cur.style.setProperty('display', 'none', 'important'); if (q && q.length <= 300) send('zap:' + q); }
      done();
    };
    const key = (e) => { if (e.key === 'Escape') { e.preventDefault(); e.stopImmediatePropagation(); done(); } };
    addEventListener('mousemove', move, true); addEventListener('click', click, true); addEventListener('keydown', key, true);
  };
})();
