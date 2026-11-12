'use strict';

const $ = (selector) => document.querySelector(selector);
const encoder = new TextEncoder();
const decoder = new TextDecoder();

const ICONS = {
  folder:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z"/></svg>',
  file:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M14 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9Z"/><path d="M14 3v6h6"/></svg>',
  download:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M12 4v12"/><path d="m7 11 5 5 5-5"/><path d="M5 20h14"/></svg>',
  edit:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4Z"/></svg>',
  trash:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M4 7h16"/><path d="M10 11v6"/><path d="M14 11v6"/><path d="M6 7l1 13a2 2 0 0 0 2 1.8h6A2 2 0 0 0 17 20l1-13"/><path d="M9 7V4h6v3"/></svg>',
  home:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="m3 11 9-8 9 8"/><path d="M5 10v10h14V10"/></svg>',
  eye:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12Z"/><circle cx="12" cy="12" r="3"/></svg>',
  close:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M6 6l12 12"/><path d="M18 6 6 18"/></svg>',
};

const ARCHIVE_END_MARKER = 'AGF_TAR_END_v1';
const LIST_PAGE_SIZE = Math.max(1, Number(window.AGENTGOD_PAGE_SIZE) || 300);
const TEXT_STREAM_BYTES = 32 * 1024 * 1024;
const HISTORY_KEY = 'agentgod.history.v1';
const HISTORY_LIMIT = 300;
const OFFICE_PREVIEW_BYTES = 64 * 1024 * 1024;
const IMAGE_PREVIEW_BYTES = 128 * 1024 * 1024;
const PDF_PREVIEW_BYTES = 256 * 1024 * 1024;
const MEDIA_PREVIEW_BYTES = 512 * 1024 * 1024;
const HTML_PREVIEW_BYTES = 32 * 1024 * 1024;
const BLOB_DOWNLOAD_LIMIT = 1024 * 1024 * 1024;
const SHEET_MAX_ROWS = 2000;
const SHEET_MAX_COLS = 80;

const IMAGE_EXTENSIONS = new Set([
  'png', 'jpg', 'jpeg', 'jfif', 'gif', 'webp', 'avif', 'bmp', 'ico', 'svg',
]);
const VIDEO_EXTENSIONS = new Set(['mp4', 'm4v', 'webm', 'ogv', 'mov', 'mkv']);
const AUDIO_EXTENSIONS = new Set(['mp3', 'wav', 'ogg', 'oga', 'm4a', 'aac', 'flac']);
const TEXT_EXTENSIONS = new Set([
  'txt', 'log', 'md', 'markdown', 'json', 'yaml', 'yml', 'toml', 'ini', 'conf',
  'cfg', 'env', 'rs', 'py', 'js', 'mjs', 'ts', 'tsx', 'jsx', 'css', 'scss',
  'sh', 'bash', 'zsh', 'c', 'h', 'cpp', 'hpp', 'go', 'java', 'kt', 'swift',
  'sql', 'xml', 'vue', 'svelte', 'lock', 'gitignore', 'dockerfile',
]);
const SHEET_EXTENSIONS = new Set(['xlsx', 'xlsm', 'xls', 'csv', 'tsv']);

const state = {
  key: null,
  token: null,
  user: null,
  cwd: '/',
  entries: [],
  filter: '',
  list: { offset: 0, total: 0, hasMore: false, loading: false },
  pageSize: LIST_PAGE_SIZE,
  autoLoadPaging: true,
  navToken: 0,
  pendingHighlight: null,
  app: null,
  activeTarget: null,
};

const uploadQueue = [];
let uploadRunning = false;
let dragDepth = 0;

/* ---------- DOM helpers ---------- */

function div(className = '') {
  const element = document.createElement('div');
  if (className) element.className = className;
  return element;
}

function span(className = '') {
  const element = document.createElement('span');
  if (className) element.className = className;
  return element;
}

function buttonEl(text, className) {
  const button = document.createElement('button');
  button.type = 'button';
  button.className = className;
  button.textContent = text;
  return button;
}

function spinnerNode() {
  return span('spinner');
}

function iconNode(name) {
  const element = span('icon');
  element.innerHTML = ICONS[name];
  return element;
}

function actionButton(name, label, handler, danger = false) {
  const button = document.createElement('button');
  button.type = 'button';
  button.className = 'action-btn' + (danger ? ' danger' : '');
  button.title = label;
  button.setAttribute('aria-label', label);
  button.innerHTML = ICONS[name];
  button.addEventListener('click', (event) => {
    event.stopPropagation();
    handler();
  });
  return button;
}

/* ---------- Encoding helpers ---------- */

function bytesToBase64(input) {
  const bytes = input instanceof Uint8Array ? input : new Uint8Array(input);
  let binary = '';
  const step = 0x8000;
  for (let index = 0; index < bytes.length; index += step) {
    binary += String.fromCharCode.apply(null, bytes.subarray(index, index + step));
  }
  return btoa(binary);
}

function base64ToBytes(text) {
  const binary = atob(text);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return bytes;
}

function randomBytes(length) {
  const bytes = new Uint8Array(length);
  crypto.getRandomValues(bytes);
  return bytes;
}

function pemToDer(pem) {
  const body = pem
    .replace(/-----BEGIN [^-]+-----/g, '')
    .replace(/-----END [^-]+-----/g, '')
    .replace(/\s+/g, '');
  return base64ToBytes(body);
}

function concatBytes(first, second) {
  if (!first.length) return second;
  const result = new Uint8Array(first.length + second.length);
  result.set(first);
  result.set(second, first.length);
  return result;
}

function formatSize(bytes) {
  const value = Number(bytes) || 0;
  if (value < 1024) return `${value} B`;
  const units = ['KB', 'MB', 'GB', 'TB', 'PB'];
  let size = value / 1024;
  let index = 0;
  while (size >= 1024 && index < units.length - 1) {
    size /= 1024;
    index += 1;
  }
  return `${size >= 10 ? size.toFixed(0) : size.toFixed(1)} ${units[index]}`;
}

const dateFormatter = new Intl.DateTimeFormat('zh-CN', {
  year: 'numeric',
  month: '2-digit',
  day: '2-digit',
  hour: '2-digit',
  minute: '2-digit',
});

function formatDate(milliseconds) {
  if (!milliseconds) return '—';
  try {
    return dateFormatter.format(new Date(milliseconds));
  } catch {
    return '—';
  }
}

function joinPath(base, name) {
  const trimmed = base === '/' ? '' : base.replace(/\/+$/, '');
  return `${trimmed}/${name}`;
}

function normalizeRelPath(input) {
  const parts = String(input || '')
    .split('/')
    .filter((part) => part && part !== '.');
  if (!parts.length) return '';
  if (parts.some((part) => part === '..' || part.includes('\0'))) return '';
  return parts.join('/');
}

function fileExtension(name) {
  const index = name.lastIndexOf('.');
  if (index <= 0 || index === name.length - 1) return '';
  return name.slice(index + 1).toLowerCase();
}

function dirnameOf(relativePath) {
  const index = relativePath.lastIndexOf('/');
  return index === -1 ? '' : relativePath.slice(0, index);
}

function previewKindOf(name) {
  const extension = fileExtension(name);
  if (IMAGE_EXTENSIONS.has(extension)) {
    return extension === 'svg' ? 'svg' : 'image';
  }
  if (VIDEO_EXTENSIONS.has(extension)) return 'video';
  if (AUDIO_EXTENSIONS.has(extension)) return 'audio';
  if (extension === 'pdf') return 'pdf';
  if (extension === 'html' || extension === 'htm') return 'html';
  if (extension === 'docx') return 'docx';
  if (SHEET_EXTENSIONS.has(extension)) return 'sheet';
  if (TEXT_EXTENSIONS.has(extension)) return 'text';
  return 'none';
}

/* ---------- Crypto ---------- */

function ensureCrypto() {
  if (!window.crypto || !window.crypto.subtle) {
    throw new Error('当前页面不是安全上下文（需要 HTTPS），浏览器禁用了加密接口');
  }
}

async function importAesKey(rawKey) {
  return crypto.subtle.importKey('raw', rawKey, 'AES-GCM', false, [
    'encrypt',
    'decrypt',
  ]);
}

async function encryptWithKey(key, aad, value) {
  const iv = randomBytes(12);
  const plaintext = encoder.encode(JSON.stringify(value));
  const ciphertext = await crypto.subtle.encrypt(
    { name: 'AES-GCM', iv, additionalData: encoder.encode(aad) },
    key,
    plaintext,
  );
  return { iv: bytesToBase64(iv), ct: bytesToBase64(ciphertext) };
}

async function decryptWithKey(key, aad, envelope) {
  const plaintext = await crypto.subtle.decrypt(
    {
      name: 'AES-GCM',
      iv: base64ToBytes(envelope.iv),
      additionalData: encoder.encode(aad),
    },
    key,
    base64ToBytes(envelope.ct),
  );
  return JSON.parse(decoder.decode(plaintext));
}

async function encryptPayload(aad, value) {
  if (!state.key) throw new Error('会话已失效，请重新登录');
  return encryptWithKey(state.key, aad, value);
}

async function decryptPayload(aad, envelope) {
  if (!state.key) throw new Error('会话已失效，请重新登录');
  if (!envelope || !envelope.iv || !envelope.ct) {
    throw new Error('服务器响应格式异常');
  }
  return decryptWithKey(state.key, aad, envelope);
}

/* ---------- API ---------- */

