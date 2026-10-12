#!/usr/bin/env node
/**
 * Measures the Ask view on a synthetic long conversation, in Edge with
 * software compositing (--disable-gpu, as the app's WebView2 runs).
 *
 *   npx vite build --config perf/vite.config.ts
 *   node perf/measure.mjs [--runs 3] [--turns 160] [--out result.json]
 *
 * Per run: time to open the conversation (click to first paint, and until
 * the main thread is quiet), scroll frame times through the whole
 * conversation, typing latency in the composer with it open, the renderer's
 * memory, and a streamed answer: frame times while it streams, that it stays
 * pinned to the bottom, and that it leaves a reader who scrolled up alone.
 * No dependencies: a static server and raw CDP over Node's WebSocket.
 */
import { spawn, execFileSync } from 'node:child_process';
import { createServer } from 'node:http';
import { mkdtempSync, readFileSync, rmSync, existsSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const distDir = path.join(here, 'dist');
const args = process.argv.slice(2);
const arg = (name, fallback) => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 ? args[i + 1] : fallback;
};
const RUNS = Number(arg('runs', '3'));
const TURNS = Number(arg('turns', '160'));
const OUT = arg('out', null);
const SHOTS = arg('shots', null);
const PROFILE = arg('profile', null);
const THEME = arg('theme', 'dark');
const EDGE = arg('browser', 'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe');

if (!existsSync(path.join(distDir, 'perf', 'long-chat.html'))) {
  console.error('Build the harness first: npx vite build --config perf/vite.config.ts');
  process.exit(1);
}

const MIME = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml', '.woff2': 'font/woff2', '.woff': 'font/woff', '.ttf': 'font/ttf', '.wasm': 'application/wasm', '.json': 'application/json', '.png': 'image/png' };
const server = createServer((req, res) => {
  const pathname = decodeURIComponent(new URL(req.url, 'http://x').pathname);
  const file = path.join(distDir, pathname);
  if (!file.startsWith(distDir) || !existsSync(file)) {
    res.statusCode = 404;
    res.end();
    return;
  }
  res.setHeader('Content-Type', MIME[path.extname(file)] ?? 'application/octet-stream');
  res.end(readFileSync(file));
});
await new Promise(r => server.listen(0, '127.0.0.1', r));
const port = server.address().port;

const sleep = ms => new Promise(r => setTimeout(r, ms));

class Cdp {
  constructor(url) {
    this.ws = new WebSocket(url);
    this.id = 0;
    this.pending = new Map();
    this.log = [];
    this.ws.onmessage = e => {
      const msg = JSON.parse(e.data);
      if (msg.method === 'Runtime.exceptionThrown') this.log.push(msg.params.exceptionDetails.exception?.description ?? msg.params.exceptionDetails.text);
      if (msg.method === 'Runtime.consoleAPICalled' && msg.params.type === 'error') this.log.push(msg.params.args.map(a => a.value ?? a.description).join(' '));
      if (msg.id && this.pending.has(msg.id)) {
        const { resolve, reject } = this.pending.get(msg.id);
        this.pending.delete(msg.id);
        if (msg.error) reject(new Error(msg.error.message));
        else resolve(msg.result);
      }
    };
  }
  open() {
    return new Promise((resolve, reject) => {
      this.ws.onopen = resolve;
      this.ws.onerror = reject;
    });
  }
  send(method, params = {}) {
    const id = ++this.id;
    this.ws.send(JSON.stringify({ id, method, params }));
    return new Promise((resolve, reject) => this.pending.set(id, { resolve, reject }));
  }
  async eval(expression) {
    const r = await this.send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description ?? r.exceptionDetails.text);
    return r.result.value;
  }
  close() {
    this.ws.close();
  }
}

