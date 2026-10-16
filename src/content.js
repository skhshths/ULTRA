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
  addEventListener('keydown', (e) => {
    const c = e.ctrlKey || e.metaKey, k = (e.key || '').toLowerCase(), code = e.code || '';
    const digit = /^(Digit|Numpad)([1-9])$/.exec(code);
    let m = null;
    if (c && e.altKey && e.shiftKey && digit) m = 'tows:' + digit[2];
    else if (c && e.altKey && digit) m = 'ws:' + digit[2];
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
    // Type-to-search: a bare letter/number outside any text field jumps to the address bar.
    else if (window.__ultraType && !c && !e.altKey && !e.isComposing && e.key.length === 1
      && /[\p{L}\p{N}]/u.test(e.key) && !typing()) m = 'type:' + e.key;
    if (m) { e.preventDefault(); e.stopImmediatePropagation(); send(m); }
  }, true);
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