async function api(path, payload = {}, method = 'POST') {
  const aad = `${method} ${path}`;
  const envelope = await encryptPayload(aad, payload);
  const response = await fetch(path, {
    method,
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${state.token}`,
    },
    body: JSON.stringify(envelope),
  });
  let data = null;
  try {
    data = await response.json();
  } catch {
    data = null;
  }
  if (!response.ok) {
    if (response.status === 401) handleSessionLost('会话已过期，请重新登录');
    throw new Error(data && data.error ? data.error : `请求失败 (${response.status})`);
  }
  return decryptPayload(aad, data);
}

async function login(username, password) {
  ensureCrypto();
  const keyResponse = await fetch('/api/pubkey', { cache: 'no-store' });
  if (!keyResponse.ok) throw new Error('无法获取服务器公钥');
  const keyInfo = await keyResponse.json();
  const publicKey = await crypto.subtle.importKey(
    'spki',
    pemToDer(keyInfo.public_key),
    { name: 'RSA-OAEP', hash: 'SHA-256' },
    false,
    ['encrypt'],
  );

  const rawKey = randomBytes(32);
  const sessionKey = await importAesKey(rawKey);
  const iv = randomBytes(12);
  const payload = encoder.encode(
    JSON.stringify({ username, password, ts: Date.now() }),
  );
  const ciphertext = await crypto.subtle.encrypt(
    { name: 'AES-GCM', iv, additionalData: encoder.encode('login') },
    sessionKey,
    payload,
  );
  const wrappedKey = await crypto.subtle.encrypt(
    { name: 'RSA-OAEP' },
    publicKey,
    rawKey,
  );

  const response = await fetch('/api/login', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      ek: bytesToBase64(wrappedKey),
      iv: bytesToBase64(iv),
      ct: bytesToBase64(ciphertext),
    }),
  });
  let data = null;
  try {
    data = await response.json();
  } catch {
    data = null;
  }
  if (!response.ok) {
    throw new Error(data && data.error ? data.error : '登录失败');
  }

  const result = await decryptWithKey(sessionKey, 'login-resp', data);
  state.key = sessionKey;
  state.token = result.token;
  state.user = result.user;
}

/* ---------- Session / views ---------- */

function showLoginError(message) {
  $('#login-error').textContent = message || '';
}

function handleSessionLost(message) {
  if (isLocalTarget()) {
    location.reload();
    return;
  }
  state.key = null;
  state.token = null;
  state.user = null;
  state.entries = [];
  state.filter = '';
  state.cwd = '/';
  state.list = { offset: 0, total: 0, hasMore: false, loading: false };
  state.pageSize = LIST_PAGE_SIZE;
  state.navToken += 1;
  state.pendingHighlight = null;
  uploadQueue.length = 0;
  $('#app-view').hidden = true;
  $('#login-view').hidden = false;
  $('#upload-panel').hidden = true;
  $('#upload-list').replaceChildren();
  $('#file-list').replaceChildren();
  $('#list-skeleton').hidden = true;
  $('#list-footer').replaceChildren();
  $('#list-footer').hidden = true;
  $('#dir-summary').textContent = '';
  $('#free-space').textContent = '';
  uploadQueue.length = 0;
  $('#password').value = '';
  const dialog = $('#modal');
  if (dialog.open) dialog.close();
  if (message) showLoginError(message);
  setTimeout(() => {
    if (!$('#username').value) $('#username').focus();
  }, 30);
}

async function onLogin(event) {
  event.preventDefault();
  showLoginError('');
  const username = $('#username').value.trim();
  const password = $('#password').value;
  if (!username || !password) {
    showLoginError('请输入用户名和密码');
    return;
  }
  const button = $('#login-btn');
  button.disabled = true;
  button.textContent = '正在加密登录…';
  try {
    await login(username, password);
    $('#password').value = '';
    showAppView();
    $('#user-badge').textContent = state.user || username;
    await loadDir('/');
  } catch (error) {
    showLoginError(error.message || '登录失败');
  } finally {
    button.disabled = false;
    button.textContent = '加密登录';
  }
}

async function onLogout() {
  if (isLocalTarget()) {
    location.reload();
    return;
  }
  try {
    if (state.token) await api('/api/logout', {});
  } catch {
    /* the local session is discarded regardless */
  }
  handleSessionLost('');
  showLoginError('');
  toast('已安全退出', 'info');
}

/* ---------- Listing ---------- */

async function loadDir(path, options = {}) {
  if (!state.key) return;
  const target = normalizeRelPath(path) || '/';
  state.cwd = target;
  state.entries = [];
  state.filter = '';
  $('#filter').value = '';
  state.list = { offset: 0, total: 0, hasMore: false, loading: false };
  state.navToken = (state.navToken || 0) + 1;
  const token = state.navToken;
  renderBreadcrumbs();
  showListSkeleton(true);
  $('#dir-summary').textContent = '正在加载…';

  try {
    const data = await api('/api/list', {
      path: target,
      offset: 0,
      limit: state.pageSize,
    });
    if (token !== state.navToken) return;
    state.cwd = data.path || target;
    state.entries = Array.isArray(data.entries) ? data.entries : [];
    state.list.offset = state.entries.length;
    state.list.total = Number(data.total) || state.entries.length;
    state.list.hasMore = Boolean(data.has_more);
    $('#free-space').textContent =
      typeof data.free_space === 'number' ? `剩余 ${formatSize(data.free_space)}` : '';
    renderBreadcrumbs();
    showListSkeleton(false);
    renderFiles();
    if (data.truncated) {
      toast('目录项目超过 50000 个，仅显示前 50000 项', 'error');
    }
    if (!options.skipHistory) {
      recordHistory('dir', state.cwd, basenameOf(state.cwd), parentOf(state.cwd));
    }
    if (state.pendingHighlight) {
      const pending = state.pendingHighlight;
      state.pendingHighlight = null;
      await highlightEntry(pending);
    }
  } catch (error) {
    if (token !== state.navToken) return;
    showListSkeleton(false);
    $('#dir-summary').textContent = '加载失败';
    renderFiles();
    toast(error.message || '读取目录失败', 'error');
  }
}

function renderBreadcrumbs() {
  const nav = $('#breadcrumbs');
  nav.replaceChildren();
  const root = buttonEl('', 'crumb');
  root.append(iconNode('home'), span(''));
  root.lastChild.textContent = '根目录';
  root.addEventListener('click', () => loadDir('/'));
  nav.append(root);

  let accumulated = '';
  for (const segment of state.cwd.split('/').filter(Boolean)) {
    accumulated += `/${segment}`;
    const separator = span('crumb-sep');
    separator.textContent = '/';
    const target = accumulated;
    const button = buttonEl(segment, 'crumb');
    button.title = target;
    button.addEventListener('click', () => loadDir(target));
    nav.append(separator, button);
  }
}

function visibleEntries() {
  const filter = state.filter.trim().toLowerCase();
  if (!filter) return state.entries;
  return state.entries.filter((entry) => entry.name.toLowerCase().includes(filter));
}

function showListSkeleton(show) {
  const skeleton = $('#list-skeleton');
  if (!skeleton) return;
  skeleton.hidden = !show;
  if (show) {
    if (!skeleton.children.length) {
      for (let index = 0; index < 8; index += 1) {
        skeleton.append(div('skeleton-row'));
      }
    }
    $('#file-list').hidden = true;
    $('#list-footer').hidden = true;
    $('#empty-state').hidden = true;
    $('.list-head').hidden = true;
  }
}

function renderFiles() {
  const list = $('#file-list');
  list.replaceChildren();
  const visible = visibleEntries();
  const hasEntries = visible.length > 0;
  $('.list-head').hidden = !hasEntries;
  list.hidden = !hasEntries;
  const empty = $('#empty-state');
  empty.hidden = hasEntries || state.list.loading;
  if (!hasEntries && !state.list.loading) {
    empty.querySelector('p').textContent = state.filter.trim()
      ? '没有匹配的项目'
      : '这个文件夹还是空的';
    empty.querySelector('span:last-child').textContent = state.filter.trim()
      ? '换个关键词试试'
      : '拖拽文件到此处或点击“上传”';
  }

  for (const entry of visible) {
    list.append(renderRow(entry));
  }
  updateListSummary();
  renderListFooter();
}

function updateListSummary() {
  const visible = visibleEntries().length;
  const loaded = state.entries.length;
  const total = Math.max(state.list.total, loaded);
  if (state.filter.trim()) {
    $('#dir-summary').textContent = `${visible} 个匹配项 · 已加载 ${loaded} / ${total} 项`;
  } else if (loaded < total) {
    $('#dir-summary').textContent = `已加载 ${loaded} / ${total} 项`;
  } else {
    $('#dir-summary').textContent = `${total} 个项目`;
  }
}

function renderListFooter() {
  const footer = $('#list-footer');
  footer.replaceChildren();
  if (state.filter.trim()) {
    footer.hidden = true;
    return;
  }
  if (state.list.loading) {
    footer.hidden = false;
    const row = div('list-loading');
    const label = span('');
    label.textContent = '正在加载更多…';
    row.append(spinnerNode(), label);
    footer.append(row);
    return;
  }
  if (state.list.hasMore) {
    footer.hidden = false;
    const button = buttonEl('加载更多', 'btn ghost load-more');
    button.addEventListener('click', () => void loadMore());
    footer.append(button);
    return;
  }
  if (state.entries.length > 0) {
    footer.hidden = false;
    const note = div('list-loading muted');
    note.textContent = `已加载全部 ${state.list.total || state.entries.length} 项`;
    footer.append(note);
  } else {
    footer.hidden = true;
  }
}

function appendRows(fromIndex) {
  if (state.filter.trim()) {
    renderFiles();
    return;
  }
  const list = $('#file-list');
  const visible = visibleEntries();
  for (let index = fromIndex; index < visible.length; index += 1) {
    list.append(renderRow(visible[index]));
  }
  const hasEntries = visible.length > 0;
  $('.list-head').hidden = !hasEntries;
  list.hidden = !hasEntries;
  $('#empty-state').hidden = hasEntries;
  updateListSummary();
  renderListFooter();
}

async function loadMore() {
  if (!state.key || state.list.loading || !state.list.hasMore) return;
  const token = state.navToken;
  state.list.loading = true;
  renderListFooter();
  try {
    const data = await api('/api/list', {
      path: state.cwd,
      offset: state.list.offset,
      limit: state.pageSize,
    });
    if (token !== state.navToken) return;
    const before = state.entries.length;
    const entries = Array.isArray(data.entries) ? data.entries : [];
    state.entries.push(...entries);
    state.list.offset = state.entries.length;
    state.list.total = Number(data.total) || state.list.total;
    state.list.hasMore = Boolean(data.has_more);
    appendRows(before);
  } catch (error) {
    if (token === state.navToken) {
      toast(error.message || '加载更多失败', 'error');
    }
  } finally {
    if (token === state.navToken) {
      state.list.loading = false;
      renderListFooter();
    }
  }
}

async function highlightEntry(fullPath) {
  for (let attempt = 0; attempt < 200; attempt += 1) {
    const row = findRow(fullPath);
    if (row) {
      flashRow(row);
      return;
    }
    if (!state.list.hasMore) return;
    const before = state.entries.length;
    await loadMore();
    if (state.entries.length === before) return;
  }
}

function findRow(fullPath) {
  for (const row of document.querySelectorAll('#file-list .file-row')) {
    if (row.dataset.path === fullPath) return row;
  }
  return null;
}

function flashRow(row) {
  row.classList.add('highlight');
  row.scrollIntoView({ block: 'center', behavior: 'smooth' });
  setTimeout(() => row.classList.remove('highlight'), 3200);
}

function renderRow(entry) {
  const row = div('file-row');
  row.setAttribute('role', 'listitem');

  const nameCell = div('cell name-cell');
  const icon = span('file-icon ' + (entry.is_dir ? 'is-dir' : 'is-file'));
  icon.innerHTML = entry.is_dir ? ICONS.folder : ICONS.file;
  nameCell.append(icon);

  if (entry.is_dir) {
    const nameButton = buttonEl(entry.name, 'name-btn');
    nameButton.title = entry.name;
    nameButton.addEventListener('click', () =>
      loadDir(joinPath(state.cwd, entry.name)),
    );
    nameCell.append(nameButton);
  } else {
    const name = span('file-name');
    name.textContent = entry.name;
    name.title = entry.name;
    if (previewKindOf(entry.name) !== 'none') {
      name.classList.add('previewable');
      name.addEventListener('click', () => openPreview(entry));
    }
    nameCell.append(name);
  }
  if (entry.is_symlink) {
    const badge = span('badge');
    badge.textContent = '链接';
    nameCell.append(badge);
  }

  const sizeCell = div('cell size');
  sizeCell.textContent = entry.is_dir ? '—' : formatSize(entry.size);

  const timeCell = div('cell time');
  timeCell.textContent = formatDate(entry.modified);

  const actions = div('cell actions');
  const fullPath = joinPath(state.cwd, entry.name);
  row.dataset.path = fullPath;
  if (entry.is_dir) {
    actions.append(
      actionButton('download', '下载文件夹 (.tar)', () =>
        startFolderDownload(fullPath, entry.name),
      ),
    );
  } else {
    if (previewKindOf(entry.name) !== 'none') {
      actions.append(
        actionButton('eye', '预览', () => openPreview(entry)),
      );
    }
    actions.append(
      actionButton('download', '下载', () => startDownload(fullPath, entry.name)),
    );
  }
  actions.append(
    actionButton('edit', '重命名', () => renameEntry(entry)),
    actionButton('trash', '删除', () => deleteEntry(entry), true),
  );

  row.append(nameCell, sizeCell, timeCell, actions);
  return row;
}

/* ---------- File operations ---------- */

async function renameEntry(entry) {
  const newName = await askText({
    title: '重命名',
    label: '新的名称',
    value: entry.name,
    confirmText: '重命名',
  });
  if (!newName || newName === entry.name) return;
  try {
    await api('/api/rename', {
      path: joinPath(state.cwd, entry.name),
      new_name: newName,
    });
    toast('已重命名', 'success');
    await loadDir(state.cwd);
  } catch (error) {
    toast(error.message || '重命名失败', 'error');
  }
}

async function deleteEntry(entry) {
  const detail = entry.is_dir
    ? '该文件夹及其中的全部内容都会被删除，此操作不可恢复。'
    : '此操作不可恢复。';
  const confirmed = await confirmDialog({
    title: `删除「${entry.name}」？`,
    message: detail,
  });
  if (!confirmed) return;
  try {
    await api('/api/delete', { path: joinPath(state.cwd, entry.name) });
    toast('已删除', 'success');
    await loadDir(state.cwd);
  } catch (error) {
    toast(error.message || '删除失败', 'error');
  }
}

async function createFolder() {
  const name = await askText({
    title: '新建文件夹',
    label: '文件夹名称',
    placeholder: '例如：备份',
    confirmText: '创建',
  });
  if (!name) return;
  try {
    await api('/api/mkdir', { path: joinPath(state.cwd, name) });
    toast('文件夹已创建', 'success');
    await loadDir(state.cwd);
  } catch (error) {
    toast(error.message || '创建文件夹失败', 'error');
  }
}

/* ---------- Download ---------- */

async function chooseSaveTarget(suggestedName) {
  if (typeof window.showSaveFilePicker !== 'function') {
    return { writable: null, usePicker: false };
  }
  try {
    const handle = await window.showSaveFilePicker({ suggestedName });
    const writable = await handle.createWritable();
    return { writable, usePicker: true };
  } catch (error) {
    if (error && error.name === 'AbortError') throw error;
    return { writable: null, usePicker: false };
  }
}

async function startDownload(path, name) {
  recordHistory('file', path, name, parentOf(path));
  await streamEncryptedDownload({
    endpoint: '/api/download',
    payload: { path },
    suggestedName: name,
    expectEndMarker: false,
    title: `下载 ${name}`,
  });
}

async function startFolderDownload(path, name) {
  recordHistory('dir', path, name, parentOf(path));
  await streamEncryptedDownload({
    endpoint: '/api/download_folder',
    payload: { path },
    suggestedName: `${name}.tar`,
    expectEndMarker: true,
    title: `打包 ${name}`,
  });
}

async function streamEncryptedDownload({
  endpoint,
  payload,
  suggestedName,
  expectEndMarker,
  title,
}) {
  let target;
  try {
    target = await chooseSaveTarget(suggestedName);
  } catch (error) {
    if (error && error.name === 'AbortError') return;
    target = { writable: null, usePicker: false };
  }

  const progress = progressToast(title);
  try {
    const aad = `POST ${endpoint}`;
    const envelope = await encryptPayload(aad, payload);
    const response = await fetch(endpoint, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        Authorization: `Bearer ${state.token}`,
      },
      body: JSON.stringify(envelope),
    });
    if (!response.ok) {
      let message = null;
      try {
        const body = await response.json();
        message = body && body.error;
      } catch {
        message = null;
      }
      if (response.status === 401) handleSessionLost('会话已过期，请重新登录');
      throw new Error(message || `下载失败 (${response.status})`);
    }

    const total = Number(response.headers.get('x-file-size') || '0');
    const chunkSize = Number(response.headers.get('x-chunk-size') || '65536');
    if (!target.usePicker && !expectEndMarker && total > BLOB_DOWNLOAD_LIMIT) {
      throw new Error('当前浏览器不支持流式保存，超过 1 GB 的文件请使用 Chrome 或 Edge 下载');
    }
    if (expectEndMarker) progress.update(null, '正在打包…');

    const parts = target.usePicker ? null : [];
    const reader = response.body.getReader();
    let pending = new Uint8Array(0);
    let sequence = 0;
    let received = 0;
    let ended = false;

    outer: while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      pending = concatBytes(pending, value);
      while (pending.length >= 4) {
        const length = new DataView(pending.buffer, pending.byteOffset, 4).getUint32(0, false);
        if (length > chunkSize + 16 + 1024) {
          throw new Error('下载数据校验失败');
        }
        if (pending.length < 4 + 12 + length) break;
        const iv = pending.subarray(4, 16);
        const ciphertext = pending.subarray(16, 16 + length);
        let plaintext;
        try {
          plaintext = new Uint8Array(
            await crypto.subtle.decrypt(
              {
                name: 'AES-GCM',
                iv,
                additionalData: encoder.encode(`dl:${sequence}`),
              },
              state.key,
              ciphertext,
            ),
          );
        } catch {
          throw new Error('下载数据解密失败，连接可能已被篡改');
        }
        sequence += 1;
        pending = pending.subarray(16 + length);

        if (
          expectEndMarker &&
          plaintext.length === ARCHIVE_END_MARKER.length &&
          decoder.decode(plaintext) === ARCHIVE_END_MARKER
        ) {
          ended = true;
          break outer;
        }
        if (plaintext.length > chunkSize) {
          throw new Error('下载分块大小异常');
        }

        if (target.usePicker) {
          await target.writable.write(plaintext);
        } else {
          if (received + plaintext.length > BLOB_DOWNLOAD_LIMIT) {
            throw new Error('已超过浏览器内存下载上限 1 GB，请改用 Chrome 或 Edge');
          }
          parts.push(plaintext);
        }
        received += plaintext.length;
        progress.update(
          total > 0 ? received / total : null,
          total > 0
            ? `${formatSize(received)} / ${formatSize(total)}`
            : formatSize(received),
        );
      }
    }

    if (expectEndMarker && !ended) throw new Error('归档传输不完整，请重试');
    if (!expectEndMarker) {
      if (pending.length !== 0) throw new Error('下载数据不完整');
      if (received !== total) throw new Error('文件大小校验失败');
    }

    if (target.usePicker) {
      await target.writable.close();
    } else {
      const blob = new Blob(parts, { type: 'application/octet-stream' });
      triggerBlobDownload(blob, suggestedName);
    }
    progress.done(`已保存 ${suggestedName}`);
    toast(`下载完成：${suggestedName}`, 'success');
  } catch (error) {
    try {
      if (target.writable) await target.writable.abort();
    } catch {
      /* ignore abort errors */
    }
    const message = error.message || '下载失败';
    progress.fail(message);
    toast(message, 'error');
  }
}

function triggerBlobDownload(blob, name) {
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement('a');
  anchor.href = url;
  anchor.download = name;
  document.body.append(anchor);
  anchor.click();
  anchor.remove();
  setTimeout(() => URL.revokeObjectURL(url), 60_000);
}

/* ---------- Upload ---------- */

function toUploadItems(input) {
  return Array.from(input || []).map((entry) => {
    if (entry && entry.file instanceof File) {
      const relative =
        normalizeRelPath(entry.relative || entry.file.webkitRelativePath || entry.file.name) ||
        entry.file.name;
      return { file: entry.file, relative };
    }
    const file = entry;
    const relative =
      normalizeRelPath(file.webkitRelativePath || file.name) || file.name;
    return { file, relative };
  });
}

function enqueueUploads(input, dir, directories = []) {
  const items = toUploadItems(input);
  const base = dir || state.cwd;
  const directorySet = new Set(
    (directories || []).map((relative) => normalizeRelPath(relative)).filter(Boolean),
  );

  for (const item of items) {
    let parent = dirnameOf(item.relative);
    while (parent) {
      directorySet.add(parent);
      parent = dirnameOf(parent);
    }
  }
  if (!items.length && !directorySet.size) return;

  if (directorySet.size) {
    void ensureDirectories(base, [...directorySet]);
  }

  for (const item of items) {
    const parent = dirnameOf(item.relative);
    uploadQueue.push({
      file: item.file,
      dir: parent ? joinPath(base, parent) : base,
      display: item.relative,
    });
  }
  $('#upload-panel').hidden = false;
  void pumpUploads();
}

async function ensureDirectories(base, relatives) {
  try {
    await api('/api/ensure_dirs', {
      paths: relatives.map((relative) => joinPath(base, relative)),
    });
  } catch {
    // Folder creation also happens lazily in upload_init; a batch failure here
    // is not fatal, individual uploads will surface the real error.
  }
}

async function collectDropItems(dataTransfer) {
  const items = [];
  const directories = [];
  const entries = [];
  for (const item of dataTransfer.items || []) {
    if (item.kind !== 'file') continue;
    const entry = item.webkitGetAsEntry ? item.webkitGetAsEntry() : null;
    if (entry) entries.push(entry);
  }
  if (!entries.length) {
    for (const file of dataTransfer.files || []) {
      items.push({ file, relative: file.name });
    }
    return { items, directories };
  }
  for (const entry of entries) {
    await walkEntry(entry, '', items, directories);
  }
  return { items, directories };
}

async function walkEntry(entry, prefix, items, directories) {
  if (entry.isFile) {
    const file = await new Promise((resolve, reject) => entry.file(resolve, reject));
    const relative = prefix ? `${prefix}/${file.name}` : file.name;
    items.push({ file, relative });
    return;
  }
  if (!entry.isDirectory) return;
  const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
  directories.push(relative);
  const reader = entry.createReader();
  while (true) {
    const batch = await new Promise((resolve, reject) => reader.readEntries(resolve, reject));
    if (!batch.length) break;
    for (const child of batch) {
      await walkEntry(child, relative, items, directories);
    }
  }
}

async function pumpUploads() {
  if (uploadRunning) return;
  uploadRunning = true;
  try {
    while (uploadQueue.length) {
      const item = uploadQueue.shift();
      await uploadFile(item.file, item.dir, item.display);
    }
  } finally {
    uploadRunning = false;
  }
}

async function uploadFile(file, dir, display) {
  const item = addUploadItem(display || file.name, file.size, dir);
  let uploadId = null;
  try {
    const init = await api('/api/upload/init', {
      dir,
      name: file.name,
      size: file.size,
    });
    uploadId = init.id;

    const chunkSize = 2 * 1024 * 1024;
    let offset = 0;
    let sequence = 0;
    while (offset < file.size) {
      const end = Math.min(offset + chunkSize, file.size);
      const plaintext = new Uint8Array(await file.slice(offset, end).arrayBuffer());
      const iv = randomBytes(12);
      const path = `/api/upload/chunk/${encodeURIComponent(uploadId)}/${sequence}`;
      const aad = `PUT ${path}`;
      const ciphertext = new Uint8Array(
        await crypto.subtle.encrypt(
          { name: 'AES-GCM', iv, additionalData: encoder.encode(aad) },
          state.key,
          plaintext,
        ),
      );
      const frame = makeFrame(iv, ciphertext);
      const response = await fetch(path, {
        method: 'PUT',
        headers: {
          'Content-Type': 'application/octet-stream',
          Authorization: `Bearer ${state.token}`,
        },
        body: frame,
      });
      let data = null;
      try {
        data = await response.json();
      } catch {
        data = null;
      }
      if (!response.ok) {
        if (response.status === 401) handleSessionLost('会话已过期，请重新登录');
        throw new Error(data && data.error ? data.error : `上传失败 (${response.status})`);
      }
      await decryptPayload(aad, data);

      offset = end;
      sequence += 1;
      item.update(
        file.size === 0 ? 1 : offset / file.size,
        `${formatSize(offset)} / ${formatSize(file.size)}`,
      );
    }

    const result = await api('/api/upload/finish', { id: uploadId });
    uploadId = null;
    item.done(result.path || file.name);
    toast(`已上传：${display || file.name}`, 'success');
    if (dir === state.cwd || dir.startsWith(`${state.cwd === '/' ? '' : state.cwd}/`)) {
      await loadDir(state.cwd);
    }
  } catch (error) {
    const message = error.message || '上传失败';
    item.fail(message);
    toast(message, 'error');
    if (uploadId && state.token) {
      try {
        await api('/api/upload/abort', { id: uploadId });
      } catch {
        /* best effort cleanup */
      }
    }
  }
}

function makeFrame(iv, ciphertext) {
  const frame = new Uint8Array(4 + 12 + ciphertext.length);
  new DataView(frame.buffer).setUint32(0, ciphertext.length, false);
  frame.set(iv, 4);
  frame.set(ciphertext, 16);
  return frame;
}

function addUploadItem(name, size, dir) {
  $('#upload-panel').hidden = false;
  const wrapper = div('upload-item');
  const head = div('upload-item-head');
  const title = document.createElement('strong');
  title.textContent = name;
  title.title = name;
  const meta = span('upload-meta');
  meta.textContent = `${formatSize(size)} · ${dir === '/' ? '根目录' : dir}`;
  head.append(title, meta);
  const bar = document.createElement('progress');
  bar.max = 100;
  bar.value = 0;
  const status = span('upload-status');
  status.textContent = '等待上传…';
  wrapper.append(head, bar, status);
  $('#upload-list').append(wrapper);

  wrapper.scrollIntoView({ block: 'nearest' });

  return {
    update(ratio, text) {
      bar.value = Math.max(0, Math.min(100, ratio * 100));
      status.textContent = text;
    },
    done(path) {
      bar.value = 100;
      status.textContent = `已完成 · ${path}`;
      wrapper.classList.add('done');
    },
    fail(message) {
      status.textContent = `失败 · ${message}`;
      wrapper.classList.add('failed');
    },
  };
}

function clearFinishedUploads() {
  for (const item of document.querySelectorAll('.upload-item.done, .upload-item.failed')) {
    item.remove();
  }
  if (!$('#upload-list').children.length) $('#upload-panel').hidden = true;
}

/* ---------- Toasts / dialogs ---------- */

function toast(message, kind = 'info') {
  const element = div(`toast ${kind}`);
  const text = span('toast-text');
  text.textContent = message;
  element.append(text);
  $('#toasts').append(element);
  setTimeout(() => element.remove(), kind === 'error' ? 6500 : 3800);
}

function progressToast(title) {
  const element = div('toast progress-toast');
  const head = div('toast-head');
  head.textContent = title;
  const bar = document.createElement('progress');
  bar.max = 100;
  bar.value = 0;
  const status = span('toast-status');
  status.textContent = '准备中…';
  element.append(head, bar, status);
  $('#toasts').append(element);

  return {
    update(ratio, text) {
      if (ratio === null || ratio === undefined) {
        bar.removeAttribute('value');
      } else {
        bar.value = Math.max(0, Math.min(100, ratio * 100));
      }
      if (text) status.textContent = text;
    },
    done(text) {
      status.textContent = text;
      element.classList.add('success');
      setTimeout(() => element.remove(), 3500);
    },
    fail(text) {
      status.textContent = text;
      element.classList.add('error');
      setTimeout(() => element.remove(), 6500);
    },
  };
}

function openModal(builder) {
  return new Promise((resolve) => {
    const dialog = $('#modal');
    dialog.replaceChildren();
    let settled = false;
    const finish = (value) => {
      if (settled) return;
      settled = true;
      dialog.close();
      dialog.replaceChildren();
      resolve(value);
    };
    dialog.oncancel = (event) => {
      event.preventDefault();
      finish(null);
    };
    dialog.onclose = () => {
      if (!settled) {
        settled = true;
        resolve(null);
      }
    };
    builder(dialog, finish);
    dialog.showModal();
  });
}

function askText({ title, label, value = '', placeholder = '', confirmText = '确定' }) {
  return openModal((dialog, finish) => {
    const body = div('modal-body');
    const heading = document.createElement('h2');
    heading.textContent = title;
    const field = document.createElement('label');
    field.className = 'field';
    const fieldLabel = document.createElement('span');
    fieldLabel.textContent = label;
    const input = document.createElement('input');
    input.type = 'text';
    input.value = value;
    input.placeholder = placeholder;
    input.autocomplete = 'off';
    input.spellcheck = false;
    field.append(fieldLabel, input);

    const actions = div('modal-actions');
    const cancel = buttonEl('取消', 'btn ghost');
    const confirm = buttonEl(confirmText, 'btn primary');
    cancel.addEventListener('click', () => finish(null));
    const submit = () => {
      const text = input.value.trim();
      if (!text) {
        input.classList.add('invalid');
        input.focus();
        return;
      }
      finish(text);
    };
    input.addEventListener('input', () => input.classList.remove('invalid'));
    input.addEventListener('keydown', (event) => {
      if (event.key === 'Enter') {
        event.preventDefault();
        submit();
      }
    });
    confirm.addEventListener('click', submit);
    actions.append(cancel, confirm);
    body.append(heading, field, actions);
    dialog.append(body);
    setTimeout(() => {
      input.focus();
      input.select();
    }, 0);
  });
}

function confirmDialog({ title, message, confirmText = '删除' }) {
  return openModal((dialog, finish) => {
    const body = div('modal-body');
    const heading = document.createElement('h2');
    heading.textContent = title;
    const text = document.createElement('p');
    text.className = 'modal-text';
    text.textContent = message;
    const actions = div('modal-actions');
    const cancel = buttonEl('取消', 'btn ghost');
    const confirm = buttonEl(confirmText, 'btn danger');
    cancel.addEventListener('click', () => finish(false));
    confirm.addEventListener('click', () => finish(true));
    actions.append(cancel, confirm);
    body.append(heading, text, actions);
    dialog.append(body);
    setTimeout(() => confirm.focus(), 0);
  }).then((value) => value === true);
}

/* ---------- Preview ---------- */

const scriptLoads = new Map();

function loadScript(src) {
  if (!scriptLoads.has(src)) {
    scriptLoads.set(
      src,
      new Promise((resolve, reject) => {
        const script = document.createElement('script');
        script.src = src;
        script.async = true;
        script.onload = () => resolve();
        script.onerror = () => {
          scriptLoads.delete(src);
          script.remove();
          reject(new Error('预览组件加载失败，请检查网络后重试'));
        };
        document.head.append(script);
      }),
    );
  }
  return scriptLoads.get(src);
}

function previewMessage(title, text) {
  const wrapper = div('preview-message');
  const heading = document.createElement('strong');
  heading.textContent = title;
  wrapper.append(heading);
  if (text) {
    const detail = span('');
    detail.textContent = text;
    wrapper.append(detail);
  }
  return wrapper;
}

let previewController = null;

function visibleImageEntries() {
  const filter = state.filter.trim().toLowerCase();
  return state.entries.filter((entry) => {
    if (entry.is_dir) return false;
    if (filter && !entry.name.toLowerCase().includes(filter)) return false;
    const kind = previewKindOf(entry.name);
    return kind === 'image' || kind === 'svg';
  });
}

function handlePreviewKeys(event) {
  const controller = previewController;
  if (!controller) return;
  const dialog = document.querySelector('#modal');
  if (!dialog || !dialog.open) return;

  const target = event.target;
  const tag = target && target.tagName ? target.tagName.toLowerCase() : '';
  if (tag === 'input' || tag === 'textarea' || tag === 'select') return;
  if (target && target.isContentEditable) return;
  if (event.altKey || event.ctrlKey || event.metaKey) return;

  if (controller.kind === 'video') {
    const video = controller.video;
    if (!video || !video.isConnected) return;
    // When the video itself has focus the browser's native controls already
    // implement these shortcuts; handling them here too would double-seek.
    if (target === video) return;

    if (event.code === 'Space' || event.key === ' ') {
      event.preventDefault();
      if (event.repeat) return;
      if (video.paused) {
        const promise = video.play();
        if (promise && typeof promise.catch === 'function') promise.catch(() => {});
      } else {
        video.pause();
      }
    } else if (event.code === 'ArrowLeft') {
      event.preventDefault();
      video.currentTime = Math.max(0, video.currentTime - 5);
    } else if (event.code === 'ArrowRight') {
      event.preventDefault();
      const duration = Number.isFinite(video.duration)
        ? video.duration
        : Number.MAX_SAFE_INTEGER;
      video.currentTime = Math.min(duration, video.currentTime + 5);
    }
    return;
  }

  if (
    (controller.kind === 'image' || controller.kind === 'svg') &&
    controller.images.length > 1
  ) {
    if (event.code === 'ArrowLeft' || event.code === 'ArrowRight') {
      event.preventDefault();
      const count = controller.images.length;
      const delta = event.code === 'ArrowLeft' ? -1 : 1;
      const index = (controller.index + delta + count) % count;
      void controller.show(controller.images[index]);
    }
  }
}

function openPreview(entry) {
  recordHistory('file', joinPath(state.cwd, entry.name), entry.name, state.cwd);
  const initialKind = previewKindOf(entry.name);
  const isImage = initialKind === 'image' || initialKind === 'svg';
  let images = isImage ? visibleImageEntries() : [];
  if (isImage && !images.some((item) => item.name === entry.name)) {
    images = [entry];
  }

  return openModal((dialog, finish) => {
    dialog.classList.add('preview-dialog');
    dialog.setAttribute('tabindex', '-1');

    const header = div('preview-head');
    const info = div('preview-title');
    const heading = document.createElement('strong');
    const meta = span('preview-meta');
    const hint = span('preview-hint');
    info.append(heading, meta, hint);

    const tools = div('preview-tools');
    const downloadButton = buttonEl('下载', 'btn');
    const closeButton = buttonEl('', 'btn ghost icon-only');
    closeButton.innerHTML = ICONS.close;
    closeButton.title = '关闭预览';
    closeButton.setAttribute('aria-label', '关闭预览');
    closeButton.addEventListener('click', () => finish(null));
    tools.append(downloadButton, closeButton);
    header.append(info, tools);

    const content = div('preview-content');
    dialog.append(header, content);

    let currentEntry = entry;
    let renderGeneration = 0;
    let objectUrls = [];
    const trackUrl = (url) => {
      objectUrls.push(url);
      return url;
    };
    const releaseObjectUrls = () => {
      for (const url of objectUrls) URL.revokeObjectURL(url);
      objectUrls = [];
    };
    const controller = {
      dialog,
      kind: initialKind,
      video: null,
      images,
      index: Math.max(0, images.findIndex((item) => item.name === entry.name)),
      show: null,
    };

    const updateHeader = (nextEntry, nextKind) => {
      heading.textContent = nextEntry.name;
      header.title = nextEntry.name;
      meta.textContent = `${formatSize(nextEntry.size)} · ${formatDate(nextEntry.modified)}`;
      if (nextKind === 'video') {
        hint.textContent = '← / → 快退或快进 5 秒 · 空格 播放/暂停';
      } else if (
        (nextKind === 'image' || nextKind === 'svg') &&
        controller.images.length > 1
      ) {
        hint.textContent = `第 ${controller.index + 1} / ${controller.images.length} 张 · ← / → 切换图片`;
      } else {
        hint.textContent = '';
      }
    };

    const showEntry = async (nextEntry) => {
      const nextKind = previewKindOf(nextEntry.name);
      currentEntry = nextEntry;
      controller.kind = nextKind;
      controller.video = null;
      const index = controller.images.findIndex((item) => item.name === nextEntry.name);
      if (index >= 0) controller.index = index;

      updateHeader(nextEntry, nextKind);
      downloadButton.onclick = () =>
        startDownload(joinPath(state.cwd, currentEntry.name), currentEntry.name);

      const generation = ++renderGeneration;
      releaseObjectUrls();
      content.className = 'preview-content';
      const loading = div('preview-loading');
      loading.textContent = '正在准备预览…';
      content.replaceChildren(loading);

      try {
        await renderPreview({ content, entry: nextEntry, kind: nextKind, trackUrl });
        if (generation !== renderGeneration) return;
        controller.video = content.querySelector('video');
      } catch (error) {
        if (generation !== renderGeneration) return;
        content.classList.remove('media-mode');
        content.replaceChildren(
          previewMessage('预览失败', error.message || '无法预览该文件'),
        );
      }
    };

    controller.show = showEntry;
    previewController = controller;

    dialog.addEventListener(
      'close',
      () => {
        if (previewController === controller) previewController = null;
        releaseObjectUrls();
      },
      { once: true },
    );

    void showEntry(entry);
    setTimeout(() => {
      try {
        dialog.focus();
      } catch {
        /* focus is best-effort */
      }
    }, 0);
  });
}

function mediaStatus(text) {
  const status = div('preview-media-status');
  status.textContent = text;
  return status;
}

function previewProgress(wrapper) {
  const box = div('preview-progress');
  const bar = document.createElement('progress');
  bar.max = 100;
  bar.value = 0;
  const label = span('preview-progress-label');
  label.textContent = '正在接收…';
  box.append(bar, label);
  wrapper.append(box);
  return {
    update(received, total) {
      if (total > 0) bar.value = Math.min(100, (received / total) * 100);
      label.textContent =
        total > 0
          ? `正在接收 ${formatSize(received)} / ${formatSize(total)}`
          : `正在接收 ${formatSize(received)}`;
    },
    parse(labelText) {
      bar.removeAttribute('value');
      label.textContent = labelText;
    },
    remove() {
      box.remove();
    },
  };
}

function previewByteLimit(kind) {
  switch (kind) {
    case 'image':
    case 'svg':
      return IMAGE_PREVIEW_BYTES;
    case 'pdf':
      return PDF_PREVIEW_BYTES;
    case 'html':
      return HTML_PREVIEW_BYTES;
    case 'video':
    case 'audio':
      return MEDIA_PREVIEW_BYTES;
    case 'docx':
    case 'sheet':
      return OFFICE_PREVIEW_BYTES;
    default:
      return 0;
  }
}

function previewTooLarge(content, limit, action) {
  content.replaceChildren(
    previewMessage(
      '文件过大，无法在线预览',
      `加密预览需要先在浏览器内完整接收并解密，上限为 ${formatSize(limit)}，请下载后${action}。`,
    ),
  );
}

const PREVIEW_HTML_CSP =
  "default-src 'none'; style-src 'unsafe-inline'; img-src data: blob:; " +
  "media-src data: blob:; font-src data: blob:; script-src 'none'; " +
  "connect-src 'none'; base-uri 'none'; form-action 'none';";

/** Injects a restrictive CSP because a blob document no longer gets the
 * server's response headers. Scripts and network requests stay disabled. */
function securePreviewHtml(html) {
  const meta = `<meta http-equiv="Content-Security-Policy" content="${PREVIEW_HTML_CSP}">`;
  const head = /<head\b[^>]*>/i.exec(html);
  if (head) {
    const at = head.index + head[0].length;
    return html.slice(0, at) + meta + html.slice(at);
  }
  const htmlTag = /<html\b[^>]*>/i.exec(html);
  if (htmlTag) {
    const at = htmlTag.index + htmlTag[0].length;
    return `${html.slice(0, at)}<head>${meta}</head>${html.slice(at)}`;
  }
  return meta + html;
}

async function requestPreviewStream(path) {
  const endpoint = '/api/preview/stream';
  const aad = `POST ${endpoint}`;
  const envelope = await encryptPayload(aad, { path });
  const response = await fetch(endpoint, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${state.token}`,
    },
    body: JSON.stringify(envelope),
    cache: 'no-store',
  });
  if (!response.ok) {
    let message = null;
    try {
      const body = await response.json();
      message = body && body.error;
    } catch {
      message = null;
    }
    if (response.status === 401) handleSessionLost('会话已过期，请重新登录');
    throw new Error(message || `读取文件失败 (${response.status})`);
  }
  const streamId = response.headers.get('x-stream-id');
  if (!streamId || !response.body) throw new Error('服务器响应缺少加密流信息');
  return {
    reader: response.body.getReader(),
    streamId,
    size: Number(response.headers.get('x-file-size') || 0),
    chunkSize: Number(response.headers.get('x-chunk-size') || 65536),
    contentType:
      response.headers.get('x-preview-content-type') || 'application/octet-stream',
  };
}