/** Working set and private bytes of this browser's page renderer (the largest renderer). */
async function rendererMemory(browserCdp) {
  const { processInfo } = await browserCdp.send('SystemInfo.getProcessInfo');
  const pids = processInfo.filter(p => p.type === 'renderer').map(p => p.id);
  if (pids.length === 0) return { pid: 0, workingSetMB: 0, privateMB: 0 };
  const ps = `Get-Process -Id ${pids.join(',')} | ForEach-Object { "$($_.Id) $($_.WorkingSet64) $($_.PrivateMemorySize64)" }`;
  const out = execFileSync('powershell.exe', ['-NoProfile', '-Command', ps], { encoding: 'utf8' }).trim();
  const rows = out.split(/\r?\n/).filter(Boolean).map(l => l.trim().split(/\s+/).map(Number));
  rows.sort((a, b) => b[1] - a[1]);
  const [pid, ws, priv] = rows[0];
  return { pid, workingSetMB: +(ws / 2 ** 20).toFixed(1), privateMB: +(priv / 2 ** 20).toFixed(1) };
}

const pct = (xs, p) => {
  if (xs.length === 0) return 0;
  const s = [...xs].sort((a, b) => a - b);
  return s[Math.min(s.length - 1, Math.floor((p / 100) * s.length))];
};
const median = xs => pct(xs, 50);
const round = x => +x.toFixed(1);

const PAGE_SETUP = `
  window.__m = { longTasks: [], events: [] };
  new PerformanceObserver(l => { for (const e of l.getEntries()) window.__m.longTasks.push({ start: e.startTime, dur: e.duration }); }).observe({ type: 'longtask', buffered: true });
  new PerformanceObserver(l => { for (const e of l.getEntries()) window.__m.events.push({ name: e.name, start: e.startTime, dur: e.duration }); }).observe({ type: 'event', durationThreshold: 16, buffered: true });
  window.__scroller = () => document.querySelector('section[aria-label="Ask"] [role="log"]')?.parentElement ?? null;
  window.__quiet = async (quietMs, maxMs) => {
    const t0 = performance.now();
    for (;;) {
      await new Promise(r => setTimeout(r, 100));
      const last = window.__m.longTasks.reduce((m, t) => Math.max(m, t.start + t.dur), 0);
      const now = performance.now();
      if (now - Math.max(last, t0) >= quietMs || now - t0 > maxMs) return Math.max(last, t0);
    }
  };
  true`;

