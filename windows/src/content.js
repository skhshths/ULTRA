// Injected into every page: forwards pane focus, type-to-search and SPA url changes.
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
  // ULTRA's ctrl/alt shortcuts are caught natively (AcceleratorKeyPressed), so they work in every frame.
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