async function readEncryptedPreview(source, onPlaintext) {
  if (!state.key) throw new Error('会话已失效，请重新登录');
  const { reader, streamId, chunkSize } = source;
  let pending = new Uint8Array(0);
  let sequence = 0;
  let received = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      pending = concatBytes(pending, value);
      while (pending.length >= 4) {
        const length = new DataView(
          pending.buffer,
          pending.byteOffset,
          4,
        ).getUint32(0, false);
        if (length > chunkSize + 16 + 1024) throw new Error('预览数据校验失败');
        if (pending.length < 16 + length) break;
        const iv = pending.subarray(4, 16);
        const ciphertext = pending.subarray(16, 16 + length);
        let plaintext;
        try {
          plaintext = new Uint8Array(
            await crypto.subtle.decrypt(
              {
                name: 'AES-GCM',
                iv,
                additionalData: encoder.encode(`pv:${streamId}:${sequence}`),
              },
              state.key,
              ciphertext,
            ),
          );
        } catch {
          throw new Error('预览数据解密失败，连接可能已被篡改');
        }
        sequence += 1;
        pending = pending.subarray(16 + length);
        received += plaintext.length;
        if (await onPlaintext(plaintext, received)) {
          await reader.cancel().catch(() => {});
          return { received, stopped: true };
        }
      }
    }
    if (pending.length !== 0) throw new Error('预览数据不完整');
    return { received, stopped: false };
  } catch (error) {
    await reader.cancel().catch(() => {});
    throw error;
  }
}

