#!/usr/bin/env node
/**
 * Desktop command-bar / file-list regression test over Chrome DevTools
 * Protocol. Covers the top command bar, hidden-file toggle, folder sizes,
 * right-click menu, multi-select and clipboard shortcuts.
 *
 * Usage (against dev_app with the fixture files described below):
 *   FRONT_URL=http://127.0.0.1:<port>/ \
 *   CHROME_DEBUG_URL=http://127.0.0.1:9223 \
 *   node scripts/ui-features-smoke.mjs
 *
 * Expected local fixture:
 *   普通文件.txt (6 bytes), 另一个文件.txt, .隐藏文件.txt
 *   资料夹/里面.txt (6 bytes)
 */

const DEBUG_URL = (process.env.CHROME_DEBUG_URL || 'http://127.0.0.1:9223').replace(/\/+$/, '');
const PAGE_URL = process.env.FRONT_URL || '';
const VISIBLE_FILE = process.env.VISIBLE_FILE || '普通文件.txt';
const SECOND_FILE = process.env.SECOND_FILE || '另一个文件.txt';
const HIDDEN_FILE = process.env.HIDDEN_FILE || '.隐藏文件.txt';
const FOLDER = process.env.FOLDER || '资料夹';
const FOLDER_SIZE = process.env.FOLDER_SIZE || '6 B';