async function oneRun(run) {
  const profileDir = mkdtempSync(path.join(tmpdir(), 'shodh-perf-'));
  const debugPort = 9300 + Math.floor(Math.random() * 500);
  const browser = spawn(EDGE, [
    '--headless=new', '--disable-gpu', `--remote-debugging-port=${debugPort}`, `--user-data-dir=${profileDir}`,
    '--window-size=1400,900', '--no-first-run', '--no-default-browser-check', '--disable-extensions', 'about:blank',
  ], { stdio: 'ignore' });
  try {
    let target = null;
    for (let i = 0; i < 100 && !target; i++) {
      try {
        const list = await (await fetch(`http://127.0.0.1:${debugPort}/json/list`)).json();
        target = list.find(t => t.type === 'page');
      } catch {
        await sleep(100);
      }
    }
    if (!target) throw new Error('Edge did not start');
    const cdp = new Cdp(target.webSocketDebuggerUrl);
    await cdp.open();
    const version = await (await fetch(`http://127.0.0.1:${debugPort}/json/version`)).json();
    const browserCdp = new Cdp(version.webSocketDebuggerUrl);
    await browserCdp.open();
    await cdp.send('Page.enable');
    await cdp.send('Runtime.enable');
    await cdp.send('Performance.enable');
    await cdp.send('Emulation.setDeviceMetricsOverride', { width: 1400, height: 900, deviceScaleFactor: 1, mobile: false });
    await cdp.send('Page.addScriptToEvaluateOnNewDocument', { source: PAGE_SETUP });
    await cdp.send('Page.navigate', { url: `http://127.0.0.1:${port}/perf/long-chat.html?turns=${TURNS}&theme=${THEME}` });

    // The short conversation is shown first.
    for (let i = 0; i < 200; i++) {
      if (await cdp.eval(`document.body.innerText.includes('What is reciprocal rank fusion?')`)) break;
      await sleep(100);
    }
    await cdp.eval(`window.__quiet(1500, 15000)`);
    const before = await rendererMemory(browserCdp);
    const shot = async name => {
      if (!SHOTS) return;
      const img = await cdp.send('Page.captureScreenshot', { format: 'png' });
      writeFileSync(path.join(SHOTS, `run${run}-${name}.png`), Buffer.from(img.data, 'base64'));
    };
    await shot('loaded');

    // --- Open the long conversation from the sidebar.
    const rect = await cdp.eval(`(() => {
      const el = [...document.querySelectorAll('button, [role="button"], a')].find(b => b.textContent.includes('Synthetic long conversation'));
      if (!el) return null;
      el.scrollIntoView({ block: 'center' });
      const r = el.getBoundingClientRect();
      return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
    })()`);
    if (!rect) {
      const text = await cdp.eval(`document.body.innerText.slice(0, 1500) + '\\nCommands: ' + Object.keys(window.__perf.commands).join(', ')`);
      const shot = await cdp.send('Page.captureScreenshot', { format: 'png' });
      writeFileSync(path.join(tmpdir(), 'shodh-perf-failure.png'), Buffer.from(shot.data, 'base64'));
      throw new Error(`The long conversation is not in the sidebar. Page text:\n${text}\nErrors:\n${cdp.log.slice(0, 10).join('\n')}`);
    }
    await cdp.eval(`(() => {
      window.__open = {};
      document.addEventListener('click', e => {
        window.__open.clickAt = e.timeStamp;
        requestAnimationFrame(() => setTimeout(() => { window.__open.paintAt = performance.now(); }, 0));
      }, { capture: true, once: true });
      window.__m.longTasks.length = 0;
      return true;
    })()`);
    await cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', x: rect.x, y: rect.y, button: 'left', clickCount: 1 });
    await cdp.send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: rect.x, y: rect.y, button: 'left', clickCount: 1 });
    const quietAt = await cdp.eval(`window.__quiet(1000, 30000)`);
    const open = await cdp.eval(`(() => {
      const o = window.__open;
      const lt = window.__m.longTasks;
      return {
        firstPaintMs: o.paintAt - o.clickAt,
        longTaskTotalMs: lt.reduce((s, t) => s + t.dur, 0),
        longestTaskMs: lt.reduce((m, t) => Math.max(m, t.dur), 0),
        mounted: document.querySelectorAll('section[aria-label="Ask"] [data-message-id]').length,
        domNodes: document.getElementsByTagName('*').length,
      };
    })()`);
    await shot('opened');
    open.settledMs = quietAt - (await cdp.eval(`window.__open.clickAt`));
    const atBottom = await cdp.eval(`(() => { const s = window.__scroller(); return s.scrollHeight - s.scrollTop - s.clientHeight; })()`);
    open.distanceFromBottomPx = atBottom;
    const memoryAt = async () => {
      await cdp.send('HeapProfiler.collectGarbage');
      await sleep(500);
      const metrics = Object.fromEntries((await cdp.send('Performance.getMetrics')).metrics.map(m => [m.name, m.value]));
      const proc = await rendererMemory(browserCdp);
      return { jsHeapUsedMB: round(metrics.JSHeapUsedSize / 2 ** 20), domNodes: metrics.Nodes, rendererWorkingSetMB: proc.workingSetMB, rendererPrivateMB: proc.privateMB };
    };
    await cdp.send('HeapProfiler.enable');
    const memoryOpen = await memoryAt();

    // --- Typing latency in the composer, conversation open.
    await cdp.eval(`(() => {
      const t = document.querySelector('section[aria-label="Ask"] textarea');
      t.focus();
      window.__typing = [];
      t.addEventListener('keydown', e => {
        const ts = e.timeStamp;
        requestAnimationFrame(() => setTimeout(() => window.__typing.push(performance.now() - ts), 0));
      }, { capture: true });
      window.__m.events.length = 0;
      return true;
    })()`);
    const phrase = 'how does product quantization change recall';
    for (const ch of phrase) {
      await cdp.send('Input.dispatchKeyEvent', { type: 'keyDown', key: ch, text: ch, unmodifiedText: ch });
      await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', key: ch });
      await sleep(60);
    }
    await sleep(300);
    const typingRaw = await cdp.eval(`({ lat: window.__typing, ev: window.__m.events.filter(e => e.name === 'keydown' || e.name === 'keypress' || e.name === 'input') })`);
    const typing = {
      p50Ms: round(median(typingRaw.lat)),
      p95Ms: round(pct(typingRaw.lat, 95)),
      maxMs: round(Math.max(...typingRaw.lat)),
      eventTimingOver16: typingRaw.ev.length,
    };
    // Clear the draft without sending.
    await cdp.eval(`(() => { const t = document.querySelector('section[aria-label="Ask"] textarea'); const set = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set; set.call(t, ''); t.dispatchEvent(new Event('input', { bubbles: true })); return true; })()`);
    await sleep(300);

    // --- Scroll frame times: bottom to top and back, 120 px per frame. The
    // first pass also loads lazy chunks (code highlighting, charts); the second
    // is warm.
    const scrollPass = async (stepPx = 120, spanPx = Infinity) => {
      const scroll = await cdp.eval(`(async () => {
        const s = window.__scroller();
        const frames = [];
        window.__m.longTasks.length = 0;
        let last = performance.now();
        const startTop = s.scrollTop;
        const step = async dir => {
          for (let i = 0; i < 6000; i++) {
            const before = s.scrollTop;
            s.scrollTop = before + dir * ${stepPx};
            await new Promise(r => requestAnimationFrame(r));
            const now = performance.now();
            frames.push(now - last);
            last = now;
            if (dir < 0 && ((s.scrollTop <= 0 && before <= 0) || startTop - s.scrollTop >= ${spanPx === Infinity ? 'Infinity' : spanPx})) break;
            if (dir > 0 && s.scrollHeight - s.scrollTop - s.clientHeight < 1 && Math.abs(s.scrollTop - before) < 1) break;
          }
        };
        await step(-1);
        const topMounted = document.querySelectorAll('section[aria-label="Ask"] [data-message-id]').length;
        await step(1);
        const lt = window.__m.longTasks;
        return { frames, topMounted, longTaskTotalMs: lt.reduce((a, t) => a + t.dur, 0), longestTaskMs: lt.reduce((m, t) => Math.max(m, t.dur), 0) };
      })()`);
      const frames = scroll.frames.slice(1);
      await cdp.eval(`window.__quiet(800, 20000)`);
      return {
        frames: frames.length,
        p50Ms: round(median(frames)),
        p95Ms: round(pct(frames, 95)),
        maxMs: round(Math.max(...frames)),
        over33Pct: round((100 * frames.filter(f => f > 33.4).length) / frames.length),
        longTaskTotalMs: round(scroll.longTaskTotalMs),
        longestTaskMs: round(scroll.longestTaskMs),
        mountedAtTop: scroll.topMounted,
      };
    };
    const scrollCold = await scrollPass();
    if (PROFILE) {
      await cdp.send('Profiler.enable');
      await cdp.send('Profiler.setSamplingInterval', { interval: 200 });
      await cdp.send('Profiler.start');
    }
    const scrollWarm = await scrollPass();
    const scrollModerate = await scrollPass(40, 20000);
    if (PROFILE) {
      const { profile } = await cdp.send('Profiler.stop');
      writeFileSync(path.join(PROFILE, `run${run}-scroll-warm.cpuprofile`), JSON.stringify(profile));
    }
    const memoryScrolled = await memoryAt();
    const memory = {
      open: memoryOpen,
      afterScroll: memoryScrolled,
      rendererWorkingSetShortMB: before.workingSetMB,
    };

    // A screenshot with the find bar open and its match highlighted.
    if (SHOTS) {
      await cdp.eval(`window.__beforeFindClose = () => new Promise(r => { window.__findShot = r; }); true`);
    }
    // --- Find in the conversation: a word only in the oldest answer's question.
    const findPromise = cdp.eval(`(async () => {
      const t = document.querySelector('section[aria-label="Ask"] textarea');
      t.focus();
      t.dispatchEvent(new KeyboardEvent('keydown', { key: 'f', ctrlKey: true, bubbles: true, cancelable: true }));
      let input = null;
      for (let i = 0; i < 60 && !input; i++) {
        await new Promise(r => setTimeout(r, 50));
        input = document.querySelector('input[aria-label="Find in conversation"]');
      }
      if (!input) return { opened: false };
      const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
      set.call(input, 'Question 1:');
      input.dispatchEvent(new Event('input', { bubbles: true }));
      for (let i = 0; i < 60; i++) {
        await new Promise(r => setTimeout(r, 50));
        const status = document.querySelector('[role="search"] [role="status"]')?.textContent ?? '';
        if (/of/.test(status) && CSS.highlights?.get('conversation-find-current')) break;
      }
      if (window.__beforeFindClose) await window.__beforeFindClose();
      const status = document.querySelector('[role="search"] [role="status"]')?.textContent ?? '';
      const current = CSS.highlights?.get('conversation-find-current');
      let visible = false;
      if (current) {
        const r = [...current][0].getBoundingClientRect();
        const box = window.__scroller().getBoundingClientRect();
        visible = r.top >= box.top && r.bottom <= box.bottom;
      }
      input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }));
      await new Promise(r => setTimeout(r, 200));
      return { opened: true, status, highlighted: Boolean(current), visible, closed: !document.querySelector('[role="search"]') };
    })()`);
    if (SHOTS) {
      for (let i = 0; i < 100; i++) {
        if (await cdp.eval(`typeof window.__findShot === 'function'`)) break;
        await sleep(50);
      }
      if (await cdp.eval(`typeof window.__findShot === 'function'`)) {
        await sleep(300);
        await shot(`find-${THEME}`);
        await cdp.eval(`window.__findShot(); true`);
      }
    }
    const find = await findPromise;
    // Native find is available when the conversation is not windowed (before): there is no find bar to test.

    // --- Go to message (the gallery's reveal) on an early answer, and a source
    // preview's answer kept mounted while scrolled away.
    const reveal = await cdp.eval(`(async () => {
      window.__perf.reveal('conv-perf-long', 'msg-a-3');
      let el = null;
      for (let i = 0; i < 100; i++) {
        await new Promise(r => setTimeout(r, 50));
        el = document.querySelector('[data-message-id="msg-a-3"]');
        if (el && el.dataset.revealed === 'true') break;
      }
      if (!el) return { mounted: false };
      await new Promise(r => setTimeout(r, 600));
      const box = window.__scroller().getBoundingClientRect();
      const r = el.getBoundingClientRect();
      return { mounted: true, highlighted: el.dataset.revealed === 'true', topInView: r.top >= box.top - 1 && r.top < box.bottom, focused: document.activeElement === el };
    })()`);
    const previewKept = await cdp.eval(`(async () => {
      const s = window.__scroller();
      s.scrollTop = s.scrollHeight;
      await new Promise(r => setTimeout(r, 400));
      const answers = [...document.querySelectorAll('[data-message-id]')];
      const last = answers[answers.length - 1];
      const badge = last && [...last.querySelectorAll('button[aria-label^="Source "]')][0];
      if (!badge) return { opened: false };
      const id = last.dataset.messageId;
      badge.click();
      await new Promise(r => setTimeout(r, 600));
      const dialog = document.querySelector('aside[role="dialog"]');
      s.scrollTop = 0;
      await new Promise(r => setTimeout(r, 600));
      const still = document.querySelector('[data-message-id="' + id + '"]');
      const pressed = Boolean(still && still.querySelector('button[aria-pressed="true"]'));
      document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
      window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
      await new Promise(r => setTimeout(r, 400));
      return { opened: Boolean(dialog), keptMounted: Boolean(still), badgePressed: pressed };
    })()`);

    // --- Streaming: at the bottom, a new answer streams and stays pinned.
    await cdp.eval(`(() => { const s = window.__scroller(); s.scrollTop = s.scrollHeight; return true; })()`);
    await sleep(300);
    const sendText = async text => {
      await cdp.eval(`document.querySelector('section[aria-label="Ask"] textarea').focus()`);
      await cdp.send('Input.insertText', { text });
      await cdp.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13 });
      await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13 });
      for (let i = 0; i < 100; i++) {
        if (await cdp.eval(`window.__perf.sends`) > 0) return;
        await sleep(50);
      }
      throw new Error('The answer did not start');
    };
    await cdp.eval(`window.__perf.sends = 0`);
    await sendText('Summarize the trade-offs');
    const streamPinned = await cdp.eval(`(async () => {
      const s = window.__scroller();
      const frames = [];
      let done = false;
      let maxGap = 0;
      const p = window.__perf.stream(240, 25).then(() => { done = true; });
      let last = performance.now();
      while (!done) {
        await new Promise(r => requestAnimationFrame(r));
        const now = performance.now();
        frames.push(now - last);
        last = now;
        maxGap = Math.max(maxGap, s.scrollHeight - s.scrollTop - s.clientHeight);
      }
      await p;
      await new Promise(r => setTimeout(r, 300));
      return { frames, maxGap, endGap: s.scrollHeight - s.scrollTop - s.clientHeight };
    })()`);
    await cdp.eval(`window.__quiet(800, 20000)`);

    // --- Streaming while the reader has scrolled up: the view must not move.
    await cdp.eval(`window.__perf.sends = 0`);
    await sendText('And the memory costs?');
    const streamUp = await cdp.eval(`(async () => {
      const s = window.__scroller();
      await new Promise(r => setTimeout(r, 200));
      s.scrollTop = Math.max(0, s.scrollTop - 2500);
      await new Promise(r => setTimeout(r, 300));
      // The first answer whose top is on screen is the anchor the reader looks at.
      const box = s.getBoundingClientRect();
      const anchor = [...s.querySelectorAll('[data-message-id]')].find(el => el.getBoundingClientRect().top >= box.top);
      if (!anchor) return { moved: -1 };
      const at = anchor.getBoundingClientRect().top;
      await window.__perf.stream(160, 25);
      await new Promise(r => setTimeout(r, 300));
      return { moved: anchor.isConnected ? Math.abs(anchor.getBoundingClientRect().top - at) : -1 };
    })()`);

    const sf = streamPinned.frames.slice(1);
    const streaming = {
      p50Ms: round(median(sf)),
      p95Ms: round(pct(sf, 95)),
      maxMs: round(Math.max(...sf)),
      maxDistanceFromBottomPx: round(streamPinned.maxGap),
      endDistanceFromBottomPx: round(streamPinned.endGap),
      scrolledUpMovedPx: round(streamUp.moved),
    };
    cdp.close();
    browserCdp.close();
    return {
      run,
      open: { firstPaintMs: round(open.firstPaintMs), settledMs: round(open.settledMs), longTaskTotalMs: round(open.longTaskTotalMs), longestTaskMs: round(open.longestTaskMs), mounted: open.mounted, domNodes: open.domNodes, distanceFromBottomPx: round(open.distanceFromBottomPx) },
      typing,
      scrollCold,
      scrollWarm,
      scrollModerate,
      memory,
      find,
      reveal,
      previewKept,
      streaming,
    };
  } finally {
    browser.kill();
    await sleep(800);
    try {
      rmSync(profileDir, { recursive: true, force: true });
    } catch {
      // Edge may still hold the profile for a moment; it is a temp directory.
    }
  }
}