async function collectPreviewChunks(source, onProgress) {
  const parts = [];
  const result = await readEncryptedPreview(source, (chunk, received) => {
    parts.push(chunk);
    if (onProgress) onProgress(received, source.size);
    return false;
  });
  if (source.size > 0 && result.received !== source.size) {
    throw new Error('预览数据不完整，请重试');
  }
  return { parts, total: result.received };
}

function joinChunks(parts, total) {
  const bytes = new Uint8Array(total);
  let offset = 0;
  for (const part of parts) {
    bytes.set(part, offset);
    offset += part.length;
  }
  return bytes;
}

function mediaProgress(status, received, total, label) {
  status.hidden = false;
  status.textContent =
    total > 0
      ? `${label} ${formatSize(received)} / ${formatSize(total)}`
      : `${label} ${formatSize(received)}`;
}

async function renderPreview({ content, entry, kind, trackUrl }) {
  if (kind === 'none') {
    content.replaceChildren(
      previewMessage('暂不支持在线预览此类型', '可以下载后使用本地应用打开。'),
    );
    return;
  }

  const limit = previewByteLimit(kind);
  if (limit && entry.size > limit) {
    previewTooLarge(
      content,
      limit,
      kind === 'video' || kind === 'audio' ? '播放' : '查看',
    );
    return;
  }

  const path = joinPath(state.cwd, entry.name);

  if (kind === 'image' || kind === 'svg') {
    content.classList.add('media-mode');
    const wrapper = div('preview-media-wrap');
    const status = mediaStatus('正在接收图片…');
    wrapper.append(status);
    content.replaceChildren(wrapper);

    const source = await requestPreviewStream(path);
    if (source.size > IMAGE_PREVIEW_BYTES) {
      await source.reader.cancel().catch(() => {});
      previewTooLarge(content, IMAGE_PREVIEW_BYTES, '查看');
      return;
    }
    const { parts, total } = await collectPreviewChunks(source, (received, size) => {
      mediaProgress(status, received, size || entry.size, '正在接收图片');
    });
    if (!wrapper.isConnected || total === 0) {
      if (total === 0 && wrapper.isConnected) {
        content.classList.remove('media-mode');
        content.replaceChildren(previewMessage('图片为空', ''));
      }
      return;
    }
    const type = source.contentType || 'application/octet-stream';
    const url = trackUrl(URL.createObjectURL(new Blob(parts, { type })));
    const image = document.createElement('img');
    image.className = 'preview-image';
    image.alt = entry.name;
    image.decoding = 'async';
    image.addEventListener('load', () => status.remove(), { once: true });
    image.addEventListener('error', () => {
      content.classList.remove('media-mode');
      content.replaceChildren(
        previewMessage('图片加载失败', '文件可能已损坏或格式不受支持。'),
      );
    });
    status.textContent = '正在解码图片…';
    image.src = url;
    wrapper.append(image);
    return;
  }

  if (kind === 'video') {
    content.classList.add('media-mode');
    const wrapper = div('preview-media-wrap');
    const status = mediaStatus('正在接收视频…');
    wrapper.append(status);
    content.replaceChildren(wrapper);

    const source = await requestPreviewStream(path);
    if (source.size > MEDIA_PREVIEW_BYTES) {
      await source.reader.cancel().catch(() => {});
      previewTooLarge(content, MEDIA_PREVIEW_BYTES, '播放');
      return;
    }
    const { parts, total } = await collectPreviewChunks(source, (received, size) => {
      mediaProgress(status, received, size || entry.size, '正在接收视频');
    });
    if (!wrapper.isConnected) return;
    if (total === 0) {
      content.classList.remove('media-mode');
      content.replaceChildren(previewMessage('视频为空', ''));
      return;
    }
    const url = trackUrl(
      URL.createObjectURL(new Blob(parts, { type: source.contentType || 'video/mp4' })),
    );
    const video = document.createElement('video');
    video.className = 'preview-video';
    video.controls = true;
    video.autoplay = true;
    video.playsInline = true;
    video.preload = 'metadata';
    const showStatus = (text) => {
      status.hidden = false;
      status.textContent = text;
    };
    const hideStatus = () => {
      status.hidden = true;
    };
    video.addEventListener('waiting', () => showStatus('正在缓冲…'));
    video.addEventListener('stalled', () => showStatus('正在缓冲…'));
    video.addEventListener('playing', hideStatus);
    video.addEventListener('canplay', hideStatus);
    video.addEventListener('error', () => {
      content.classList.remove('media-mode');
      content.replaceChildren(
        previewMessage(
          '浏览器无法播放该视频',
          '可能是编码格式不受支持（例如 H.265/HEVC），可下载后使用本地播放器。',
        ),
      );
    });
    status.textContent = '正在加载视频…';
    video.src = url;
    wrapper.append(video);
    return;
  }

  if (kind === 'audio') {
    content.classList.add('media-mode');
    const wrapper = div('preview-media-wrap');
    const status = mediaStatus('正在接收音频…');
    wrapper.append(status);
    content.replaceChildren(wrapper);

    const source = await requestPreviewStream(path);
    if (source.size > MEDIA_PREVIEW_BYTES) {
      await source.reader.cancel().catch(() => {});
      previewTooLarge(content, MEDIA_PREVIEW_BYTES, '播放');
      return;
    }
    const { parts, total } = await collectPreviewChunks(source, (received, size) => {
      mediaProgress(status, received, size || entry.size, '正在接收音频');
    });
    if (!wrapper.isConnected) return;
    if (total === 0) {
      content.classList.remove('media-mode');
      content.replaceChildren(previewMessage('音频为空', ''));
      return;
    }
    const url = trackUrl(
      URL.createObjectURL(new Blob(parts, { type: source.contentType || 'audio/mpeg' })),
    );
    const audio = document.createElement('audio');
    audio.className = 'preview-audio';
    audio.controls = true;
    audio.autoplay = true;
    audio.addEventListener('waiting', () => {
      status.hidden = false;
      status.textContent = '正在缓冲…';
    });
    audio.addEventListener('stalled', () => {
      status.hidden = false;
      status.textContent = '正在缓冲…';
    });
    audio.addEventListener('playing', () => {
      status.hidden = true;
    });
    audio.addEventListener('canplay', () => {
      status.hidden = true;
    });
    status.textContent = '正在加载音频…';
    audio.src = url;
    wrapper.append(audio);
    return;
  }

  if (kind === 'pdf') {
    content.classList.add('media-mode');
    const wrapper = div('preview-media-wrap');
    const status = mediaStatus('正在接收 PDF…');
    wrapper.append(status);
    content.replaceChildren(wrapper);

    const source = await requestPreviewStream(path);
    if (source.size > PDF_PREVIEW_BYTES) {
      await source.reader.cancel().catch(() => {});
      previewTooLarge(content, PDF_PREVIEW_BYTES, '查看');
      return;
    }
    const { parts, total } = await collectPreviewChunks(source, (received, size) => {
      mediaProgress(status, received, size || entry.size, '正在接收 PDF');
    });
    if (!wrapper.isConnected) return;
    if (total === 0) {
      content.classList.remove('media-mode');
      content.replaceChildren(previewMessage('PDF 为空', ''));
      return;
    }
    const frame = document.createElement('iframe');
    frame.className = 'preview-frame';
    frame.title = entry.name;
    const url = trackUrl(
      URL.createObjectURL(new Blob(parts, { type: 'application/pdf' })),
    );
    status.textContent = '正在加载 PDF…';
    frame.addEventListener('load', () => status.remove(), { once: true });
    frame.src = url;
    wrapper.append(frame);
    return;
  }

  if (kind === 'html') {
    content.classList.add('media-mode');
    const note = div('preview-banner');
    note.textContent = 'HTML 在隔离沙箱中静态渲染：脚本与外部网络请求已禁用。';
    const frameWrap = div('preview-media-wrap');
    const status = mediaStatus('正在接收 HTML…');
    frameWrap.append(status);
    const wrapper = div('preview-stack');
    wrapper.append(note, frameWrap);
    content.replaceChildren(wrapper);

    const source = await requestPreviewStream(path);
    if (source.size > HTML_PREVIEW_BYTES) {
      await source.reader.cancel().catch(() => {});
      previewTooLarge(content, HTML_PREVIEW_BYTES, '查看');
      return;
    }
    const { parts, total } = await collectPreviewChunks(source, (received, size) => {
      mediaProgress(status, received, size || entry.size, '正在接收 HTML');
    });
    if (!wrapper.isConnected) return;
    const html = securePreviewHtml(
      new TextDecoder('utf-8', { fatal: false }).decode(joinChunks(parts, total)),
    );
    const frame = document.createElement('iframe');
    frame.className = 'preview-frame';
    frame.title = entry.name;
    frame.setAttribute('sandbox', '');
    frame.setAttribute('referrerpolicy', 'no-referrer');
    const url = trackUrl(
      URL.createObjectURL(new Blob([html], { type: 'text/html;charset=utf-8' })),
    );
    status.textContent = '正在加载 HTML…';
    frame.addEventListener('load', () => status.remove(), { once: true });
    frame.src = url;
    frameWrap.append(frame);
    return;
  }

  if (kind === 'text') {
    const wrapper = div('preview-stack');
    const banner = div('preview-banner');
    banner.textContent = '正在接收…';
    const pre = document.createElement('pre');
    pre.className = 'preview-text';
    const textNode = document.createTextNode('');
    pre.append(textNode);
    wrapper.append(banner, pre);
    content.replaceChildren(wrapper);

    const source = await requestPreviewStream(path);
    const total = source.size || entry.size || 0;
    const textDecoder = new TextDecoder('utf-8', { fatal: false });
    let received = 0;
    let truncated = false;
    let lastPaint = 0;
    await readEncryptedPreview(source, (chunk, count) => {
      if (!wrapper.isConnected) return true;
      received = count;
      textNode.appendData(textDecoder.decode(chunk, { stream: true }));
      const now = performance.now();
      if (now - lastPaint > 120) {
        lastPaint = now;
        banner.textContent =
          total > 0
            ? `正在接收 ${formatSize(received)} / ${formatSize(total)}（内容已可边收边看）`
            : `正在接收 ${formatSize(received)}（内容已可边收边看）`;
      }
      if (received >= TEXT_STREAM_BYTES) {
        truncated = true;
        return true;
      }
      return false;
    });
    if (!wrapper.isConnected) return;
    textNode.appendData(textDecoder.decode());
    banner.textContent = truncated
      ? `文件较大，仅显示前 ${formatSize(TEXT_STREAM_BYTES)}（已接收 ${formatSize(received)}），完整内容请下载查看。`
      : total > 0 && received >= total
        ? `已接收全部 ${formatSize(received)}`
        : `已接收 ${formatSize(received)}`;
    return;
  }

  if (kind === 'docx') {
    const wrapper = div('preview-stack');
    const documentView = div('preview-docx');
    const progress = previewProgress(wrapper);
    wrapper.append(documentView);
    content.replaceChildren(wrapper);
    await loadScript('/vendor/mammoth.js');

    const source = await requestPreviewStream(path);
    if (source.size > OFFICE_PREVIEW_BYTES) {
      await source.reader.cancel().catch(() => {});
      previewTooLarge(content, OFFICE_PREVIEW_BYTES, '查看');
      return;
    }
    const { parts, total } = await collectPreviewChunks(source, (received, size) => {
      progress.update(received, size || entry.size);
    });
    if (!wrapper.isConnected) return;
    progress.parse('正在解析文档…');
    const bytes = joinChunks(parts, total);
    const result = await window.mammoth.convertToHtml({ arrayBuffer: bytes.buffer });
    documentView.append(sanitizeHtml(result.value || ''));
    if (!documentView.textContent.trim() && !documentView.querySelector('img, table')) {
      documentView.append(previewMessage('文档没有可显示的文本内容', ''));
    }
    progress.remove();
    return;
  }

  if (kind === 'sheet') {
    const wrapper = div('preview-stack');
    const progress = previewProgress(wrapper);
    content.replaceChildren(wrapper);
    await loadScript('/vendor/xlsx.js');

    const source = await requestPreviewStream(path);
    if (source.size > OFFICE_PREVIEW_BYTES) {
      await source.reader.cancel().catch(() => {});
      previewTooLarge(content, OFFICE_PREVIEW_BYTES, '查看');
      return;
    }
    const { parts, total } = await collectPreviewChunks(source, (received, size) => {
      progress.update(received, size || entry.size);
    });
    if (!wrapper.isConnected) return;
    progress.parse('正在解析表格…');
    const bytes = joinChunks(parts, total);
    const extension = fileExtension(entry.name);
    let workbook;
    if (extension === 'csv' || extension === 'tsv') {
      workbook = window.XLSX.read(new TextDecoder('utf-8').decode(bytes), {
        type: 'string',
        raw: false,
      });
    } else {
      workbook = window.XLSX.read(bytes, { type: 'array' });
    }
    progress.remove();
    wrapper.append(renderWorkbook(workbook));
    return;
  }

  content.replaceChildren(previewMessage('暂不支持在线预览此类型', '可以下载后使用本地应用打开。'));
}

