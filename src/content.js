// Injected into every page: forwards ULTRA shortcuts, pane focus, type-to-search and SPA url changes.
// Messages carry no page-controlled data beyond one typed character; ULTRA reads the real url from WebView2.
(() => {
  if (window.__ultra) return;
  window.__ultra = 1;
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
  // ULTRA's own shortcuts: modifier combos, caught before the page sees them.
  addEventListener('keydown', (e) => {
    const c = e.ctrlKey || e.metaKey, k = (e.key || '').toLowerCase(), code = e.code || '';
    const digit = /^(Digit|Numpad)([1-9])$/.exec(code);
    let m = null;
    if (c && e.altKey && e.shiftKey && digit) m = 'tows:' + digit[2];
    else if (c && e.altKey && digit) m = 'ws:' + digit[2];
    else if (c && e.altKey && !e.shiftKey && (code === 'Digit0' || code === 'Numpad0')) m = 'ws:0';
    else if (c && e.altKey && code === 'KeyL') m = 'lock';
    else if (c && e.shiftKey && digit) m = 'mv:' + digit[2];
    else if (c && e.shiftKey && code === 'KeyF') m = 'zoom';
    else if (c && e.shiftKey && code === 'KeyP') m = 'palette';
    else if (c && e.shiftKey && code === 'KeyE') m = 'mvnew';
    else if (c && e.shiftKey && code === 'KeyW') m = 'closeat';
    else if (c && e.altKey && k.startsWith('arrow')) m = 'dir:' + k[5];
    else if (c && code === 'KeyT') m = e.shiftKey ? 'reopen' : 'new';
    else if (c && code === 'KeyW') m = 'close';
    else if (c && code === 'KeyL') m = 'focusurl';
    else if (c && code === 'KeyK') m = 'overview';
    else if (c && code === 'Comma') m = 'settings';
    else if (c && k === 'tab') m = e.shiftKey ? 'prev' : 'next';
    else if (c && !e.altKey && digit) m = 'tab:' + digit[2];
    if (m) { e.preventDefault(); e.stopImmediatePropagation(); send(m); }
  }, true);
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
})();
