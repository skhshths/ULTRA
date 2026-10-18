// ULTRA vs Arc vs Neo: start-up, opening N tabs, memory, idle CPU and tab switching.
// usage: node bench.mjs <browser> <n,n,...>     browser: ultra | ultra-awake | neo | arc
import { spawn, execFileSync, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { connect, socket, targets } from './cdp.mjs';
import { start, url } from './server.mjs';

const HERE = path.dirname(new URL(import.meta.url).pathname).replace(/^\/([A-Za-z]:)/, '$1');
const LOCAL = process.env.LOCALAPPDATA;
// Browser profiles for the test (never your real ones); results land next to this script.
const PROFILES = path.join(os.tmpdir(), 'ultra-bench-profiles');
const [which, counts = '1,10,25,50'] = process.argv.slice(2);
const NS = counts.split(',').map(Number);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const log = (...a) => console.log(`[${which}]`, ...a);

const evs = [];
const waiters = new Set();
const srv = await start((e) => { evs.push(e); for (const w of waiters) w(e); });
// Resolve when an event matching `pred` arrives (or already arrived after `since`).
const until = (pred, since, ms) => new Promise((ok) => {
  const hit = evs.find((e) => e.t >= since && pred(e));
  if (hit) return ok(hit);
  const w = (e) => { if (e.t >= since && pred(e)) { waiters.delete(w); clearTimeout(tm); ok(e); } };
  const tm = setTimeout(() => { waiters.delete(w); ok(null); }, ms);
  waiters.add(w);
});
const mem = (pid, cpu = 0) => JSON.parse(execFileSync('powershell', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', path.join(HERE, 'mem.ps1'), String(pid), String(cpu)], { encoding: 'utf8' }));
// Pages in a window behind other windows count as hidden in Chromium; turn that off everywhere so
// window stacking can't skew one browser.
const NO_OCCLUSION = '--disable-features=CalculateNativeWinOcclusion';
const killTree = (pid) => spawnSync('taskkill', ['/F', '/T', '/PID', String(pid)], { stdio: 'ignore' });

// ── per-browser drivers ──────────────────────────────────────────────────────
function chromium(exe, port, profile, extra = []) {
  let proc, b;
  const opened = [];
  return {
    async launch() {
      proc = spawn(exe, [`--user-data-dir=${profile}`, `--remote-debugging-port=${port}`, '--no-first-run', '--no-default-browser-check', NO_OCCLUSION, ...extra], { stdio: 'ignore', detached: true });
      b = await connect(port, 60000);
      return proc.pid;
    },
    async open(u) { const r = await b.call('Target.createTarget', { url: u }); opened.push(r.result?.targetId); },
    async prepareSwitch(ks) {
      const l = await targets(port);
      const out = {};
      for (const k of ks) { const t = l.find((x) => x.type === 'page' && x.url === url(k)); if (t) out[k] = await socket(t.webSocketDebuggerUrl); }
      return async (k) => out[k]?.call('Page.bringToFront');
    },
    async quit() {
      for (const id of opened) await b.call('Target.closeTarget', { targetId: id });
      await sleep(500);
      await b.call('Browser.close');
      await sleep(2500);
      killTree(proc.pid);
      opened.length = 0;
    },
  };
}

function ultra(awake) {
  let proc, ui, port = 9403;
  const exe = path.join(HERE, '..', '..', 'windows', 'target', 'release', 'ultra-exp.exe');
  const ev = async (e) => (await ui.call('Runtime.evaluate', { expression: e, returnByValue: true })).result?.result?.value;
  return {
    async launch() {
      const local = fs.mkdtempSync(path.join(os.tmpdir(), 'ultra-bench-'));
      const env = { ...process.env, LOCALAPPDATA: local, ULTRA_DEVTOOLS_PORT: String(port),
        WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--enable-features=msSmartScreenProtection ${NO_OCCLUSION} --remote-debugging-port=${port}` };
      if (awake) env.ULTRA_WARM_TABS = '999';
      proc = spawn(exe, [], { stdio: 'ignore', detached: true, env });
      for (let i = 0; i < 600; i++) {
        try { const t = (await targets(port)).find((x) => x.title === 'ULTRA'); if (t) { ui = await socket(t.webSocketDebuggerUrl); break; } } catch {}
        await sleep(50);
      }
      while (!(await ev(`typeof ultra === 'object' && !!window.ipc`))) await sleep(30);
      await ev(`if (!window.__wrapped) { const r = ultra.render; ultra.render = function (st) { window.__S = st; return r.apply(this, arguments); }; window.__wrapped = true; } window.__send = (cmd, arg) => window.ipc.postMessage(JSON.stringify({ cmd, arg }));`);
      await ev(`__send('tour', false)`); // the first-run tour covers the panes (pages count as hidden)
      return proc.pid;
    },
    async open(u) { await ev(`__send('newurl', ${JSON.stringify(u)})`); },
    async prepareSwitch() {
      return async (k) => ev(`(() => { const i = window.__S.tabs.findIndex((t) => t.u === ${JSON.stringify(url(k))}); __send('tab', i); return i; })()`);
    },
    async quit() { killTree(proc.pid); await sleep(1500); },
  };
}

const drivers = {
  neo: () => chromium(path.join(LOCAL, 'Neo', 'Application', 'neo.exe'), 9401, path.join(PROFILES, 'neo'), ['about:blank']),
  // Edge: the same Chromium engine as Chrome, the closest stand-in where Chrome can't run.
  edge: () => chromium('C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe', 9404, path.join(PROFILES, 'edge'), ['about:blank']),
  // Arc keeps its sidebar (your real spaces) outside the profile: every tab opened here is closed again.
  arc: () => chromium(path.join(LOCAL, 'Microsoft', 'WindowsApps', 'Arc.exe'), 9402, path.join(PROFILES, 'arc')),
  ultra: () => ultra(false),
  'ultra-awake': () => ultra(true),
};

// ── one run: start, open N tabs, settle, measure, switch, quit ───────────────
async function run(n) {
  const d = drivers[which]();
  const res = { browser: which, n };
  const t0 = performance.now();
  const pid = await d.launch();
  res.readyMs = Math.round(performance.now() - t0);
  await sleep(3000); // let start-up work finish before timing tabs
  res.base = mem(pid);

  // open N tabs back to back, time until every one has loaded
  const s0 = performance.now();
  for (let k = 0; k < n; k++) await d.open(url(k));
  const loads = new Set();
  const deadline = s0 + 30000 + n * 1500;
  while (loads.size < n && performance.now() < deadline) {
    const e = await until((e) => e.type === 'load' && e.i < n && !loads.has(e.i), s0, deadline - performance.now());
    if (!e) break;
    loads.add(e.i);
    if (loads.size === 1) res.firstLoadMs = Math.round(e.t - s0);
    res.allLoadMs = Math.round(e.t - s0);
  }
  res.loaded = loads.size;

  // Memory and idle CPU over two minutes after the last tab loads: browsers do one-off work a
  // little after tabs go to the background (ULTRA suspends hidden tabs after 20 s and the engine
  // compacts them), so a single early snapshot can catch a burst instead of the steady state.
  res.series = [];
  for (const at of [30, 60, 90, 120]) {
    const wait = s0 + (res.allLoadMs || 0) + at * 1000 - 5000 - performance.now();
    if (wait > 0) await sleep(wait);
    const m = mem(pid, 5);
    res.series.push({ at, wsMB: m.wsMB, privMB: m.privMB, cpuPct: m.cpuPct, procs: m.procs });
  }
  res.mem = res.series.at(-1);

  if (n > 1) {
    const ks = [...new Set([0, n - 2, Math.floor(n / 2), 1, Math.floor(n / 4), Math.floor((3 * n) / 4), n - 1, 2].filter((k) => k >= 0 && k < n))];
    const sw = await d.prepareSwitch(ks);
    await sleep(1000);
    res.switches = [];
    for (const k of ks) {
      const t = performance.now();
      await sw(k);
      const e = await until((e) => e.i === k && e.vis === 'visible' && (e.type === 'vis' || e.type === 'load'), t, 15000);
      res.switches.push(e ? { k, ms: Math.round(e.t - t), how: e.type } : { k, ms: null });
      await sleep(1200);
    }
  }
  await d.quit();
  log(JSON.stringify(res));
  fs.appendFileSync(path.join(HERE, 'results.jsonl'), JSON.stringify(res) + '\n');
}

for (const n of NS) {
  try { await run(n); } catch (e) { log('run failed', n, e.stack); }
  await sleep(4000);
}
srv.close();
process.exit(0);