const ALLOWED_PREVIEW_TAGS = new Set([
  'a', 'b', 'blockquote', 'br', 'caption', 'code', 'del', 'div', 'em', 'h1',
  'h2', 'h3', 'h4', 'h5', 'h6', 'hr', 'i', 'img', 'ins', 'li', 'ol', 'p',
  'pre', 's', 'span', 'strong', 'sub', 'sup', 'table', 'tbody', 'td', 'tfoot',
  'th', 'thead', 'tr', 'u', 'ul',
]);

const ALLOWED_PREVIEW_ATTRS = {
  a: new Set(['href', 'title']),
  img: new Set(['src', 'alt', 'title']),
  td: new Set(['colspan', 'rowspan']),
  th: new Set(['colspan', 'rowspan']),
};

function sanitizeHtml(html) {
  const template = document.createElement('template');
  template.innerHTML = html;
  const walker = document.createTreeWalker(template.content, NodeFilter.SHOW_ELEMENT);
  const doomed = [];
  while (walker.nextNode()) {
    const element = walker.currentNode;
    const tag = element.tagName.toLowerCase();
    if (!ALLOWED_PREVIEW_TAGS.has(tag)) {
      doomed.push(element);
      continue;
    }
    const allowed = ALLOWED_PREVIEW_ATTRS[tag] || new Set();
    for (const attribute of [...element.attributes]) {
      if (!allowed.has(attribute.name.toLowerCase())) {
        element.removeAttribute(attribute.name);
      }
    }
    if (tag === 'a') {
      const href = element.getAttribute('href') || '';
      if (/^https?:/i.test(href)) {
        element.setAttribute('target', '_blank');
        element.setAttribute('rel', 'noopener noreferrer');
      } else {
        element.removeAttribute('href');
      }
    }
    if (tag === 'img') {
      const src = element.getAttribute('src') || '';
      if (!/^data:image\//i.test(src)) {
        element.remove();
        continue;
      }
      element.removeAttribute('width');
      element.removeAttribute('height');
    }
    for (const attribute of [...element.attributes]) {
      if (attribute.name.toLowerCase().startsWith('on')) {
        element.removeAttribute(attribute.name);
      }
    }
  }
  for (const element of doomed) element.remove();
  return template.content;
}

