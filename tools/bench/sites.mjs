// Real-site benchmark: ad-heavy pages loaded fresh (new profile, cold cache) in each browser.
// Per load: time to the load event, requests, bytes downloaded, and how many requests to
// well-known ad/tracker servers got through (a fixed list, independent of ULTRA's own lists).
// usage: node sites.mjs <browser> <runs>      browser: ultra | edge | neo
import { spawn, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { connect, socket, targets } from './cdp.mjs';

const HERE = path.dirname(new URL(import.meta.url).pathname).replace(/^\/([A-Za-z]:)/, '$1');
const [which, runs = '3'] = process.argv.slice(2);
const SITES = process.env.ONE ? [process.env.ONE] : ['https://www.cnn.com/', 'https://www.forbes.com/', 'https://www.yahoo.com/', 'https://www.dailymail.co.uk/home/index.html', 'https://weather.com/'];
const ADS = ['doubleclick.net', 'googlesyndication.com', 'googleadservices.com', 'google-analytics.com', 'googletagmanager.com', 'googletagservices.com',
  'adservice.google.com', 'amazon-adsystem.com', 'adnxs.com', 'rubiconproject.com', 'pubmatic.com', 'openx.net', 'criteo.com', 'criteo.net', 'casalemedia.com',
  'taboola.com', 'outbrain.com', 'scorecardresearch.com', 'chartbeat.com', 'chartbeat.net', 'quantserve.com', 'moatads.com', 'doubleverify.com',
  'adsafeprotected.com', 'indexww.com', '33across.com', 'sharethrough.com', 'teads.tv', 'yieldmo.com', 'adform.net', 'smartadserver.com', 'media.net',
  'hotjar.com', 'bat.bing.com', 'bidswitch.net', 'lijit.com', 'sovrn.com', '3lift.com', 'triplelift.com', 'gumgum.com', 'permutive.com', 'krxd.net',
  'demdex.net', 'omtrdc.net', 'everesttech.net', 'mathtag.com', 'rlcdn.com', 'agkn.com', 'tapad.com', 'adsrvr.org', 'id5-sync.com', 'connatix.com', 'primis.tech'];
const isAd = (u) => { try { const h = new URL(u).hostname; return ADS.some((d) => h === d || h.endsWith('.' + d)); } catch { return false; } };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const NO_OCCLUSION = '--disable-features=CalculateNativeWinOcclusion';
const PORT = 9480;
const tmp = (p) => fs.mkdtempSync(path.join(os.tmpdir(), p));

async function launch() {
  if (which === 'ultra') {
    const local = tmp('ultra-sites-');
    const env = { ...process.env, LOCALAPPDATA: local, ULTRA_DEVTOOLS_PORT: String(PORT), WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--enable-features=msSmartScreenProtection ${NO_OCCLUSION} --remote-debugging-port=${PORT}` };
    const p = spawn(path.join(HERE, '..', '..', 'windows', 'target', 'release', 'ultra-exp.exe'), [], { stdio: 'ignore', detached: true, env });
    let ui; for (let i = 0; i < 400 && !ui; i++) { try { const t = (await targets(PORT)).find((x) => x.title === 'ULTRA'); if (t) ui = await socket(t.webSocketDebuggerUrl); } catch {} await sleep(50); }
    const ev = async (e) => (await Promise.race([ui.call('Runtime.evaluate', { expression: e, returnByValue: true }), sleep(3000).then(() => ({}))])).result?.result?.value;
    while (!(await ev(`typeof ultra === 'object' && !!window.ipc`))) await sleep(50);
    await ev(`window.ipc.postMessage(JSON.stringify({ cmd: 'tour', arg: false }))`);
    await sleep(4000); // first run: the ad-block engine is built from the lists
    await ev(`window.ipc.postMessage(JSON.stringify({ cmd: 'go', arg: 'about:blank' }))`);
    let t; for (let i = 0; i < 80 && !t; i++) { t = (await targets(PORT)).find((x) => x.type === 'page' && x.url === 'about:blank' && x.title !== 'ULTRA'); await sleep(100); }
    return { pid: p.pid, page: t };
  }
  const exe = which === 'edge' ? 'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe' : path.join(process.env.LOCALAPPDATA, 'Neo', 'Application', 'neo.exe');
  const p = spawn(exe, [`--user-data-dir=${tmp(which + '-sites-')}`, `--remote-debugging-port=${PORT}`, '--no-first-run', '--no-default-browser-check', NO_OCCLUSION, 'about:blank'], { stdio: 'ignore', detached: true });
  const b = await connect(PORT, 60000);
  await sleep(3000);
  const r = await b.call('Target.createTarget', { url: 'about:blank' });
  await sleep(500);
  const t = (await targets(PORT)).find((x) => x.id === r.result.targetId);
  return { pid: p.pid, page: t, b };
}

async function measure(page, url) {
  const s = await socket(page.webSocketDebuggerUrl);
  const reqs = new Map();
  let bytes = 0, adThrough = 0, adTried = 0, blocked = 0, loadAt = null, t0 = null;
  const wait = new Map();
  let id = 0;
  // our own client so we can see events
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((ok) => (ws.onopen = ok));
  const call = (method, params = {}) => new Promise((r) => { wait.set(++id, r); ws.send(JSON.stringify({ id, method, params })); });
  ws.onmessage = (m) => {
    const d = JSON.parse(m.data);
    if (d.id && wait.has(d.id)) { wait.get(d.id)(d); wait.delete(d.id); return; }
    const p = d.params || {};
    if (d.method === 'Network.requestWillBeSent') { reqs.set(p.requestId, p.request.url); if (isAd(p.request.url)) adTried++; if (t0 === null) t0 = p.timestamp; }
    if (d.method === 'Network.responseReceived' && isAd(p.response.url) && p.response.status >= 200 && p.response.status < 400) adThrough++;
    if (d.method === 'Network.responseReceived' && p.response.status === 403 && p.response.statusText === 'Blocked by ULTRA') blocked++;
    if (d.method === 'Network.loadingFinished') bytes += p.encodedDataLength || 0;
    if (d.method === 'Network.loadingFailed' && p.blockedReason) blocked++;
    if (d.method === 'Page.loadEventFired' && loadAt === null) loadAt = p.timestamp;
  };
  await call('Network.enable'); await call('Page.enable');
  await call('Network.setCacheDisabled', { cacheDisabled: true });
  await call('Page.navigate', { url });
  for (let i = 0; i < 300 && loadAt === null; i++) await sleep(100); // up to 30 s
  await sleep(6000); // ads and trackers keep loading after the load event
  ws.close(); s.close();
  return { url, loadMs: loadAt && t0 ? Math.round((loadAt - t0) * 1000) : null, requests: reqs.size, mb: +(bytes / 1048576).toFixed(2), adTried, adThrough, blocked };
}

const out = [];
for (let r = 0; r < +runs; r++) {
  for (const url of SITES) {
    const { pid, page } = await launch();
    try {
      const m = await measure(page, url);
      m.browser = which; m.run = r;
      console.log(JSON.stringify(m));
      fs.appendFileSync(path.join(HERE, 'sites.jsonl'), JSON.stringify(m) + '\n');
    } catch (e) { console.log('failed', url, e.message); }
    spawnSync('taskkill', ['/F', '/T', '/PID', String(pid)], { stdio: 'ignore' });
    await sleep(2500);
  }
}
process.exit(0);
