#!/usr/bin/env node
/**
 * Desktop SSH-target UI smoke test over Chrome DevTools Protocol.
 *
 * The SSH host is deliberately unreachable (127.0.0.1:1), so this verifies the
 * target form, tab lifecycle, error surface, edit and delete paths without
 * needing a real SSH server. Run against `dev_app` with a fresh data dir:
 *
 *   FRONT_URL=http://127.0.0.1:<port>/ \
 *   CHROME_DEBUG_URL=http://127.0.0.1:9223 \
 *   node scripts/ssh-ui-smoke.mjs
 */

const DEBUG_URL = (process.env.CHROME_DEBUG_URL || 'http://127.0.0.1:9223').replace(/\/+$/, '');
const PAGE_URL = process.env.FRONT_URL || '';
const SSH_HOST = process.env.SSH_HOST || '127.0.0.1';
const SSH_PORT = process.env.SSH_PORT || '1';
const SSH_USER = process.env.SSH_USER || 'tester';
const SSH_PASSWORD = process.env.SSH_PASSWORD || 'secret';
const SSH_NAME = process.env.SSH_NAME || `SSH ${Date.now().toString(36)}`;

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

const waitFor = async (expression, timeout = 20_000, label = expression) => {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (await evaluate(expression)) return;
    await sleep(150);
  }
  throw new Error(`timeout waiting for: ${label}`);
};

const pass = (label) => console.log(`PASS  ${label}`);
const tabExists = (name) =>
  `[...document.querySelectorAll('.workspace-tab-label')].some((node) => node.textContent === ${JSON.stringify(name)})`;

const setField = (label, value) =>
  evaluate(`(() => {
    const field = [...document.querySelectorAll('#server-panel .field')]
      .find((node) => node.querySelector('span')?.textContent === ${JSON.stringify(label)});
    if (!field) return false;
    const input = field.querySelector('input, select');
    if (!input) return false;
    input.value = ${JSON.stringify(value)};
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.dispatchEvent(new Event('change', { bubbles: true }));
    return true;
  })()`);

const openPanel = () =>
  evaluate("document.querySelector('#btn-add-server').click(); true");

const clickButton = (text) =>
  evaluate(`(() => {
    const button = [...document.querySelectorAll('#server-panel button')]
      .find((node) => node.textContent.trim() === ${JSON.stringify(text)});
    if (!button) return false;
    button.click();
    return true;
  })()`);

const rowFor = (name) =>
  `[...document.querySelectorAll('#server-panel .server-row')]
     .find((row) => row.textContent.includes(${JSON.stringify(name)}))`;

try {
  await waitFor(
    "document.readyState === 'complete' && !document.querySelector('#workspace-bar').hidden",
    20_000,
    'workspace boot',
  );
  const initialTabs = await evaluate(
    "[...document.querySelectorAll('.workspace-tab-label')].map((node) => node.textContent)",
  );
  if (initialTabs.length !== 1 || initialTabs[0] !== '本机') {
    throw new Error(`SSH smoke needs a fresh profile, got ${JSON.stringify(initialTabs)}`);
  }

  await openPanel();
  await waitFor("!document.querySelector('#server-overlay').hidden", 5_000, 'panel');
  const switched = await evaluate(`(() => {
    const select = [...document.querySelectorAll('#server-panel select')]
      .find((node) => [...node.options].some((option) => option.value === 'ssh'));
    if (!select) return false;
    select.value = 'ssh';
    select.dispatchEvent(new Event('change', { bubbles: true }));
    return true;
  })()`);
  if (!switched) throw new Error('SSH type selector missing');
  pass('server panel exposes an SSH type');

  for (const [label, value] of [
    ['名称', SSH_NAME],
    ['主机', SSH_HOST],
    ['端口', SSH_PORT],
    ['用户名', SSH_USER],
    ['密码', SSH_PASSWORD],
  ]) {
    if (!(await setField(label, value))) throw new Error(`missing field ${label}`);
  }
  if (!(await clickButton('添加 SSH 连接'))) throw new Error('add button missing');
  await waitFor(tabExists(SSH_NAME), 15_000, 'SSH tab');
  pass('add SSH target creates a tab');

  await waitFor(
    `!document.querySelector('#login-view').hidden &&
     document.querySelector('#login-desc').textContent.includes(${JSON.stringify(`${SSH_USER}@${SSH_HOST}:${SSH_PORT}`)})`,
    20_000,
    'SSH login status',
  );
  await waitFor(
    `document.querySelector('#login-error').textContent.includes('无法连接')`,
    20_000,
    'SSH connection error',
  );
  pass('unreachable SSH host reports a clear error');

  await openPanel();
  await waitFor(`${rowFor(SSH_NAME)}`, 5_000, 'SSH row');
  const rowText = await evaluate(`${rowFor(SSH_NAME)}.textContent`);
  if (!rowText.includes('SSH') || !rowText.includes(`${SSH_USER}@${SSH_HOST}:${SSH_PORT}`)) {
    throw new Error(`unexpected SSH row: ${rowText}`);
  }
  pass('manager lists the SSH target with kind and endpoint');

  await evaluate(`(() => {
    const row = ${rowFor(SSH_NAME)};
    [...row.querySelectorAll('button')].find((button) => button.textContent === '编辑').click();
    return true;
  })()`);
  const prefill = await evaluate(`(() => {
    const value = (label) => {
      const field = [...document.querySelectorAll('#server-panel .field')]
        .find((node) => node.querySelector('span')?.textContent === label);
      return field?.querySelector('input')?.value;
    };
    return {
      host: value('主机'),
      port: value('端口'),
      user: value('用户名'),
      name: value('名称'),
    };
  })()`);
  if (
    prefill.host !== SSH_HOST ||
    prefill.port !== SSH_PORT ||
    prefill.user !== SSH_USER ||
    prefill.name !== SSH_NAME
  ) {
    throw new Error(`bad SSH edit prefill: ${JSON.stringify(prefill)}`);
  }
  pass('edit form prefills the SSH target');

  const renamed = `${SSH_NAME} 改`;
  await setField('名称', renamed);
  if (!(await clickButton('保存修改'))) throw new Error('save button missing');
  await waitFor(tabExists(renamed), 15_000, 'renamed SSH tab');
  pass('editing renames the SSH tab');

  await openPanel();
  await waitFor(`${rowFor(renamed)}`, 5_000, 'renamed row');
  await evaluate(`(() => {
    const row = ${rowFor(renamed)};
    [...row.querySelectorAll('button')].find((button) => button.textContent === '删除').click();
    return true;
  })()`);
  await waitFor(`!${tabExists(renamed)}`, 10_000, 'SSH tab removed');
  await waitFor("!document.querySelector('#workspace-bar').hidden", 5_000, 'local survives');
  pass('deleting the SSH target closes its tab');

  const relevantErrors = errors.filter(
    (message) => message && !/favicon|Failed to load resource/i.test(message),
  );
  if (relevantErrors.length) throw new Error(`page errors: ${relevantErrors.join(' | ')}`);
  pass('no JS exceptions');

  console.log('\nSSH UI SMOKE PASSED');
} finally {
  socket.close();
  await fetch(`${DEBUG_URL}/json/close/${target.id}`).catch(() => {});
}
