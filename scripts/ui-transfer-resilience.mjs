#!/usr/bin/env node
/**
 * Transfer-resilience regression test over Chrome DevTools Protocol.
 *
 * Reproduces the failure where one stalled upload wedged the whole queue: the
 * next file the user picked produced no progress row and no error at all.
 * Asserts that:
 *   1. a newly queued file shows feedback immediately, even while an earlier
 *      transfer is stuck;
 *   2. failing the stuck transfer lets the queue continue;
 *   3. cancelling an in-flight upload marks it cancelled and the queue still
 *      moves on to the next file.
 *
 * Usage:
 *   CHROME_DEBUG_URL=http://127.0.0.1:9223 UI_URL=https://127.0.0.1:8090/ \
 *   ADMIN_PASSWORD=... node scripts/ui-transfer-resilience.mjs
 */

const DEBUG_URL = (process.env.CHROME_DEBUG_URL || 'http://127.0.0.1:9223').replace(/\/+$/, '');
const PAGE_URL = process.env.UI_URL || 'https://127.0.0.1:18443/';
const ADMIN_USER = process.env.ADMIN_USER || 'admin';
const ADMIN_PASSWORD = process.env.ADMIN_PASSWORD || '';

if (!ADMIN_PASSWORD) {
  console.error('ADMIN_PASSWORD is required');
  process.exit(2);
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const targets = await (await fetch(`${DEBUG_URL}/json/list`)).json();
const page = targets.find((target) => target.type === 'page');
if (!page) throw new Error('no page target in Chrome');

const ws = new WebSocket(page.webSocketDebuggerUrl);
let nextId = 1;
const pending = new Map();
const events = [];
ws.addEventListener('message', (event) => {
  const message = JSON.parse(event.data);
  if (message.id && pending.has(message.id)) {
    const { resolve, reject } = pending.get(message.id);
    pending.delete(message.id);
    message.error ? reject(new Error(JSON.stringify(message.error))) : resolve(message.result);
    return;
  }
  events.push(message);
});
await new Promise((resolve, reject) => {
  ws.addEventListener('open', resolve, { once: true });
  ws.addEventListener('error', reject, { once: true });
});

function send(method, params = {}) {
  const id = nextId++;
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    ws.send(JSON.stringify({ id, method, params }));
  });
}

// The interceptor holds exactly one chunk request (the simulated dead
// connection) and immediately continues everything else, so the recovery phase
// is never blocked by the test harness itself.
let holdNextChunk = false;
let heldRequestId = null;
ws.addEventListener('message', (event) => {
  const message = JSON.parse(event.data);
  if (message.method !== 'Fetch.requestPaused') return;
  const isChunk = message.params.request.url.includes('/api/upload/chunk/');
  if (holdNextChunk && !heldRequestId && isChunk) {
    heldRequestId = message.params.requestId;
    return;
  }
  void send('Fetch.continueRequest', { requestId: message.params.requestId }).catch(() => {});
});

async function evaluate(expression) {
  const result = await send('Runtime.evaluate', {
    expression,
    awaitPromise: true,
    returnByValue: true,
  });
  if (result.exceptionDetails) {
    throw new Error(result.exceptionDetails.exception?.description || result.exceptionDetails.text);
  }
  return result.result.value;
}

async function waitFor(expression, timeoutMs, label) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await evaluate(expression)) return;
    await sleep(150);
  }
  throw new Error(`timeout waiting for: ${label}`);
}

async function waitForHeldChunk(timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (heldRequestId) {
      const paused = events.find(
        (event) =>
          event.method === 'Fetch.requestPaused' &&
          event.params.requestId === heldRequestId,
      );
      if (paused) return paused.params;
    }
    await sleep(100);
  }
  throw new Error('no upload chunk request was intercepted');
}

const RUN = Date.now();
const STALLED = `stalled-a-${RUN}.bin`;
const AFTER_STALL = `after-stall-${RUN}.txt`;
const CANCEL = `cancel-me-${RUN}.bin`;
const AFTER_CANCEL = `after-cancel-${RUN}.txt`;

