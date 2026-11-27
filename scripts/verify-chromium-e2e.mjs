#!/usr/bin/env node
/**
 * Zero-dependency real Chromium (CDP) E2E verifier for zerho.
 * Verifies that a live zerho dev or zerho preview URL renders a React 19 + Zustand + Lucide app
 * with 0 uncaught JS exceptions, renders the expected SVG + button, and updates state on click.
 *
 * Usage:
 *   node scripts/verify-chromium-e2e.mjs <url>
 * Optional env:
 *   CHROME_BIN=/usr/bin/google-chrome
 */
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const targetUrl = process.argv[2];
if (!targetUrl) {
  console.error('Usage: node scripts/verify-chromium-e2e.mjs <url>');
  process.exit(2);
}

function findChrome() {
  if (process.env.CHROME_BIN && fs.existsSync(process.env.CHROME_BIN)) {
    return process.env.CHROME_BIN;
  }
  const candidates =
    process.platform === 'win32'
      ? [
          'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
          'C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe',
          'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe',
        ]
      : process.platform === 'darwin'
        ? ['/Applications/Google Chrome.app/Contents/MacOS/Google Chrome']
        : ['/usr/bin/google-chrome', '/usr/bin/chromium-browser', '/usr/bin/chromium'];

  for (const bin of candidates) {
    if (fs.existsSync(bin)) return bin;
  }
  throw new Error(`No Chrome/Chromium binary found (checked: ${candidates.join(', ')})`);
}

const chromePath = findChrome();
const profileDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zerho-cdp-'));
const port = 9220 + Math.floor(Math.random() * 500);

const chrome = spawn(
  chromePath,
  [
    '--headless=new',
    `--remote-debugging-port=${port}`,
    `--user-data-dir=${profileDir}`,
    '--disable-gpu',
    '--no-sandbox',
    '--no-first-run',
    '--no-default-browser-check',
    'about:blank',
  ],
  { stdio: 'ignore' },
);

async function waitForTargets() {
  for (let i = 0; i < 60; i++) {
    try {
      const res = await fetch(`http://127.0.0.1:${port}/json/list`);
      const list = await res.json();
      const page = list.find((t) => t.type === 'page' && t.webSocketDebuggerUrl);
      if (page) return page.webSocketDebuggerUrl;
    } catch {
      // retry until CDP port is listening
    }
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error('Timed out waiting for Chrome DevTools Protocol');
}

try {
  const wsUrl = await waitForTargets();
  const ws = new WebSocket(wsUrl);
  await new Promise((res, rej) => {
    ws.addEventListener('open', res, { once: true });
    ws.addEventListener('error', rej, { once: true });
  });

  let nextId = 1;
  const pending = new Map();
  const exceptions = [];
  const failedAssets = [];

  ws.addEventListener('message', (ev) => {
    const msg = JSON.parse(ev.data);
    if (msg.id && pending.has(msg.id)) {
      const { res, rej } = pending.get(msg.id);
      pending.delete(msg.id);
      if (msg.error) rej(new Error(msg.error.message));
      else res(msg.result);
    } else if (msg.method === 'Runtime.exceptionThrown') {
      const d = msg.params.exceptionDetails;
      exceptions.push({
        text: d.text,
        description: d.exception?.description || d.exception?.value || '',
      });
    } else if (msg.method === 'Network.responseReceived') {
      const { status, url } = msg.params.response;
      if (status >= 400 && !url.endsWith('/favicon.ico')) {
        failedAssets.push({ status, url });
      }
    }
  });

  const send = (method, params = {}) =>
    new Promise((res, rej) => {
      const id = nextId++;
      pending.set(id, { res, rej });
      ws.send(JSON.stringify({ id, method, params }));
    });

  await send('Page.enable');
  await send('Runtime.enable');
  await send('Network.enable');
  await send('Network.setCacheDisabled', { cacheDisabled: true });
  await send('Page.navigate', { url: targetUrl });

  // Wait up to 8s for React root to render
  let before = '';
  for (let i = 0; i < 40; i++) {
    await new Promise((r) => setTimeout(r, 200));
    const evalRes = await send('Runtime.evaluate', {
      expression: `document.getElementById('root')?.innerHTML ?? ''`,
      returnByValue: true,
    });
    before = evalRes.result?.value ?? '';
    if (before.includes('Count: 1')) break;
  }

  const clickRes = await send('Runtime.evaluate', {
    expression: `(() => { const b = document.querySelector('#root button'); if (!b) return 'no-button'; b.click(); return 'clicked'; })()`,
    returnByValue: true,
  });
  await new Promise((r) => setTimeout(r, 300));
  const afterRes = await send('Runtime.evaluate', {
    expression: `document.getElementById('root')?.innerHTML ?? ''`,
    returnByValue: true,
  });
  const after = afterRes.result?.value ?? '';

  ws.close();

  const ok =
    exceptions.length === 0 &&
    failedAssets.length === 0 &&
    before.includes('<svg') &&
    before.includes('Count: 1') &&
    clickRes.result?.value === 'clicked' &&
    after.includes('Count: 2');

  console.log(
    JSON.stringify(
      {
        url: targetUrl,
        ok,
        beforeLen: before.length,
        clicked: clickRes.result?.value,
        hasCount2: after.includes('Count: 2'),
        exceptions,
        failedAssets,
      },
      null,
      2,
    ),
  );

  if (!ok) {
    process.exitCode = 1;
  }
} finally {
  chrome.kill();
  try {
    fs.rmSync(profileDir, { recursive: true, force: true });
  } catch {
    // Windows Chrome may briefly hold a file lock on the temp profile directory.
  }
}