if (!PAGE_URL) {
  console.error('FRONT_URL is required');
  process.exit(2);
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

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

const waitFor = async (expression, timeout = 15_000, label = 'condition') => {
  const started = Date.now();
  while (Date.now() - started < timeout) {
    if (await evaluate(expression)) return;
    await sleep(120);
  }
  throw new Error(`timed out waiting for ${label}`);
};

const pass = (label) => console.log(`PASS  ${label}`);

const rowExpression = (name) =>
  `[...document.querySelectorAll('#file-list .file-row')].find((row) => {
     const label = row.querySelector('.name-btn, .file-name');
     return label && label.textContent === ${JSON.stringify(name)};
   })`;

const waitRow = (name, timeout = 15_000) =>
  waitFor(`!!(${rowExpression(name)})`, timeout, `row ${name}`);

try {
  await waitFor(
    "document.readyState === 'complete' && !document.querySelector('#workspace-bar').hidden",
    20_000,
    'workspace boot',
  );

  const chrome = await evaluate(`({
    commandBarInTop:
      !!document.querySelector('#workspace-bar #btn-add-server') &&
      !!document.querySelector('#workspace-bar #btn-check-update') &&
      !!document.querySelector('#workspace-bar #root-select') &&
      !!document.querySelector('#workspace-bar #app-version'),
    appBarGone: !document.querySelector('#app-bar'),
    addVisible: !!document.querySelector('#btn-add-server'),
    updateVisible: !!document.querySelector('#btn-check-update'),
    version: document.querySelector('#app-version').textContent,
    columns: [...document.querySelectorAll('.list-head > span')].map((node) => node.textContent),
  })`);
  if (
    !chrome.commandBarInTop ||
    !chrome.appBarGone ||
    !chrome.addVisible ||
    !chrome.updateVisible ||
    !/^v\d/.test(chrome.version)
  ) {
    throw new Error(`unexpected command bar: ${JSON.stringify(chrome)}`);
  }
  if (chrome.columns.length !== 3 || chrome.columns.includes('操作')) {
    throw new Error(`actions column still rendered: ${JSON.stringify(chrome.columns)}`);
  }
  pass('top command bar owns add-server, update check, root select and version');

  await waitRow(VISIBLE_FILE);
  const hiddenBefore = await evaluate(`!!(${rowExpression(HIDDEN_FILE)})`);
  if (hiddenBefore) throw new Error('hidden file shown by default');
  pass('hidden files are hidden by default');

  await waitFor(
    `(() => {
      const row = ${rowExpression(FOLDER)};
      const size = row && row.querySelector('.size');
      return !!size && size.textContent !== '—';
    })()`,
    20_000,
    'folder size',
  );
  const folderSize = await evaluate(
    `(${rowExpression(FOLDER)}).querySelector('.size').textContent`,
  );
  if (folderSize !== FOLDER_SIZE) {
    throw new Error(`unexpected folder size: ${folderSize}`);
  }
  pass(`folder size computed (${folderSize})`);

  await evaluate("document.querySelector('#toggle-hidden').click(); true");
  await waitRow(HIDDEN_FILE);
  const hiddenMarked = await evaluate(
    `(${rowExpression(HIDDEN_FILE)}).classList.contains('hidden-entry')`,
  );
  if (!hiddenMarked) throw new Error('hidden row lacks hidden-entry class');
  await evaluate("document.querySelector('#toggle-hidden').click(); true");
  await waitFor(
    `!(${rowExpression(HIDDEN_FILE)})`,
    10_000,
    'hidden file hidden again',
  );
  pass('hidden-file checkbox toggles the listing');

  const menu = await evaluate(`(() => {
    const row = ${rowExpression(VISIBLE_FILE)};
    row.dispatchEvent(new MouseEvent('contextmenu', {
      bubbles: true,
      cancelable: true,
      clientX: 60,
      clientY: 90,
    }));
    const menu = document.querySelector('#context-menu');
    return {
      visible: !menu.hidden,
      items: [...menu.querySelectorAll('.context-item')].map((node) => node.textContent),
    };
  })()`);
  for (const label of ['打开', '下载', '复制', '剪切', '重命名', '删除']) {
    if (!menu.visible || !menu.items.some((item) => item.includes(label))) {
      throw new Error(`context menu missing ${label}: ${JSON.stringify(menu)}`);
    }
  }
  await evaluate('document.body.click(); true');
  await evaluate('clearSelection(activeTab()); true');
  pass('right-click menu exposes file operations');

  const selected = await evaluate(`(() => {
    const rows = [...document.querySelectorAll('#file-list .file-row')]
      .filter((row) => row.querySelector('.file-name'));
    for (const row of rows.slice(0, 2)) {
      row.dispatchEvent(new MouseEvent('click', {
        bubbles: true,
        cancelable: true,
        ctrlKey: true,
      }));
    }
    return document.querySelectorAll('#file-list .file-row.selected').length;
  })()`);
  if (selected !== 2) throw new Error(`Ctrl-click selected ${selected} rows`);
  pass('Ctrl-click multi-select');

  await evaluate(
    "document.dispatchEvent(new KeyboardEvent('keydown', { key: 'c', ctrlKey: true, bubbles: true })); true",
  );
  const copied = await evaluate('state.clipboard && state.clipboard.items.length');
  if (copied !== 2) throw new Error(`clipboard holds ${copied} items`);
  pass('Ctrl+C copies the whole selection');

  await evaluate(
    "document.dispatchEvent(new KeyboardEvent('keydown', { key: 'v', ctrlKey: true, bubbles: true })); true",
  );
  await waitFor(
    `document.querySelectorAll('#file-list .file-row').length >= 5`,
    25_000,
    'multi-paste copies',
  );
  pass('Ctrl+V pastes multiple items');

  const updateState = await evaluate(`(() => {
    window.__agentgodUpdateRequest = updateRequest;
    updateRequest = async () => {
      await new Promise((resolve) => setTimeout(resolve, 500));
      return { current_version: '0.0.0' };
    };
    document.querySelector('#btn-check-update').click();
    return {
      disabled: document.querySelector('#btn-check-update').disabled,
      label: document.querySelector('#update-check-label').textContent,
      spinner: !document.querySelector('#update-check-spinner').hidden,
    };
  })()`);
  if (!updateState.disabled || !updateState.spinner || updateState.label !== '检查中…') {
    throw new Error(`update button lacks loading state: ${JSON.stringify(updateState)}`);
  }
  await waitFor(
    "document.querySelector('#update-check-label').textContent === '检查更新'",
    10_000,
    'update button reset',
  );
  await evaluate('updateRequest = window.__agentgodUpdateRequest; true');
  pass('update check shows a spinner and disables the button');

  await waitFor("document.querySelectorAll('#file-list .file-row.selected').length === 0", 5_000, 'paste clears selection');
  if (errors.length) {
    throw new Error(`page errors: ${errors.join(' | ')}`);
  }
  console.log('ALL FEATURE CHECKS PASSED');
} finally {
  socket.close();
}