function renderWorkbook(workbook) {
  const wrapper = div('preview-stack');
  const sheetNames = (workbook.SheetNames || []).slice(0, 20);
  if (!sheetNames.length) {
    wrapper.append(previewMessage('工作簿为空', ''));
    return wrapper;
  }

  const tabs = div('sheet-tabs');
  const panel = div('sheet-panel');
  const note = div('preview-banner');
  let active = 0;

  const draw = () => {
    panel.replaceChildren();
    note.textContent = '';
    for (const [index, button] of [...tabs.children].entries()) {
      button.classList.toggle('active', index === active);
    }
    const sheet = workbook.Sheets[sheetNames[active]];
    if (!sheet) {
      panel.append(previewMessage('工作表为空', ''));
      return;
    }
    const rows = window.XLSX.utils.sheet_to_json(sheet, {
      header: 1,
      raw: false,
      defval: '',
      blankrows: false,
    });
    if (!rows.length) {
      panel.append(previewMessage('工作表为空', ''));
      return;
    }
    let columns = 0;
    for (const row of rows.slice(0, 50)) {
      if (row && row.length > columns) columns = row.length;
    }
    columns = Math.min(columns, SHEET_MAX_COLS);
    const limit = Math.min(rows.length, SHEET_MAX_ROWS);
    const table = document.createElement('table');
    table.className = 'sheet-table';
    for (let rowIndex = 0; rowIndex < limit; rowIndex += 1) {
      const row = rows[rowIndex] || [];
      const tr = document.createElement('tr');
      for (let column = 0; column < columns; column += 1) {
        const cell = document.createElement('td');
        const value = row[column];
        cell.textContent = value == null ? '' : String(value);
        tr.append(cell);
      }
      table.append(tr);
    }
    panel.append(table);
    if (rows.length > limit) {
      note.textContent += `仅显示前 ${limit} 行（共 ${rows.length} 行）。`;
    }
    if (columns >= SHEET_MAX_COLS) {
      note.textContent += ` 仅显示前 ${SHEET_MAX_COLS} 列。`;
    }
  };

  if (sheetNames.length > 1) {
    for (const [index, name] of sheetNames.entries()) {
      const button = buttonEl(name || `Sheet${index + 1}`, 'sheet-tab');
      button.title = name;
      button.addEventListener('click', () => {
        active = index;
        draw();
      });
      tabs.append(button);
    }
    wrapper.append(tabs);
  }
  wrapper.append(note, panel);
  draw();
  return wrapper;
}