const results = [];
for (let r = 1; r <= RUNS; r++) {
  const result = await oneRun(r);
  console.log(JSON.stringify(result));
  results.push(result);
}
server.close();

const med = pick => round(median(results.map(pick)));
const summary = {
  turns: TURNS,
  messages: TURNS * 2,
  runs: RUNS,
  openFirstPaintMs: med(r => r.open.firstPaintMs),
  openSettledMs: med(r => r.open.settledMs),
  openLongestTaskMs: med(r => r.open.longestTaskMs),
  answersMountedOnOpen: med(r => r.open.mounted),
  domNodesOnOpen: med(r => r.open.domNodes),
  typingP50Ms: med(r => r.typing.p50Ms),
  typingP95Ms: med(r => r.typing.p95Ms),
  typingMaxMs: med(r => r.typing.maxMs),
  scrollColdP95Ms: med(r => r.scrollCold.p95Ms),
  scrollColdMaxMs: med(r => r.scrollCold.maxMs),
  scrollColdOver33Pct: med(r => r.scrollCold.over33Pct),
  scrollWarmP50Ms: med(r => r.scrollWarm.p50Ms),
  scrollWarmP95Ms: med(r => r.scrollWarm.p95Ms),
  scrollWarmMaxMs: med(r => r.scrollWarm.maxMs),
  scrollWarmOver33Pct: med(r => r.scrollWarm.over33Pct),
  scrollModerateP50Ms: med(r => r.scrollModerate.p50Ms),
  scrollModerateP95Ms: med(r => r.scrollModerate.p95Ms),
  scrollModerateMaxMs: med(r => r.scrollModerate.maxMs),
  scrollModerateOver33Pct: med(r => r.scrollModerate.over33Pct),
  answersMountedAtTop: med(r => r.scrollWarm.mountedAtTop),
  openJsHeapMB: med(r => r.memory.open.jsHeapUsedMB),
  openDomNodes: med(r => r.memory.open.domNodes),
  openRendererWorkingSetMB: med(r => r.memory.open.rendererWorkingSetMB),
  openRendererPrivateMB: med(r => r.memory.open.rendererPrivateMB),
  scrolledJsHeapMB: med(r => r.memory.afterScroll.jsHeapUsedMB),
  scrolledDomNodes: med(r => r.memory.afterScroll.domNodes),
  scrolledRendererWorkingSetMB: med(r => r.memory.afterScroll.rendererWorkingSetMB),
  scrolledRendererPrivateMB: med(r => r.memory.afterScroll.rendererPrivateMB),
  shortChatRendererWorkingSetMB: med(r => r.memory.rendererWorkingSetShortMB),
  streamP95Ms: med(r => r.streaming.p95Ms),
  streamMaxMs: med(r => r.streaming.maxMs),
  streamMaxDistanceFromBottomPx: Math.max(...results.map(r => r.streaming.maxDistanceFromBottomPx)),
  streamEndDistanceFromBottomPx: Math.max(...results.map(r => r.streaming.endDistanceFromBottomPx)),
  streamScrolledUpAnchorMovedPx: Math.max(...results.map(r => r.streaming.scrolledUpMovedPx)),
  find: results.map(r => r.find),
  reveal: results.map(r => r.reveal),
  previewKept: results.map(r => r.previewKept),
};
console.log('SUMMARY ' + JSON.stringify(summary, null, 2));
if (OUT) writeFileSync(OUT, JSON.stringify({ summary, results }, null, 2));
