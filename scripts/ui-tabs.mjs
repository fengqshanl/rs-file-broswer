#!/usr/bin/env node
/**
 * Desktop workspace regression test over Chrome DevTools Protocol.
 *
 * Covers the local-first boot, the add-server panel (the old WKWebView freeze
 * path), per-tab encrypted login, remote previews, cross-tab copy/move, second
 * server removal and reload behaviour. Run it against `dev_app`:
 *
 *   AGENTGOD_DATA_DIR=/tmp/agentgod-tabs-data \
 *     AGENTGOD_LOCAL_ROOT=/tmp/agentgod-tabs-home \
 *     AGENTGOD_MEMORY_SECRETS=1 \
 *     ./desktop/common/target/release/dev_app
 *
 *   FRONT_URL=http://127.0.0.1:<printed-port>/ \
 *   REMOTE_URL=https://127.0.0.1:18443 \
 *   REMOTE_USER=admin REMOTE_PASS=... \
 *   LOCAL_DIR=/tmp/agentgod-tabs-home \
 *   CHROME_DEBUG_URL=http://127.0.0.1:9223 \
 *   node scripts/ui-tabs.mjs
 *
 * Create LOCAL_DIR/本地文件.txt before starting dev_app (the script also
 * creates it when missing), and use a dedicated empty data directory for the
 * remote test server.
 */

import { mkdir, writeFile } from 'node:fs/promises';

const DEBUG_URL = (process.env.CHROME_DEBUG_URL || 'http://127.0.0.1:9223').replace(/\/+$/, '');
const PAGE_URL = process.env.FRONT_URL || '';
const REMOTE_URL = process.env.REMOTE_URL || 'https://127.0.0.1:18443';
const REMOTE_USER = process.env.REMOTE_USER || 'admin';
const REMOTE_PASS = process.env.REMOTE_PASS || '';
const LOCAL_DIR = process.env.LOCAL_DIR || '';

if (!PAGE_URL || !REMOTE_PASS) {
  console.error('FRONT_URL and REMOTE_PASS are required');
  process.exit(2);
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const suffix = Date.now().toString(36);
const serverName = `Tabs ${suffix}`;
const secondServerName = `Tabs2 ${suffix}`;
const remoteFolder = `远程文件夹-${suffix}`;
const remoteChild = `子文件-${suffix}.txt`;
const remoteTop = `远程文件-${suffix}.txt`;
const localFile = `本地文件-${suffix}.txt`;
const localContent = `local-${suffix}\n`;
const remoteChildContent = `remote-child-${suffix}\n`;
const remoteTopContent = `remote-top-${suffix}\n`;

if (LOCAL_DIR) {
  await mkdir(LOCAL_DIR, { recursive: true });
  await writeFile(`${LOCAL_DIR}/${localFile}`, localContent);
}

const target = await (
  await fetch(`${DEBUG_URL}/json/new?${encodeURIComponent(PAGE_URL)}`, { method: 'PUT' })
).json();
const socket = new WebSocket(target.webSocketDebuggerUrl);
const pending = new Map();
const errors = [];
let nextId = 0;

socket.addEventListener('message', (event) => {
  const message = JSON.parse(event.data);
  if (message.id && pending.has(message.id)) {
    const { resolve, reject } = pending.get(message.id);
    pending.delete(message.id);
    if (message.error) reject(new Error(message.error.message));
    else resolve(message.result);
    return;
  }
  if (message.method === 'Runtime.exceptionThrown') {
    const detail = message.params.exceptionDetails;
    errors.push(detail.exception?.description || detail.text || 'page exception');
  }
  if (message.method === 'Log.entryAdded' && message.params.entry.level === 'error') {
    errors.push(`console: ${message.params.entry.text}`);
  }
});

await new Promise((resolve, reject) => {
  socket.addEventListener('open', resolve);
  socket.addEventListener('error', reject);
});

const send = (method, params = {}) =>
  new Promise((resolve, reject) => {
    const id = ++nextId;
    pending.set(id, { resolve, reject });
    socket.send(JSON.stringify({ id, method, params }));
  });

await send('Runtime.enable');
await send('Page.enable');
await send('Log.enable');

const evaluate = async (expression) => {
  const result = await send('Runtime.evaluate', {
    expression,
    awaitPromise: true,
    returnByValue: true,
  });
  if (result.exceptionDetails) {
    throw new Error(
      result.exceptionDetails.exception?.description ||
        result.exceptionDetails.text ||
        'evaluation failed',
    );
  }
  return result.result.value;
};

const waitFor = async (expression, timeout = 20_000, label = expression) => {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (await evaluate(expression)) return;
    await sleep(150);
  }
  throw new Error(`timeout waiting for: ${label}`);
};