/* ---------- Account ---------- */

function labeledInput(labelText, type, autocomplete) {
  const field = document.createElement('label');
  field.className = 'field';
  const label = span('');
  label.textContent = labelText;
  const input = document.createElement('input');
  input.type = type;
  input.autocomplete = autocomplete;
  input.spellcheck = false;
  field.append(label, input);
  return { field, input };
}

function openAccountDialog() {
  return openModal((dialog, finish) => {
    dialog.classList.add('account-dialog');
    const body = div('modal-body');
    const heading = document.createElement('h2');
    heading.textContent = '账户设置';

    const hint = document.createElement('p');
    hint.className = 'modal-text';
    hint.textContent =
      '在这里修改登录用户名和密码。修改成功后，其他设备上的登录会立即失效，当前会话保持登录。新密码留空表示只修改用户名。';

    const current = labeledInput('当前密码', 'password', 'current-password');
    const username = labeledInput('用户名', 'text', 'username');
    username.input.value = state.user || '';
    const next = labeledInput('新密码（至少 8 位，留空则不修改）', 'password', 'new-password');
    const confirm = labeledInput('确认新密码', 'password', 'new-password');

    const errorEl = div('form-error inline-error');
    const actions = div('modal-actions');
    const cancel = buttonEl('取消', 'btn ghost');
    const save = buttonEl('保存修改', 'btn primary');

    const submit = async () => {
      errorEl.textContent = '';
      const currentValue = current.input.value;
      const usernameValue = username.input.value.trim();
      const passwordValue = next.input.value;
      const confirmValue = confirm.input.value;

      if (!currentValue) {
        errorEl.textContent = '请输入当前密码';
        current.input.focus();
        return;
      }
      if (!usernameValue) {
        errorEl.textContent = '用户名不能为空';
        username.input.focus();
        return;
      }
      if (passwordValue && passwordValue !== confirmValue) {
        errorEl.textContent = '两次输入的新密码不一致';
        confirm.input.focus();
        return;
      }
      if (passwordValue && passwordValue.length < 8) {
        errorEl.textContent = '新密码至少需要 8 个字符';
        next.input.focus();
        return;
      }

      save.disabled = true;
      save.textContent = '正在保存…';
      try {
        const result = await api('/api/account/update', {
          current_password: currentValue,
          new_username: usernameValue,
          new_password: passwordValue,
        });
        state.user = result.user;
        $('#user-badge').textContent = result.user;
        if (result.password_changed) {
          toast('用户名和密码已更新', 'success');
        } else {
          toast('用户名已更新', 'success');
        }
        finish(true);
      } catch (error) {
        errorEl.textContent = error.message || '修改失败';
      } finally {
        save.disabled = false;
        save.textContent = '保存修改';
      }
    };

    cancel.addEventListener('click', () => finish(null));
    save.addEventListener('click', () => void submit());
    for (const field of [current, username, next, confirm]) {
      field.input.addEventListener('keydown', (event) => {
        if (event.key === 'Enter') {
          event.preventDefault();
          void submit();
        }
      });
    }

    actions.append(cancel, save);
    body.append(
      heading,
      hint,
      current.field,
      username.field,
      next.field,
      confirm.field,
      errorEl,
      actions,
    );
    dialog.append(body);
    setTimeout(() => current.input.focus(), 0);
  });
}

/* ---------- History ---------- */

function basenameOf(path) {
  const clean = normalizeRelPath(path);
  if (!clean) return '根目录';
  return clean.slice(clean.lastIndexOf('/') + 1);
}

function parentOf(path) {
  const clean = normalizeRelPath(path);
  if (!clean) return '/';
  const index = clean.lastIndexOf('/');
  return index === -1 ? '/' : `/${clean.slice(0, index)}`;
}

function relativeTime(milliseconds) {
  const delta = Date.now() - Number(milliseconds || 0);
  if (!Number.isFinite(delta) || delta < 0) return '';
  const minute = 60_000;
  const hour = 60 * minute;
  const day = 24 * hour;
  if (delta < minute) return '刚刚';
  if (delta < hour) return `${Math.floor(delta / minute)} 分钟前`;
  if (delta < day) return `${Math.floor(delta / hour)} 小时前`;
  if (delta < 2 * day) return '昨天';
  if (delta < 7 * day) return `${Math.floor(delta / day)} 天前`;
  return new Date(milliseconds).toLocaleDateString('zh-CN');
}

function readHistory() {
  try {
    const raw = localStorage.getItem(HISTORY_KEY);
    const parsed = raw ? JSON.parse(raw) : [];
    if (!Array.isArray(parsed)) return [];
    return parsed.filter(
      (item) =>
        item &&
        (item.type === 'dir' || item.type === 'file') &&
        typeof item.path === 'string' &&
        typeof item.name === 'string' &&
        typeof item.at === 'number',
    );
  } catch {
    return [];
  }
}

function writeHistory(items) {
  try {
    localStorage.setItem(HISTORY_KEY, JSON.stringify(items.slice(0, HISTORY_LIMIT)));
  } catch {
    /* storage may be unavailable (private mode); history is best effort */
  }
}

function recordHistory(type, path, name, parent) {
  const clean = normalizeRelPath(path);
  if (!clean && type !== 'dir') return;
  const entry = {
    type,
    path: clean ? `/${clean}` : '/',
    name: name || basenameOf(path),
    parent: parent || parentOf(path),
    at: Date.now(),
  };
  const items = readHistory().filter(
    (item) => !(item.type === entry.type && item.path === entry.path),
  );
  items.unshift(entry);
  writeHistory(items);
}

function openHistoryDialog() {
  const items = readHistory();
  return openModal((dialog, finish) => {
    dialog.classList.add('history-dialog');
    const body = div('modal-body');
    const head = div('history-head');
    const heading = document.createElement('h2');
    heading.textContent = '浏览历史';
    const clear = buttonEl('清空', 'btn ghost small');
    clear.disabled = items.length === 0;
    clear.addEventListener('click', () => {
      writeHistory([]);
      finish(null);
      toast('历史记录已清空', 'info');
    });
    head.append(heading, clear);
    body.append(head);

    if (!items.length) {
      const empty = div('history-empty');
      empty.textContent = '还没有浏览记录。打开文件夹或预览文件后会出现在这里。';
      body.append(empty);
    } else {
      const list = div('history-list');
      for (const item of items.slice(0, 100)) {
        const row = buttonEl('', 'history-item');
        const icon = span('history-icon');
        icon.innerHTML = item.type === 'dir' ? ICONS.folder : ICONS.file;
        const info = div('history-info');
        const title = span('history-name');
        title.textContent = item.name;
        const meta = span('history-meta');
        meta.textContent = `${item.type === 'dir' ? '文件夹' : '文件'} · ${
          item.parent === '/' ? '根目录' : item.parent
        }`;
        info.append(title, meta);
        const time = span('history-time');
        time.textContent = relativeTime(item.at);
        row.append(icon, info, time);
        row.addEventListener('click', () => {
          finish(null);
          void jumpToHistory(item);
        });
        list.append(row);
      }
      body.append(list);
    }

    const actions = div('modal-actions');
    const close = buttonEl('关闭', 'btn ghost');
    close.addEventListener('click', () => finish(null));
    actions.append(close);
    body.append(actions);
    dialog.append(body);
  });
}

async function jumpToHistory(item) {
  if (item.type === 'dir') {
    await loadDir(item.path);
    return;
  }
  state.pendingHighlight = item.path;
  await loadDir(item.parent || '/');
}

/* ---------- Desktop app mode ---------- */

function isLocalTarget() {
  return Boolean(state.activeTarget && state.activeTarget.kind === 'local');
}

function showAppView() {
  $('#login-view').hidden = true;
  $('#setup-view').hidden = true;
  $('#app-view').hidden = false;
  $('#app-bar').hidden = !state.app;
}

function showLoginView() {
  $('#setup-view').hidden = true;
  $('#app-view').hidden = true;
  $('#login-view').hidden = false;
  $('#login-servers').hidden = !state.app;
}

function showSetupView() {
  $('#login-view').hidden = true;
  $('#app-view').hidden = true;
  $('#setup-view').hidden = false;
}

async function boot() {
  if (!window.crypto || !window.crypto.subtle) {
    showLoginError('当前页面不是安全上下文（需要 HTTPS），无法进行加密登录');
    return;
  }
  let info = null;
  try {
    const response = await fetch('/app/info', { cache: 'no-store' });
    if (response.ok) info = await response.json();
  } catch {
    info = null;
  }
  if (!info || !info.mode) return; // standalone server: normal login flow

  state.app = info;
  $('#app-bar').hidden = false;
  $('#app-version').textContent = `v${info.version || ''}`;
  $('#login-servers').hidden = false;
  renderAppBar();

  if (!info.active) {
    showSetupView();
    return;
  }
  const target =
    (info.targets || []).find((item) => item.id === info.active) || null;
  state.activeTarget = target;

  if (target && target.kind === 'local') {
    await enterLocalMode();
  } else {
    showLoginView();
  }
}

