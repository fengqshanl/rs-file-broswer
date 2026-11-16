#!/usr/bin/env node
/**
 * Theme, persisted settings and connection-picker regression test over CDP.
 *
 * Usage (against dev_app):
 *   FRONT_URL=http://127.0.0.1:<port>/ \
 *   CHROME_DEBUG_URL=http://127.0.0.1:9223 \
 *   node scripts/ui-theme-smoke.mjs
 */

const DEBUG_URL = (process.env.CHROME_DEBUG_URL || 'http://127.0.0.1:9223').replace(/\/+$/, '');
const PAGE_URL = process.env.FRONT_URL || '';

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

const emulateScheme = (value) =>
  send('Emulation.setEmulatedMedia', {
    features: [{ name: 'prefers-color-scheme', value }],
  });

const click = (selector) =>
  evaluate(`(() => {
    const node = document.querySelector(${JSON.stringify(selector)});
    if (!node) return false;
    node.click();
    return true;
  })()`);

const themeState = () =>
  evaluate(`({
    theme: document.documentElement.dataset.theme,
    choice: document.documentElement.dataset.themeChoice,
  })`);

const settings = () =>
  evaluate(`(async () => (await (await fetch('/app/info', { cache: 'no-store' })).json()).settings)()`);

try {
  await send('Page.navigate', { url: PAGE_URL });
  await waitFor(
    "document.readyState === 'complete' && !document.querySelector('#workspace-bar').hidden",
    20_000,
    'workspace boot',
  );
  await waitFor("!!document.querySelector('#theme-switch')", 5_000, 'theme switch');

  const initial = await themeState();
  if (initial.choice !== 'auto') throw new Error(`unexpected initial theme choice: ${JSON.stringify(initial)}`);
  const initialSettings = await settings();
  if (!initialSettings || initialSettings.theme !== 'auto') {
    throw new Error(`settings endpoint missing auto theme: ${JSON.stringify(initialSettings)}`);
  }
  pass('settings endpoint and automatic theme boot');

  await emulateScheme('light');
  await waitFor("document.documentElement.dataset.theme === 'light'", 5_000, 'system light');
  await emulateScheme('dark');
  await waitFor("document.documentElement.dataset.theme === 'dark'", 5_000, 'system dark');
  pass('automatic theme follows the system preference');

  await click('#theme-switch [data-theme-choice="light"]');
  await waitFor("document.documentElement.dataset.theme === 'light'", 3_000, 'manual light');
  await sleep(450);
  const lightSettings = await settings();
  if (lightSettings.theme !== 'light') {
    throw new Error(`light theme not persisted: ${JSON.stringify(lightSettings)}`);
  }
  pass('manual light theme persists to settings.json');

  await send('Page.reload');
  await waitFor(
    "document.readyState === 'complete' && !document.querySelector('#workspace-bar').hidden",
    20_000,
    'reload',
  );
  const afterReload = await themeState();
  if (afterReload.choice !== 'light' || afterReload.theme !== 'light') {
    throw new Error(`theme did not survive reload: ${JSON.stringify(afterReload)}`);
  }
  pass('theme survives a reload (cookie + settings)');

  await click('#theme-switch [data-theme-choice="auto"]');
  await waitFor("document.documentElement.dataset.theme === 'dark'", 3_000, 'auto follows dark');
  await sleep(450);
  pass('switching back to automatic follows the system again');

  await click('#toggle-hidden');
  await sleep(450);
  const hiddenSettings = await settings();
  if (!hiddenSettings.show_hidden) {
    throw new Error(`hidden preference not persisted: ${JSON.stringify(hiddenSettings)}`);
  }
  await send('Page.reload');
  await waitFor(
    "document.readyState === 'complete' && !document.querySelector('#workspace-bar').hidden",
    20_000,
    'reload after hidden toggle',
  );
  const hiddenChecked = await evaluate("document.querySelector('#toggle-hidden').checked");
  if (!hiddenChecked) throw new Error('hidden-file preference did not survive reload');
  await click('#toggle-hidden');
  await sleep(300);
  pass('hidden-file preference persists through settings.json');

  const opened = await click('#conn-trigger');
  if (!opened) throw new Error('connection trigger missing');
  await waitFor("!document.querySelector('#conn-menu').hidden", 3_000, 'connection menu');
  const menu = await evaluate(`({
    expanded: document.querySelector('#conn-trigger').getAttribute('aria-expanded'),
    names: [...document.querySelectorAll('#conn-menu .conn-item-name')].map((node) => node.textContent),
    manage: !!document.querySelector('#conn-menu .conn-manage'),
  })`);
  if (menu.expanded !== 'true' || !menu.names.includes('本机') || !menu.manage) {
    throw new Error(`unexpected connection menu: ${JSON.stringify(menu)}`);
  }
  await evaluate(
    "document.querySelector('#conn-menu').dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })); true",
  );
  await waitFor("document.querySelector('#conn-menu').hidden", 3_000, 'menu closes on Escape');
  if ((await evaluate("document.querySelector('#conn-trigger').getAttribute('aria-expanded')")) !== 'false') {
    throw new Error('aria-expanded not reset after Escape');
  }
  pass('connection picker opens, lists local and closes on Escape');

  await click('#btn-add-server');
  await waitFor("!document.querySelector('#server-overlay').hidden && !!document.querySelector('.cm-sidebar')", 5_000, 'manager');
  const manager = await evaluate(`({
    local: [...document.querySelectorAll('.cm-item.local .conn-item-name')].map((node) => node.textContent),
    empty: !document.querySelector('.cm-editor-empty').hidden,
    formHidden: document.querySelector('.cm-form').hidden,
  })`);
  if (!manager.local.includes('本机磁盘') || !manager.empty || !manager.formHidden) {
    throw new Error(`unexpected manager initial state: ${JSON.stringify(manager)}`);
  }
  await click('[data-testid=add-connection]');
  await waitFor("!document.querySelector('.cm-form').hidden", 3_000, 'add form');
  await click('[data-testid=connection-kind] [data-kind=ssh]');
  const kind = await evaluate(`({
    sshVisible: !document.querySelector('.cm-section:nth-of-type(3)').hidden,
    sshButton: document.querySelector('[data-testid=connection-kind] [data-kind=ssh]').classList.contains('active'),
    host: !!document.querySelector('[data-testid=connection-ssh-host]'),
  })`);
  if (!kind.sshVisible || !kind.sshButton || !kind.host) {
    throw new Error(`SSH form did not open: ${JSON.stringify(kind)}`);
  }
  await click('.cm-actions .btn.ghost:not([data-testid])');
  await waitFor("!document.querySelector('.cm-editor-empty').hidden", 3_000, 'cancel to empty state');
  await evaluate("document.querySelector('#server-overlay').hidden = true; true");
  pass('connection manager opens, switches to SSH and cancels');

  const cspErrors = errors.filter((message) => /Content Security Policy/i.test(message));
  if (cspErrors.length) throw new Error(`CSP violations: ${cspErrors.join(' | ')}`);
  if (errors.length) throw new Error(`page errors: ${errors.join(' | ')}`);
  pass('no CSP violations or JS exceptions');

  console.log('\nTHEME / SETTINGS UI TEST PASSED');
} finally {
  socket.close();
  await fetch(`${DEBUG_URL}/json/close/${target.id}`).catch(() => {});
}