const pass = (label) => console.log(`PASS  ${label}`);

const tabExpression = (name) =>
  `[...document.querySelectorAll('.workspace-tab')].find((node) => node.textContent.trim() === ${JSON.stringify(name)})`;

const clickTab = async (name) => {
  const clicked = await evaluate(`(() => {
    const tab = ${tabExpression(name)};
    if (!tab) return false;
    tab.click();
    return true;
  })()`);
  if (!clicked) throw new Error(`tab not found: ${name}`);
  await sleep(200);
};

const rowAction = async (name, title) => {
  return evaluate(`(() => {
    const row = [...document.querySelectorAll('#file-list .file-row')].find((node) => {
      const label = node.querySelector('.name-btn, .file-name');
      return label && label.textContent === ${JSON.stringify(name)};
    });
    if (!row) return false;
    row.dispatchEvent(new MouseEvent('contextmenu', {
      bubbles: true,
      cancelable: true,
      clientX: 60,
      clientY: 90,
    }));
    const item = [...document.querySelectorAll('#context-menu .context-item')].find(
      (node) => node.textContent.includes(${JSON.stringify(title)}),
    );
    if (!item) return false;
    item.click();
    return true;
  })()`);
};

const waitRow = (name, timeout = 20_000) =>
  waitFor(
    `[...document.querySelectorAll('#file-list .name-btn, #file-list .file-name')].some((node) => node.textContent === ${JSON.stringify(name)})`,
    timeout,
    `row ${name}`,
  );

const downloadText = (path) =>
  evaluate(`(async () => {
    const download = await openEncryptedDownload(activeTab(), ${JSON.stringify(path)});
    let text = '';
    await readDownloadFrames(download, (chunk) => {
      text += new TextDecoder().decode(chunk);
    });
    return text;
  })()`);

