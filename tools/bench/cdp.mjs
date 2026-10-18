// Minimal Chrome DevTools Protocol client (browser-level connection).
export async function connect(port, timeoutMs = 30000) {
  const end = Date.now() + timeoutMs;
  let ver;
  while (!ver) {
    try { ver = await (await fetch(`http://127.0.0.1:${port}/json/version`)).json(); }
    catch { if (Date.now() > end) throw new Error('no debugging port'); await new Promise((r) => setTimeout(r, 100)); }
  }
  return socket(ver.webSocketDebuggerUrl);
}

export async function socket(wsUrl) {
  const ws = new WebSocket(wsUrl);
  let id = 0;
  const wait = new Map();
  ws.onmessage = (m) => { const d = JSON.parse(m.data); if (d.id && wait.has(d.id)) { wait.get(d.id)(d); wait.delete(d.id); } };
  // Node's WebSocket doesn't keep the process alive while it connects: hold it open meanwhile.
  const keep = setInterval(() => {}, 1000);
  await new Promise((ok, bad) => { ws.onopen = ok; ws.onerror = bad; ws.onclose = () => bad(new Error('closed')); }).finally(() => clearInterval(keep));
  const call = (method, params = {}) => new Promise((r) => { wait.set(++id, r); ws.send(JSON.stringify({ id, method, params })); });
  return { call, close: () => ws.close() };
}

export const targets = async (port) => (await fetch(`http://127.0.0.1:${port}/json`)).json();