const itemText = (name) =>
  `([...document.querySelectorAll('.upload-item')].find((node) => node.textContent.includes(${JSON.stringify(name)}))?.textContent || '')`;

await send('Runtime.enable');
await send('Page.enable');
await send('Page.navigate', { url: PAGE_URL });
await waitFor("document.readyState === 'complete' && !!document.querySelector('#login-form')", 15_000, 'login page');
await evaluate(`(() => {
  document.querySelector('#username').value = ${JSON.stringify(ADMIN_USER)};
  document.querySelector('#password').value = ${JSON.stringify(ADMIN_PASSWORD)};
  document.querySelector('#login-btn').click();
  return true;
})()`);
await waitFor("!document.querySelector('#app-view').hidden", 15_000, 'login');
console.log('PASS  UI login');

await send('Fetch.enable', {
  patterns: [{ urlPattern: '*api/upload/chunk*', requestStage: 'Request' }],
});

/* ---------- 1. a stalled transfer must not hide later selections ---------- */
holdNextChunk = true;
await evaluate(`(() => {
  const data = new Uint8Array(10 * 1024 * 1024);
  crypto.getRandomValues(data.subarray(0, 65536));
  enqueueUploads([new File([data], ${JSON.stringify(STALLED)})], '/');
  return true;
})()`);
const stalled = await waitForHeldChunk(15_000);
console.log('PASS  first chunk intercepted (simulated dead connection)');

await evaluate(`(() => {
  enqueueUploads([new File(['queued while busy'], ${JSON.stringify(AFTER_STALL)})], '/');
  return true;
})()`);
await waitFor(`(${itemText(AFTER_STALL)}).includes('等待上传')`, 5_000, 'queued file row');
console.log('PASS  queued file shows feedback while an earlier upload is stuck');

// Release the stuck chunk; the first upload fails and the queue continues.
heldRequestId = null;
holdNextChunk = false;
await send('Fetch.failRequest', { requestId: stalled.requestId, errorReason: 'Failed' });
await waitFor(`(${itemText(STALLED)}).includes('失败')`, 20_000, 'stalled upload failure');
console.log('PASS  stalled upload fails with a visible error');
await waitFor(`(${itemText(AFTER_STALL)}).includes('已完成')`, 30_000, 'next upload completes');
console.log('PASS  queue recovers and uploads the next file');

/* ---------- 2. cancel an in-flight upload, queue keeps moving ---------- */
holdNextChunk = true;
await evaluate(`(() => {
  const data = new Uint8Array(10 * 1024 * 1024);
  enqueueUploads([new File([data], ${JSON.stringify(CANCEL)})], '/');
  return true;
})()`);
const cancelTarget = await waitForHeldChunk(15_000);
await evaluate(`(() => {
  const item = [...document.querySelectorAll('.upload-item')].find((node) => node.textContent.includes(${JSON.stringify(CANCEL)}));
  item.querySelector('.upload-cancel').click();
  return true;
})()`);
heldRequestId = null;
holdNextChunk = false;
try {
  await send('Fetch.failRequest', { requestId: cancelTarget.requestId, errorReason: 'Aborted' });
} catch {
  /* the browser may have already dropped the aborted request */
}
await waitFor(`(${itemText(CANCEL)}).includes('已取消')`, 10_000, 'cancelled status');
console.log('PASS  cancelling an in-flight upload updates the row');

await evaluate(`(() => {
  enqueueUploads([new File(['after cancel'], ${JSON.stringify(AFTER_CANCEL)})], '/');
  return true;
})()`);
await waitFor(`(${itemText(AFTER_CANCEL)}).includes('已完成')`, 30_000, 'upload after cancel');
console.log('PASS  queue keeps moving after a cancel');

const exceptions = events
  .filter((event) => event.method === 'Runtime.exceptionThrown')
  .map((event) => event.params.exceptionDetails.exception?.description || event.params.exceptionDetails.text);
if (exceptions.length) throw new Error(`page exceptions: ${exceptions.join(' | ')}`);
console.log('PASS  no page exceptions');

await send('Fetch.disable');
ws.close();
console.log('\nTRANSFER RESILIENCE TEST PASSED');