try {
  await waitFor(
    "document.readyState === 'complete' && !document.querySelector('#workspace-bar').hidden",
    20_000,
    'workspace boot',
  );
  const boot = await evaluate(`({
    tabs: [...document.querySelectorAll('.workspace-tab-label')].map((node) => node.textContent),
    appHidden: document.querySelector('#app-view').hidden,
    addVisible: !!document.querySelector('#btn-add-server') && !document.querySelector('#btn-add-server').hidden,
  })`);
  if (boot.tabs.length !== 1 || boot.tabs[0] !== '本机' || boot.appHidden || !boot.addVisible) {
    throw new Error(`unexpected local-first boot: ${JSON.stringify(boot)}`);
  }
  await waitRow(localFile, 15_000);
  pass('local-first boot and local listing');

  await evaluate("document.querySelector('#btn-add-server').click(); true");
  await waitFor(
    "!document.querySelector('#server-overlay').hidden && !!document.querySelector('#server-panel .server-form')",
    5_000,
    'add-server panel',
  );
  pass('add-server panel opens without blocking');

  await evaluate(`(() => {
    const inputs = [...document.querySelectorAll('#server-panel input[type=text]')];
    inputs[0].value = ${JSON.stringify(serverName)};
    inputs[1].value = ${JSON.stringify(REMOTE_URL)};
    const box = document.querySelector('#server-panel input[type=checkbox]');
    if (box) box.checked = true;
    [...document.querySelectorAll('#server-panel .server-form button')]
      .find((button) => button.textContent === '添加服务器')
      .click();
    return true;
  })()`);
  await waitFor(
    `[...document.querySelectorAll('.workspace-tab-label')].some((node) => node.textContent === ${JSON.stringify(serverName)})`,
    10_000,
    'remote tab',
  );
  await waitFor("!document.querySelector('#login-view').hidden", 10_000, 'remote login');
  await evaluate(`(() => {
    document.querySelector('#username').value = ${JSON.stringify(REMOTE_USER)};
    document.querySelector('#password').value = ${JSON.stringify(REMOTE_PASS)};
    document.querySelector('#login-btn').click();
    return true;
  })()`);
  await waitFor(
    "!document.querySelector('#app-view').hidden && document.querySelector('#login-view').hidden",
    20_000,
    'remote login finish',
  );
  pass('add server, create tab and encrypted login');

  await evaluate(`(async () => {
    await api('/api/mkdir', { path: ${JSON.stringify(`/${remoteFolder}`)} });
    enqueueUploads(
      [{ file: new File([${JSON.stringify(remoteChildContent)}], ${JSON.stringify(remoteChild)}), relative: ${JSON.stringify(remoteChild)} }],
      ${JSON.stringify(`/${remoteFolder}`)},
    );
    enqueueUploads(
      [{ file: new File([${JSON.stringify(remoteTopContent)}], ${JSON.stringify(remoteTop)}), relative: ${JSON.stringify(remoteTop)} }],
      '/',
    );
    return true;
  })()`);
  await waitRow(remoteFolder, 15_000);
  await waitRow(remoteTop, 15_000);
  pass('remote fixture upload');

  await evaluate(`(() => {
    const row = [...document.querySelectorAll('#file-list .file-row')].find((node) => {
      const label = node.querySelector('.name-btn, .file-name');
      return label && label.textContent === ${JSON.stringify(remoteFolder)};
    });
    row.querySelector('.name-btn').click();
    return true;
  })()`);
  await waitRow(remoteChild, 15_000);
  await evaluate(`openPreview({ name: ${JSON.stringify(remoteChild)}, size: 20, modified: Date.now(), is_dir: false }); true`);
  await waitFor(
    `document.querySelector('.preview-text')?.textContent.includes(${JSON.stringify(remoteChildContent.trim())})`,
    15_000,
    'remote encrypted preview',
  );
  await evaluate("document.querySelector('#modal').close(); true");
  pass('remote encrypted preview');

  await clickTab('本机');
  await evaluate("loadDir('/', { tab: activeTab() }); true");
  await waitRow(localFile, 10_000);
  if (!(await rowAction(localFile, '复制'))) throw new Error('local copy button missing');
  await clickTab(serverName);
  await waitFor(
    `document.querySelector('.crumbs').textContent.includes(${JSON.stringify(remoteFolder)})`,
    10_000,
    'remote folder still active',
  );
  await evaluate("document.querySelector('#btn-paste').click(); true");
  await waitRow(localFile, 25_000);
  const remoteText = await downloadText(`/${remoteFolder}/${localFile}`);
  if (!remoteText.includes(localContent.trim())) {
    throw new Error(`remote copy content mismatch: ${remoteText}`);
  }
  pass('copy local file to remote, content verified');

  await clickTab(serverName);
  await evaluate("loadDir('/', { tab: activeTab() }); true");
  await waitRow(remoteFolder, 15_000);
  if (!(await rowAction(remoteFolder, '复制'))) throw new Error('remote folder copy button missing');
  await clickTab('本机');
  await evaluate("document.querySelector('#btn-paste').click(); true");
  await waitRow(remoteFolder, 30_000);
  const copiedChild = await downloadText(`/${remoteFolder}/${remoteChild}`);
  if (!copiedChild.includes(remoteChildContent.trim())) {
    throw new Error(`recursive folder copy content mismatch: ${copiedChild}`);
  }
  pass('copy remote folder to local recursively');

  await clickTab(serverName);
  await evaluate("loadDir('/', { tab: activeTab() }); true");
  await waitRow(remoteTop, 15_000);
  if (!(await rowAction(remoteTop, '剪切'))) throw new Error('remote cut button missing');
  await clickTab('本机');
  await evaluate("document.querySelector('#btn-paste').click(); true");
  await waitRow(remoteTop, 25_000);
  const movedText = await downloadText(`/${remoteTop}`);
  if (!movedText.includes(remoteTopContent.trim())) {
    throw new Error(`moved file content mismatch: ${movedText}`);
  }
  const sourceStillThere = await evaluate(`(async () => {
    const tab = [...state.tabs.values()].find((item) => item.target.name === ${JSON.stringify(serverName)});
    const listing = await api('/api/list', { path: '/' }, 'POST', tab);
    return listing.entries.some((entry) => entry.name === ${JSON.stringify(remoteTop)});
  })()`);
  if (sourceStillThere) throw new Error('cut did not delete the remote source');
  pass('cut remote file to local, source removed');

  await evaluate("document.querySelector('#btn-add-server').click(); true");
  await waitFor("!document.querySelector('#server-overlay').hidden", 5_000, 'panel reopen');
  await evaluate(`(() => {
    const inputs = [...document.querySelectorAll('#server-panel input[type=text]')];
    inputs[0].value = ${JSON.stringify(secondServerName)};
    inputs[1].value = ${JSON.stringify(REMOTE_URL)};
    document.querySelector('#server-panel input[type=checkbox]').checked = true;
    [...document.querySelectorAll('#server-panel .server-form button')]
      .find((button) => button.textContent === '添加服务器')
      .click();
    return true;
  })()`);
  await waitFor(
    `[...document.querySelectorAll('.workspace-tab-label')].some((node) => node.textContent === ${JSON.stringify(secondServerName)})`,
    10_000,
    'second server tab',
  );
  pass('second server tab added');

  await evaluate("document.querySelector('#btn-add-server').click(); true");
  await waitFor(
    `[...document.querySelectorAll('#server-panel .server-row')].some((row) => row.textContent.includes(${JSON.stringify(secondServerName)}))`,
    5_000,
    'server list',
  );
  await evaluate(`(() => {
    const row = [...document.querySelectorAll('#server-panel .server-row')]
      .find((node) => node.textContent.includes(${JSON.stringify(secondServerName)}));
    [...row.querySelectorAll('button')].find((button) => button.textContent === '删除').click();
    return true;
  })()`);
  await waitFor(
    `![...document.querySelectorAll('.workspace-tab-label')].some((node) => node.textContent === ${JSON.stringify(secondServerName)})`,
    10_000,
    'second server tab removed',
  );
  pass('remove server closes tab without reload');

  await send('Page.reload');
  await waitFor(
    "document.readyState === 'complete' && !document.querySelector('#workspace-bar').hidden",
    20_000,
    'reload workspace',
  );
  await waitRow(localFile, 20_000);
  const afterReload = await evaluate(`({
    tabs: [...document.querySelectorAll('.workspace-tab-label')].map((node) => node.textContent),
    active: document.querySelector('.workspace-tab.active')?.textContent.trim(),
  })`);
  if (
    !afterReload.tabs.includes('本机') ||
    !afterReload.tabs.includes(serverName) ||
    afterReload.active !== '本机'
  ) {
    throw new Error(`bad reload state: ${JSON.stringify(afterReload)}`);
  }
  await clickTab(serverName);
  await waitFor("!document.querySelector('#login-view').hidden", 10_000, 'login form after reload');
  pass('reload restores local tab and locked remote tab');

  const relevantErrors = errors.filter(
    (message) => message && !/favicon|Failed to load resource/i.test(message),
  );
  if (relevantErrors.length) throw new Error(`page errors: ${relevantErrors.join(' | ')}`);
  pass('no JS exceptions');

  console.log('\nDESKTOP TABS TEST PASSED');
} finally {
  socket.close();
  await fetch(`${DEBUG_URL}/json/close/${target.id}`).catch(() => {});
}