function renderAppBar() {
  const info = state.app;
  if (!info) return;
  const select = $('#target-select');
  select.replaceChildren();
  const targets = info.targets || [];
  if (!targets.length) {
    const option = document.createElement('option');
    option.value = '';
    option.textContent = '未配置服务器';
    select.append(option);
  }
  for (const target of targets) {
    const option = document.createElement('option');
    option.value = target.id;
    option.textContent =
      target.kind === 'local' ? '本机' : target.name || target.url || '服务器';
    if (target.id === info.active) option.selected = true;
    select.append(option);
  }
  renderRootSelect();
}

function renderRootSelect() {
  const info = state.app;
  const wrap = $('#local-root-wrap');
  if (!info || !info.local || !Array.isArray(info.local.roots)) {
    wrap.hidden = true;
    return;
  }
  const target = (info.targets || []).find((item) => item.id === info.active);
  if (!target || target.kind !== 'local') {
    wrap.hidden = true;
    return;
  }
  wrap.hidden = false;
  const select = $('#root-select');
  select.replaceChildren();
  for (const root of info.local.roots) {
    const option = document.createElement('option');
    option.value = root.path;
    option.textContent = root.name;
    if (root.path === info.local.root) option.selected = true;
    select.append(option);
  }
}

async function enterLocalMode() {
  try {
    const response = await fetch('/api/local-login', { cache: 'no-store' });
    if (!response.ok) throw new Error('本机服务未就绪');
    const data = await response.json();
    state.key = await importAesKey(base64ToBytes(data.key));
    state.token = data.token;
    state.user = data.user || 'local';
    state.activeTarget = (state.app.targets || []).find(
      (item) => item.kind === 'local',
    ) || { kind: 'local' };
    showAppView();
    $('#user-badge').textContent = '本机';
    $('#btn-account').hidden = true;
    $('#btn-logout').hidden = true;
    await loadDir('/');
  } catch (error) {
    showLoginView();
    showLoginError(error.message || '无法进入本机模式');
  }
}

async function refreshAppInfo() {
  try {
    const response = await fetch('/app/info', { cache: 'no-store' });
    if (!response.ok) return null;
    state.app = await response.json();
    renderAppBar();
    return state.app;
  } catch {
    return null;
  }
}

async function serversAction(payload) {
  const response = await fetch('/app/servers', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(payload),
  });
  let data = null;
  try {
    data = await response.json();
  } catch {
    data = null;
  }
  if (!response.ok) {
    throw new Error(data && data.error ? data.error : '操作失败');
  }
  return data;
}

function openServersDialog() {
  const activeAtOpen = state.app ? state.app.active : null;

  return openModal((dialog, finish) => {
    dialog.classList.add('servers-dialog');
    const body = div('modal-body');
    const heading = document.createElement('h2');
    heading.textContent = '服务器连接';
    const hint = document.createElement('p');
    hint.className = 'modal-text';
    hint.textContent =
      '可以保存多台服务器并随时切换。切换后需要重新登录对应服务器；自签名证书需勾选下方选项。';

    const list = div('servers-list');
    const form = div('server-form');
    const nameField = labeledInput('名称', 'text', 'off');
    const urlField = labeledInput('服务器地址', 'text', 'off');
    urlField.input.placeholder = 'https://192.168.1.10:8090';
    const checkRow = document.createElement('label');
    checkRow.className = 'check';
    const checkBox = document.createElement('input');
    checkBox.type = 'checkbox';
    const checkText = span('');
    checkText.textContent = '允许自签名证书（首次连接常见）';
    checkRow.append(checkBox, checkText);

    const errorEl = div('form-error inline-error');
    const formActions = div('form-actions');
    const saveButton = buttonEl('添加服务器', 'btn primary');
    const cancelEdit = buttonEl('取消编辑', 'btn ghost small');
    cancelEdit.hidden = true;
    formActions.append(saveButton, cancelEdit);
    form.append(
      nameField.field,
      urlField.field,
      checkRow,
      errorEl,
      formActions,
    );

    const footer = div('modal-actions');
    const closeButton = buttonEl('关闭', 'btn ghost');
    const close = () => {
      finish(null);
      if (state.app && state.app.active !== activeAtOpen) {
        location.reload();
      }
    };
    closeButton.addEventListener('click', close);
    footer.append(closeButton);

    body.append(heading, hint, list, form, footer);
    dialog.append(body);

    let editingId = null;

    const resetForm = () => {
      editingId = null;
      nameField.input.value = '';
      urlField.input.value = '';
      checkBox.checked = false;
      saveButton.textContent = '添加服务器';
      cancelEdit.hidden = true;
      errorEl.textContent = '';
    };

    const drawList = () => {
      list.replaceChildren();
      const remotes = (state.app && state.app.targets
        ? state.app.targets
        : []
      ).filter((target) => target.kind === 'remote');
      if (!remotes.length) {
        const empty = div('servers-empty');
        empty.textContent = '还没有保存的服务器，请在下方添加。';
        list.append(empty);
        return;
      }
      for (const target of remotes) {
        const row = div('server-row');
        const info = div('server-info');
        const title = span('server-name');
        title.textContent = target.name || target.url;
        if (target.id === state.app.active) {
          const badge = span('server-badge');
          badge.textContent = '当前';
          title.append(badge);
        }
        const meta = span('server-url');
        meta.textContent = target.url;
        info.append(title, meta);

        const actions = div('server-actions');
        const edit = buttonEl('编辑', 'btn ghost small');
        edit.addEventListener('click', () => {
          editingId = target.id;
          nameField.input.value = target.name || '';
          urlField.input.value = target.url || '';
          checkBox.checked = Boolean(target.allow_invalid_certs);
          saveButton.textContent = '保存修改';
          cancelEdit.hidden = false;
          errorEl.textContent = '';
          nameField.input.focus();
        });
        const remove = buttonEl('删除', 'btn ghost small danger-text');
        remove.addEventListener('click', async () => {
          errorEl.textContent = '';
          try {
            await serversAction({ action: 'remove', id: target.id });
            await refreshAppInfo();
            drawList();
            resetForm();
          } catch (error) {
            errorEl.textContent = error.message || '删除失败';
          }
        });
        actions.append(edit, remove);
        row.append(info, actions);
        list.append(row);
      }
    };

    const submit = async () => {
      errorEl.textContent = '';
      const name = nameField.input.value.trim();
      const url = urlField.input.value.trim();
      if (!name || !url) {
        errorEl.textContent = '请填写名称和服务器地址';
        return;
      }
      saveButton.disabled = true;
      try {
        const payload = {
          name,
          url,
          allow_invalid_certs: checkBox.checked,
        };
        if (editingId) {
          payload.action = 'update';
          payload.id = editingId;
        } else {
          payload.action = 'add';
        }
        await serversAction(payload);
        await refreshAppInfo();
        drawList();
        resetForm();
        toast(editingId ? '服务器已更新' : '服务器已添加', 'success');
      } catch (error) {
        errorEl.textContent = error.message || '保存失败';
      } finally {
        saveButton.disabled = false;
      }
    };

    saveButton.addEventListener('click', () => void submit());
    cancelEdit.addEventListener('click', resetForm);
    for (const field of [nameField, urlField]) {
      field.input.addEventListener('keydown', (event) => {
        if (event.key === 'Enter') {
          event.preventDefault();
          void submit();
        }
      });
    }

    drawList();
    setTimeout(() => nameField.input.focus(), 0);
  });
}

/* ---------- Wiring ---------- */

function bindEvents() {
  $('#login-form').addEventListener('submit', onLogin);

  $('#btn-upload').addEventListener('click', () => $('#file-input').click());
  $('#file-input').addEventListener('change', (event) => {
    // FileList is live: snapshot it before clearing the input.
    const files = Array.from(event.target.files || []);
    event.target.value = '';
    enqueueUploads(files, state.cwd);
  });
  $('#btn-upload-folder').addEventListener('click', () => $('#dir-input').click());
  $('#dir-input').addEventListener('change', (event) => {
    const files = Array.from(event.target.files || []);
    event.target.value = '';
    if (files.length) enqueueUploads(files, state.cwd);
  });
  $('#btn-mkdir').addEventListener('click', createFolder);
  $('#btn-refresh').addEventListener('click', () => loadDir(state.cwd));
  $('#btn-history').addEventListener('click', openHistoryDialog);
  $('#btn-servers').addEventListener('click', openServersDialog);
  $('#setup-add').addEventListener('click', openServersDialog);
  $('#login-servers').addEventListener('click', openServersDialog);
  $('#target-select').addEventListener('change', async (event) => {
    const id = event.target.value;
    if (!id) return;
    try {
      const response = await fetch('/app/active', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ id }),
      });
      if (!response.ok) throw new Error('切换连接失败');
      location.reload();
    } catch (error) {
      toast(error.message || '切换连接失败', 'error');
      renderAppBar();
    }
  });
  $('#root-select').addEventListener('change', async (event) => {
    const path = event.target.value;
    try {
      const response = await fetch('/app/local/root', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ path }),
      });
      let data = null;
      try {
        data = await response.json();
      } catch {
        data = null;
      }
      if (!response.ok) {
        throw new Error(data && data.error ? data.error : '切换目录失败');
      }
      await refreshAppInfo();
      await loadDir('/');
    } catch (error) {
      toast(error.message || '切换目录失败', 'error');
      renderAppBar();
    }
  });
  if ('IntersectionObserver' in window) {
    const observer = new IntersectionObserver(
      (entries) => {
        if (
          entries.some((entry) => entry.isIntersecting) &&
          state.key &&
          state.autoLoadPaging !== false &&
          state.list.hasMore &&
          !state.list.loading
        ) {
          void loadMore();
        }
      },
      { rootMargin: '240px 0px' },
    );
    observer.observe($('#list-footer'));
  }
  $('#btn-account').addEventListener('click', openAccountDialog);
  $('#user-badge').addEventListener('click', () => {
    if (state.key) openAccountDialog();
  });
  $('#btn-logout').addEventListener('click', onLogout);
  $('#btn-clear-uploads').addEventListener('click', clearFinishedUploads);
  $('#filter').addEventListener('input', (event) => {
    state.filter = event.target.value;
    renderFiles();
  });

  document.addEventListener('dragenter', (event) => {
    if (!state.key || !hasFileDrag(event)) return;
    event.preventDefault();
    dragDepth += 1;
    $('#drop-overlay').hidden = false;
  });
  document.addEventListener('dragover', (event) => {
    if (!state.key || !hasFileDrag(event)) return;
    event.preventDefault();
    if (event.dataTransfer) event.dataTransfer.dropEffect = 'copy';
  });
  document.addEventListener('dragleave', (event) => {
    if (!state.key) return;
    event.preventDefault();
    dragDepth = Math.max(0, dragDepth - 1);
    if (dragDepth === 0) $('#drop-overlay').hidden = true;
  });
  document.addEventListener('drop', async (event) => {
    if (!state.key || !hasFileDrag(event)) return;
    event.preventDefault();
    dragDepth = 0;
    $('#drop-overlay').hidden = true;
    if (!event.dataTransfer) return;
    try {
      const collected = await collectDropItems(event.dataTransfer);
      enqueueUploads(collected.items, state.cwd, collected.directories);
    } catch {
      toast('读取拖入的文件失败，请改用“上传”按钮选择文件', 'error');
    }
  });
}

function hasFileDrag(event) {
  return Array.from(event.dataTransfer ? event.dataTransfer.types : []).includes('Files');
}

document.addEventListener('DOMContentLoaded', () => {
  bindEvents();
  document.addEventListener('keydown', handlePreviewKeys);
  void boot();
});
