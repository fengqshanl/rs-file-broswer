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
  copy:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="8" y="8" width="12" height="12" rx="2"/><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2"/></svg>',
  cut:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="6" cy="7" r="2.5"/><circle cx="6" cy="17" r="2.5"/><path d="m8 8.5 11 5.5"/><path d="m8 15.5 11-5.5"/></svg>',
  refresh:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M20 11a8 8 0 1 0-2.3 5.7"/><path d="M20 4v7h-7"/></svg>',
};

const ARCHIVE_END_MARKER = 'AGF_TAR_END_v1';
const LIST_PAGE_SIZE = 300;
const ENSURE_DIRS_BATCH = 100;
const TEXT_STREAM_BYTES = 32 * 1024 * 1024;
const HISTORY_KEY = 'agentgod.history.v1';
const SHOW_HIDDEN_KEY = 'agentgod.show-hidden';
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

function emptyList() {
  return { offset: 0, total: 0, hasMore: false, loading: false };
}

function readStoredShowHidden() {
  try {
    return localStorage.getItem(SHOW_HIDDEN_KEY) === '1';
  } catch {
    return false;
  }
}

function makeTab(target) {
  return {
    id: target.id,
    target,
    session: null,
    cwd: '/',
    entries: [],
    filter: '',
    list: emptyList(),
    navToken: 0,
    pendingHighlight: null,
    selectedPath: null,
    selectedEntry: null,
    selectedPaths: new Set(),
    anchorPath: null,
    folderSizes: new Map(),
    folderSizePending: new Map(),
    freeSpace: null,
    loaded: false,
    needsRefresh: false,
  };
}

const state = {
  app: null,
  tabs: new Map(),
  activeTabId: null,
  standaloneTab: null,
  clipboard: null,
  showHidden: readStoredShowHidden(),
  pageSize: LIST_PAGE_SIZE,
  autoLoadPaging: true,
};

state.standaloneTab = makeTab({ id: 'standalone', kind: 'standalone', name: '服务器' });

function activeTab() {
  return state.tabs.get(state.activeTabId) || state.standaloneTab;
}

function getTab(id) {
  return state.tabs.get(id) || null;
}

function targetLabel(target) {
  if (!target) return '服务器';
  if (target.kind === 'local') return '本机';
  return target.name || target.host || target.url || '服务器';
}

/// Local files and SSH hosts both authenticate through /api/local-login.
function isPasswordless(target) {
  return Boolean(target) && (target.kind === 'local' || target.kind === 'ssh');
}

function targetDetail(target) {
  if (!target) return '';
  if (target.kind === 'ssh') {
    const port = target.port && target.port !== 22 ? `:${target.port}` : '';
    return `${target.user || ''}@${target.host || ''}${port}`;
  }
  return target.url || '';
}

function targetHeaders(tab) {
  if (!state.app || !tab || tab.target.kind === 'standalone') return {};
  return { 'X-AgentGod-Target': tab.id };
}

for (const field of [
  'cwd',
  'entries',
  'filter',
  'list',
  'navToken',
  'pendingHighlight',
  'selectedPath',
]) {
  Object.defineProperty(state, field, {
    get() {
      const tab = activeTab();
      return field === 'cwd'
        ? tab.cwd || '/'
        : field === 'entries' || field === 'list'
          ? tab[field] || (field === 'list' ? emptyList() : [])
          : tab[field];
    },
    set(value) {
      const tab = activeTab();
      if (tab) tab[field] = value;
    },
  });
}

Object.defineProperties(state, {
  key: { get: () => activeTab().session?.key || null },
  user: { get: () => activeTab().session?.user || null },
});

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

/** Blob.arrayBuffer() is missing in older WebKit builds shipped with macOS 10.15. */
function readBlob(blob) {
  if (typeof blob.arrayBuffer === 'function') {
    return blob.arrayBuffer();
  }
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(reader.result);
    reader.onerror = () => reject(reader.error || new Error('读取文件失败'));
    reader.readAsArrayBuffer(blob);
  });
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

/* ---------- API ---------- */

async function api(path, payload = {}, method = 'POST', tab = activeTab()) {
  const session = tab.session;
  if (!session) throw new Error('会话已失效，请重新登录');
  const aad = `${method} ${path}`;
  const envelope = await encryptWithKey(session.key, aad, payload);
  const response = await fetch(path, {
    method,
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${session.token}`,
      ...targetHeaders(tab),
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
    if (response.status === 401) handleSessionLost('会话已过期，请重新登录', tab);
    throw new Error(data && data.error ? data.error : `请求失败 (${response.status})`);
  }
  return decryptWithKey(session.key, aad, data);
}

async function login(tab, username, password) {
  ensureCrypto();
  const challengeResponse = await fetch('/api/challenge', {
    cache: 'no-store',
    headers: targetHeaders(tab),
  });
  if (!challengeResponse.ok) throw new Error('无法获取登录凭证，请确认服务端已升级');
  const challengeInfo = await challengeResponse.json();
  if (!challengeInfo || !challengeInfo.challenge) throw new Error('登录凭证无效');

  const keyResponse = await fetch('/api/pubkey', {
    cache: 'no-store',
    headers: targetHeaders(tab),
  });
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
    JSON.stringify({
      username,
      password,
      ts: Date.now(),
      challenge: challengeInfo.challenge,
    }),
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
    headers: { 'Content-Type': 'application/json', ...targetHeaders(tab) },
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
  tab.session = {
    key: sessionKey,
    token: result.token,
    user: result.user,
  };
}

let loginTab = null;

function clearTabSession(tab) {
  tab.session = null;
  tab.cwd = '/';
  tab.entries = [];
  tab.filter = '';
  tab.list = emptyList();
  tab.navToken += 1;
  tab.pendingHighlight = null;
  tab.selectedPath = null;
  tab.selectedEntry = null;
  tab.selectedPaths.clear();
  tab.anchorPath = null;
  tab.folderSizes.clear();
  tab.folderSizePending.clear();
  tab.freeSpace = null;
  tab.loaded = false;
  tab.needsRefresh = false;
  for (let index = uploadQueue.length - 1; index >= 0; index -= 1) {
    if (uploadQueue[index].tab === tab) uploadQueue.splice(index, 1);
  }
  if (state.clipboard?.tabId === tab.id) {
    state.clipboard = null;
    updatePasteButton();
  }
}

function handleSessionLost(message, tab = activeTab()) {
  const wasActive = tab === activeTab();
  clearTabSession(tab);
  if (!wasActive) {
    renderTabs();
    updatePasteButton();
    return;
  }
  if (isPasswordless(tab.target)) {
    void enterLocalTab(tab).catch((error) => {
      showTabLogin(tab, error.message || '连接失败');
    });
    return;
  }
  showTabLogin(tab, message);
  renderTabs();
  updatePasteButton();
}

/* ---------- Session / views ---------- */

function showLoginError(message) {
  $('#login-error').textContent = message || '';
}

async function onLogin(event) {
  event.preventDefault();
  showLoginError('');
  const tab = loginTab || activeTab();
  const button = $('#login-btn');
  if (isPasswordless(tab.target)) {
    button.disabled = true;
    button.textContent = '正在连接…';
    try {
      await enterLocalTab(tab);
    } catch (error) {
      showLoginError(error.message || '连接失败');
    } finally {
      button.disabled = false;
      button.textContent =
        tab.target.kind === 'local' ? '重新连接本机' : '连接服务器';
    }
    return;
  }
  const username = $('#username').value.trim();
  const password = $('#password').value;
  if (!username || !password) {
    showLoginError('请输入用户名和密码');
    return;
  }
  button.disabled = true;
  button.textContent = '正在加密登录…';
  try {
    await login(tab, username, password);
    $('#password').value = '';
    loginTab = null;
    await activateTab(tab.id);
  } catch (error) {
    showLoginError(error.message || '登录失败');
  } finally {
    button.disabled = false;
    button.textContent = '加密登录';
  }
}

async function onLogout() {
  const tab = activeTab();
  if (!tab.session || isPasswordless(tab.target)) return;
  try {
    await api('/api/logout', {}, 'POST', tab);
  } catch {
    /* the session is discarded regardless */
  }
  clearTabSession(tab);
  showTabLogin(tab, '');
  renderTabs();
  updatePasteButton();
  toast('已安全退出', 'info');
}

/* ---------- Listing ---------- */

async function loadDir(path, options = {}) {
  const tab = options.tab || activeTab();
  if (!tab.session) return;
  const target = normalizeRelPath(path) || '/';
  tab.cwd = target;
  tab.entries = [];
  tab.filter = '';
  tab.list = emptyList();
  tab.navToken += 1;
  tab.loaded = false;
  tab.needsRefresh = false;
  tab.selectedPath = null;
  tab.selectedEntry = null;
  tab.selectedPaths.clear();
  tab.anchorPath = null;
  tab.folderSizes.clear();
  tab.folderSizePending.clear();
  const token = tab.navToken;
  const isActive = tab === activeTab();

  if (isActive) {
    $('#filter').value = '';
    renderBreadcrumbs();
    showListSkeleton(true);
    $('#dir-summary').textContent = '正在加载…';
  }

  try {
    const data = await api(
      '/api/list',
      {
        path: target,
        offset: 0,
        limit: state.pageSize,
        show_hidden: state.showHidden,
      },
      'POST',
      tab,
    );
    if (token !== tab.navToken) return;
    tab.cwd = data.path || target;
    tab.entries = Array.isArray(data.entries) ? data.entries : [];
    tab.list.offset = tab.entries.length;
    tab.list.total = Number(data.total) || tab.entries.length;
    tab.list.hasMore = Boolean(data.has_more);
    tab.freeSpace =
      typeof data.free_space === 'number' ? data.free_space : null;
    tab.loaded = true;
    if (!isActive || tab !== activeTab()) return;
    $('#free-space').textContent =
      typeof tab.freeSpace === 'number' ? `剩余 ${formatSize(tab.freeSpace)}` : '';
    renderBreadcrumbs();
    showListSkeleton(false);
    renderFiles();
    if (data.truncated) {
      toast('目录项目超过 50000 个，仅显示前 50000 项', 'error');
    }
    if (!options.skipHistory) {
      recordHistory('dir', tab.cwd, basenameOf(tab.cwd), parentOf(tab.cwd), tab);
    }
    if (tab.pendingHighlight) {
      const pending = tab.pendingHighlight;
      tab.pendingHighlight = null;
      await highlightEntry(pending);
    }
  } catch (error) {
    if (token !== tab.navToken) return;
    tab.needsRefresh = true;
    if (!isActive || tab !== activeTab()) return;
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
  let entries = state.entries;
  if (!state.showHidden) {
    // Newer services filter server-side and flag hidden entries. The name
    // check keeps dotfiles hidden when talking to an older service.
    entries = entries.filter(
      (entry) => !entry.hidden && !entry.name.startsWith('.'),
    );
  }
  const filter = state.filter.trim().toLowerCase();
  if (!filter) return entries;
  return entries.filter((entry) => entry.name.toLowerCase().includes(filter));
}

/* ---------- Folder sizes (lazy, cached per tab) ---------- */

const FOLDER_SIZE_CONCURRENCY = 3;
const folderSizeQueue = [];
let folderSizeActive = 0;

function folderSizeLabel(data) {
  if (data && data.error) return '—';
  return data && data.truncated
    ? `≥ ${formatSize(data.size)}`
    : formatSize(data ? data.size : 0);
}

function folderSizeTitle(data) {
  if (data && data.error) return '无法读取文件夹大小';
  if (!data) return '正在计算文件夹大小…';
  const parts = [
    formatSize(data.size),
    `${Number(data.files) || 0} 个文件`,
    `${Number(data.dirs) || 0} 个文件夹`,
  ];
  if (data.truncated) parts.push('项目过多，结果为下限');
  return parts.join(' · ');
}

function queueFolderSize(tab, entry, fullPath) {
  if (!entry.is_dir || !tab.session) return;
  if (tab.folderSizes.has(fullPath)) return;
  if (tab.folderSizePending.get(fullPath) === tab.navToken) return;
  tab.folderSizePending.set(fullPath, tab.navToken);
  folderSizeQueue.push({ tab, path: fullPath, token: tab.navToken });
  pumpFolderSizeQueue();
}

let folderSizeObserver = null;

/// Only measure folders that actually scroll into view. A directory with
/// hundreds of subfolders would otherwise start hundreds of recursive walks.
function observeFolderSize(row, tab, entry, fullPath) {
  if (!entry.is_dir || !tab.session || tab.folderSizes.has(fullPath)) return;
  if (typeof IntersectionObserver !== 'function') {
    queueFolderSize(tab, entry, fullPath);
    return;
  }
  if (!folderSizeObserver) {
    folderSizeObserver = new IntersectionObserver(
      (records, observer) => {
        for (const record of records) {
          if (!record.isIntersecting) continue;
          observer.unobserve(record.target);
          const job = record.target.__folderSizeJob;
          record.target.__folderSizeJob = null;
          if (job) queueFolderSize(job.tab, job.entry, job.path);
        }
      },
      { rootMargin: '240px 0px' },
    );
  }
  row.__folderSizeJob = { tab, entry, path: fullPath };
  folderSizeObserver.observe(row);
}

function resetFolderSizeObserver() {
  if (folderSizeObserver) folderSizeObserver.disconnect();
}

function pumpFolderSizeQueue() {
  while (folderSizeActive < FOLDER_SIZE_CONCURRENCY && folderSizeQueue.length) {
    const job = folderSizeQueue.shift();
    folderSizeActive += 1;
    void measureFolderSize(job);
  }
}

async function measureFolderSize({ tab, path, token }) {
  try {
    if (tab.session && tab.navToken === token) {
      const data = await api('/api/folder_size', { path }, 'POST', tab);
      if (tab.navToken === token) {
        tab.folderSizes.set(path, data);
        updateFolderSizeCell(tab, path);
      }
    }
  } catch {
    // Unreadable directories keep the dash; the listing itself still works.
    if (tab.navToken === token) {
      tab.folderSizes.set(path, { error: true });
      updateFolderSizeCell(tab, path);
    }
  } finally {
    if (tab.folderSizePending.get(path) === token) {
      tab.folderSizePending.delete(path);
    }
    folderSizeActive = Math.max(0, folderSizeActive - 1);
    pumpFolderSizeQueue();
  }
}

function updateFolderSizeCell(tab, path) {
  if (tab !== activeTab()) return;
  const row = findRow(path);
  if (!row) return;
  const cell = row.querySelector('.size');
  const data = tab.folderSizes.get(path);
  if (!cell || !data) return;
  cell.textContent = folderSizeLabel(data);
  cell.title = folderSizeTitle(data);
}

/* ---------- Selection ---------- */

function entryPath(tab, entry) {
  return joinPath(tab.cwd, entry.name);
}

function selectedEntries(tab = activeTab()) {
  if (!tab.selectedPaths || tab.selectedPaths.size === 0) return [];
  const selected = [];
  for (const entry of tab.entries) {
    if (tab.selectedPaths.has(entryPath(tab, entry))) selected.push(entry);
  }
  return selected;
}

function syncSelection(tab = activeTab()) {
  for (const row of document.querySelectorAll('#file-list .file-row')) {
    row.classList.toggle('selected', tab.selectedPaths.has(row.dataset.path));
  }
}

function clearSelection(tab = activeTab()) {
  tab.selectedPaths.clear();
  tab.selectedPath = null;
  tab.selectedEntry = null;
  tab.anchorPath = null;
  syncSelection(tab);
}

function selectAllEntries(tab = activeTab()) {
  const visible = visibleEntries();
  tab.selectedPaths.clear();
  for (const entry of visible) {
    tab.selectedPaths.add(entryPath(tab, entry));
  }
  const last = visible[visible.length - 1];
  tab.anchorPath = last ? entryPath(tab, last) : null;
  tab.selectedPath = last ? entryPath(tab, last) : null;
  tab.selectedEntry = last || null;
  syncSelection(tab);
}

function openEntry(entry, tab = activeTab()) {
  if (!entry) return;
  const fullPath = entryPath(tab, entry);
  if (entry.is_dir) return loadDir(fullPath, { tab });
  if (previewKindOf(entry.name) !== 'none') return openPreview(entry, tab);
  return startDownload(fullPath, entry.name, tab);
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
  resetFolderSizeObserver();
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
    list.append(renderRow(entry, activeTab()));
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
    list.append(renderRow(visible[index], activeTab()));
  }
  const hasEntries = visible.length > 0;
  $('.list-head').hidden = !hasEntries;
  list.hidden = !hasEntries;
  $('#empty-state').hidden = hasEntries;
  updateListSummary();
  renderListFooter();
}

async function loadMore(tab = activeTab()) {
  if (!tab.session || tab.list.loading || !tab.list.hasMore) return;
  const token = tab.navToken;
  tab.list.loading = true;
  const isActive = tab === activeTab();
  if (isActive) renderListFooter();
  try {
    const data = await api(
      '/api/list',
      {
        path: tab.cwd,
        offset: tab.list.offset,
        limit: state.pageSize,
        show_hidden: state.showHidden,
      },
      'POST',
      tab,
    );
    if (token !== tab.navToken) return;
    const before = tab.entries.length;
    const entries = Array.isArray(data.entries) ? data.entries : [];
    tab.entries.push(...entries);
    tab.list.offset = tab.entries.length;
    tab.list.total = Number(data.total) || tab.list.total;
    tab.list.hasMore = Boolean(data.has_more);
    if (!isActive || tab !== activeTab()) return;
    appendRows(before);
  } catch (error) {
    if (token === tab.navToken && tab === activeTab()) {
      toast(error.message || '加载更多失败', 'error');
    }
  } finally {
    if (token === tab.navToken) {
      tab.list.loading = false;
      if (tab === activeTab()) renderListFooter();
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

function renderRow(entry, tab = activeTab()) {
  const row = div('file-row');
  row.setAttribute('role', 'listitem');
  const fullPath = entryPath(tab, entry);
  row.dataset.path = fullPath;
  if (entry.hidden) row.classList.add('hidden-entry');
  if (tab.selectedPaths.has(fullPath)) row.classList.add('selected');
  row.addEventListener('click', (event) => {
    const modified = event.metaKey || event.ctrlKey || event.shiftKey;
    if (
      !modified &&
      event.target.closest('button, a, input, .previewable')
    ) {
      return;
    }
    selectRow(row, entry, tab, event);
  });
  row.addEventListener('contextmenu', (event) => {
    event.preventDefault();
    if (!tab.selectedPaths.has(fullPath)) {
      tab.selectedPaths.clear();
      tab.selectedPaths.add(fullPath);
      tab.anchorPath = fullPath;
      tab.selectedPath = fullPath;
      tab.selectedEntry = entry;
      syncSelection(tab);
    }
    openRowContextMenu(event.clientX, event.clientY, tab);
  });

  const nameCell = div('cell name-cell');
  const icon = span('file-icon ' + (entry.is_dir ? 'is-dir' : 'is-file'));
  icon.innerHTML = entry.is_dir ? ICONS.folder : ICONS.file;
  nameCell.append(icon);

  if (entry.is_dir) {
    const nameButton = buttonEl(entry.name, 'name-btn');
    nameButton.title = entry.name;
    nameButton.addEventListener('click', (event) => {
      if (event.metaKey || event.ctrlKey || event.shiftKey) return;
      loadDir(joinPath(tab.cwd, entry.name), { tab });
    });
    nameCell.append(nameButton);
  } else {
    const name = span('file-name');
    name.textContent = entry.name;
    name.title = entry.name;
    if (previewKindOf(entry.name) !== 'none') {
      name.classList.add('previewable');
      name.addEventListener('click', (event) => {
        if (event.metaKey || event.ctrlKey || event.shiftKey) return;
        openPreview(entry, tab);
      });
    }
    nameCell.append(name);
  }
  if (entry.is_symlink) {
    const badge = span('badge');
    badge.textContent = '链接';
    nameCell.append(badge);
  }

  const sizeCell = div('cell size');
  if (entry.is_dir) {
    const cached = tab.folderSizes.get(fullPath);
    sizeCell.textContent = cached ? folderSizeLabel(cached) : '—';
    sizeCell.title = cached ? folderSizeTitle(cached) : '正在计算文件夹大小…';
    observeFolderSize(row, tab, entry, fullPath);
  } else {
    sizeCell.textContent = formatSize(entry.size);
  }

  const timeCell = div('cell time');
  timeCell.textContent = formatDate(entry.modified);

  row.append(nameCell, sizeCell, timeCell);
  return row;
}

function selectRow(row, entry, tab, event = null) {
  const path = row.dataset.path;
  const toggle = Boolean(event && (event.metaKey || event.ctrlKey));
  const range = Boolean(event && event.shiftKey && tab.anchorPath);

  if (range) {
    const visible = visibleEntries();
    const anchorIndex = visible.findIndex(
      (item) => entryPath(tab, item) === tab.anchorPath,
    );
    const currentIndex = visible.findIndex(
      (item) => entryPath(tab, item) === path,
    );
    if (anchorIndex >= 0 && currentIndex >= 0) {
      tab.selectedPaths.clear();
      const start = Math.min(anchorIndex, currentIndex);
      const end = Math.max(anchorIndex, currentIndex);
      for (let index = start; index <= end; index += 1) {
        tab.selectedPaths.add(entryPath(tab, visible[index]));
      }
      tab.selectedPath = path;
      tab.selectedEntry = entry;
      syncSelection(tab);
      return;
    }
  }

  if (toggle) {
    if (tab.selectedPaths.has(path)) {
      tab.selectedPaths.delete(path);
      if (tab.selectedPath === path) {
        const remaining = selectedEntries(tab);
        const last = remaining[remaining.length - 1] || null;
        tab.selectedPath = last ? entryPath(tab, last) : null;
        tab.selectedEntry = last;
      }
    } else {
      tab.selectedPaths.add(path);
      tab.selectedPath = path;
      tab.selectedEntry = entry;
    }
    tab.anchorPath = path;
    syncSelection(tab);
    return;
  }

  tab.selectedPaths.clear();
  tab.selectedPaths.add(path);
  tab.anchorPath = path;
  tab.selectedPath = path;
  tab.selectedEntry = entry;
  syncSelection(tab);
}

/* ---------- File operations ---------- */

async function renameEntry(entry, tab = activeTab()) {
  const cwd = tab.cwd;
  const newName = await askText({
    title: '重命名',
    label: '新的名称',
    value: entry.name,
    confirmText: '重命名',
  });
  if (!newName || newName === entry.name) return;
  try {
    await api(
      '/api/rename',
      {
        path: joinPath(cwd, entry.name),
        new_name: newName,
      },
      'POST',
      tab,
    );
    toast('已重命名', 'success');
    await loadDir(cwd, { tab });
  } catch (error) {
    toast(error.message || '重命名失败', 'error');
  }
}

async function deleteEntries(entries, tab = activeTab()) {
  if (!entries.length) return;
  const cwd = tab.cwd;
  const multiple = entries.length > 1;
  const confirmed = await confirmDialog({
    title: multiple ? `删除 ${entries.length} 个项目？` : `删除「${entries[0].name}」？`,
    message: entries.some((entry) => entry.is_dir)
      ? '选中的文件夹及其中的全部内容都会被删除，此操作不可恢复。'
      : '此操作不可恢复。',
  });
  if (!confirmed) return;
  try {
    for (const entry of entries) {
      await api(
        '/api/delete',
        { path: joinPath(cwd, entry.name) },
        'POST',
        tab,
      );
    }
    clearSelection(tab);
    toast(multiple ? `已删除 ${entries.length} 项` : '已删除', 'success');
    await loadDir(cwd, { tab });
  } catch (error) {
    toast(error.message || '删除失败', 'error');
    await loadDir(cwd, { tab });
  }
}

function deleteEntry(entry, tab = activeTab()) {
  return deleteEntries([entry], tab);
}

async function createFolder() {
  const tab = activeTab();
  const cwd = tab.cwd;
  const name = await askText({
    title: '新建文件夹',
    label: '文件夹名称',
    placeholder: '例如：备份',
    confirmText: '创建',
  });
  if (!name) return;
  try {
    await api('/api/mkdir', { path: joinPath(cwd, name) }, 'POST', tab);
    toast('文件夹已创建', 'success');
    await loadDir(cwd, { tab });
  } catch (error) {
    toast(error.message || '创建文件夹失败', 'error');
  }
}

/* ---------- Cross-tab copy / paste ---------- */

const COPY_CHUNK = 2 * 1024 * 1024;

function clipboardItems() {
  const clipboard = state.clipboard;
  return clipboard && Array.isArray(clipboard.items) ? clipboard.items : [];
}

function updatePasteButton() {
  const button = $('#btn-paste');
  if (!button) return;
  const clipboard = state.clipboard;
  const items = clipboardItems();
  const tab = activeTab();
  button.disabled = items.length === 0 || !tab.session;
  const label = button.querySelector('.btn-label');
  const verb = clipboard?.cut ? '移动' : '粘贴';
  if (label) {
    label.textContent =
      items.length > 1 ? `${verb} (${items.length})` : verb;
  }
  button.title = items.length
    ? `${clipboard.cut ? '移动' : '复制'} ${items.length} 项到当前目录`
    : '先复制或剪切文件/文件夹';
}

function copyEntries(entries, cut = false, tab = activeTab()) {
  if (!tab.session) {
    toast('请先登录当前标签', 'error');
    return;
  }
  if (!entries.length) return;
  state.clipboard = {
    tabId: tab.id,
    items: entries.map((entry) => ({
      path: joinPath(tab.cwd, entry.name),
      name: entry.name,
      isDir: entry.is_dir,
    })),
    cut: Boolean(cut),
  };
  updatePasteButton();
  const count = state.clipboard.items.length;
  toast(
    count === 1
      ? cut
        ? `已剪切「${entries[0].name}」，切换到目标标签后点击“移动”`
        : `已复制「${entries[0].name}」，切换到目标标签后点击“粘贴”`
      : `已${cut ? '剪切' : '复制'} ${count} 项`,
    'success',
  );
}

function copyEntry(entry, cut = false, tab = activeTab()) {
  copyEntries([entry], cut, tab);
}

function addTransferItem(label, total, detail) {
  $('#upload-panel').hidden = false;
  const wrapper = div('upload-item');
  const head = div('upload-item-head');
  const title = document.createElement('strong');
  title.textContent = label;
  title.title = label;
  const meta = span('upload-meta');
  meta.textContent = detail;
  head.append(title, meta);
  const bar = document.createElement('progress');
  bar.max = 100;
  bar.value = 0;
  const status = span('upload-status');
  status.textContent = '正在准备…';
  wrapper.append(head, bar, status);
  $('#upload-list').append(wrapper);
  wrapper.scrollIntoView({ block: 'nearest' });
  return {
    update(received, size, text) {
      bar.value = size > 0 ? Math.min(100, (received / size) * 100) : 0;
      status.textContent =
        text ||
        (size > 0
          ? `${formatSize(received)} / ${formatSize(size)}`
          : formatSize(received));
    },
    done(text) {
      bar.value = 100;
      status.textContent = text || '已完成';
      wrapper.classList.add('done');
    },
    fail(text) {
      status.textContent = `失败 · ${text}`;
      wrapper.classList.add('failed');
    },
  };
}

async function listAllEntries(tab, path) {
  const entries = [];
  let offset = 0;
  while (true) {
    const data = await api(
      '/api/list',
      { path, offset, limit: 1000, show_hidden: true },
      'POST',
      tab,
    );
    const batch = Array.isArray(data.entries) ? data.entries : [];
    entries.push(...batch);
    if (!data.has_more || !batch.length) break;
    offset += batch.length;
  }
  return entries;
}

function uniqueName(name, isDir, names) {
  const taken = new Set(names || []);
  if (!taken.has(name)) return name;
  const dot = isDir ? -1 : name.lastIndexOf('.');
  const stem = dot > 0 ? name.slice(0, dot) : name;
  const extension = dot > 0 ? name.slice(dot) : '';
  let candidate = `${stem} (副本)${extension}`;
  if (!taken.has(candidate)) return candidate;
  for (let index = 2; ; index += 1) {
    candidate = `${stem} (${index})${extension}`;
    if (!taken.has(candidate)) return candidate;
  }
}

async function ensureDirsInTab(tab, paths) {
  // Every request is an AES-GCM envelope capped at 128 KiB server-side.
  // Small batches keep long nested paths below that limit instead of silently
  // losing the whole directory tree on a 413.
  for (let index = 0; index < paths.length; index += ENSURE_DIRS_BATCH) {
    await api(
      '/api/ensure_dirs',
      { paths: paths.slice(index, index + ENSURE_DIRS_BATCH) },
      'POST',
      tab,
    );
  }
}

async function openEncryptedDownload(tab, path) {
  const session = tab.session;
  if (!session) throw new Error('源标签会话已失效');
  const endpoint = '/api/download';
  const aad = `POST ${endpoint}`;
  const envelope = await encryptWithKey(session.key, aad, { path });
  const response = await fetch(endpoint, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${session.token}`,
      ...targetHeaders(tab),
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
    if (response.status === 401) handleSessionLost('会话已过期，请重新登录', tab);
    throw new Error(message || `读取文件失败 (${response.status})`);
  }
  return {
    reader: response.body.getReader(),
    total: Number(response.headers.get('x-file-size') || 0),
    chunkSize: Number(response.headers.get('x-chunk-size') || 65536),
    key: session.key,
  };
}

async function readDownloadFrames(download, onPlaintext) {
  const { reader, chunkSize, key } = download;
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
        if (length > chunkSize + 16 + 1024) throw new Error('下载分块大小异常');
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
                additionalData: encoder.encode(`dl:${sequence}`),
              },
              key,
              ciphertext,
            ),
          );
        } catch {
          throw new Error('下载数据解密失败，连接可能已被篡改');
        }
        sequence += 1;
        pending = pending.subarray(16 + length);
        received += plaintext.length;
        await onPlaintext(plaintext, received);
      }
    }
    if (pending.length !== 0) throw new Error('下载数据不完整');
    if (download.total > 0 && received !== download.total) {
      throw new Error('文件大小校验失败');
    }
    return received;
  } catch (error) {
    await reader.cancel().catch(() => {});
    throw error;
  }
}

async function uploadChunkToTab(tab, uploadId, sequence, plaintext) {
  const session = tab.session;
  if (!session) throw new Error('目标标签会话已失效');
  const path = `/api/upload/chunk/${encodeURIComponent(uploadId)}/${sequence}`;
  const aad = `PUT ${path}`;
  const iv = randomBytes(12);
  const ciphertext = new Uint8Array(
    await crypto.subtle.encrypt(
      { name: 'AES-GCM', iv, additionalData: encoder.encode(aad) },
      session.key,
      plaintext,
    ),
  );
  const response = await fetch(path, {
    method: 'PUT',
    headers: {
      'Content-Type': 'application/octet-stream',
      Authorization: `Bearer ${session.token}`,
      ...targetHeaders(tab),
    },
    body: makeFrame(iv, ciphertext),
  });
  let data = null;
  try {
    data = await response.json();
  } catch {
    data = null;
  }
  if (!response.ok) {
    if (response.status === 401) handleSessionLost('会话已过期，请重新登录', tab);
    throw new Error(data && data.error ? data.error : `写入失败 (${response.status})`);
  }
  await decryptWithKey(session.key, aad, data);
}

async function copyFileAcrossTabs(
  sourceTab,
  sourcePath,
  destTab,
  destDir,
  destName,
  onProgress,
) {
  const download = await openEncryptedDownload(sourceTab, sourcePath);
  const init = await api(
    '/api/upload/init',
    { dir: destDir, name: destName, size: download.total },
    'POST',
    destTab,
  );
  let buffer = new Uint8Array(0);
  let sequence = 0;
  let uploadId = init.id;
  const flush = async () => {
    while (buffer.length > 0) {
      const take = Math.min(COPY_CHUNK, buffer.length);
      await uploadChunkToTab(destTab, uploadId, sequence, buffer.subarray(0, take));
      sequence += 1;
      buffer = buffer.subarray(take);
    }
  };
  try {
    await readDownloadFrames(download, async (plaintext, received) => {
      buffer = concatBytes(buffer, plaintext);
      while (buffer.length >= COPY_CHUNK) {
        await uploadChunkToTab(
          destTab,
          uploadId,
          sequence,
          buffer.subarray(0, COPY_CHUNK),
        );
        sequence += 1;
        buffer = buffer.subarray(COPY_CHUNK);
      }
      if (onProgress) onProgress(received, download.total);
    });
    await flush();
    const result = await api('/api/upload/finish', { id: uploadId }, 'POST', destTab);
    uploadId = null;
    return result;
  } catch (error) {
    if (uploadId && destTab.session) {
      await api('/api/upload/abort', { id: uploadId }, 'POST', destTab).catch(
        () => {},
      );
    }
    throw error;
  }
}

async function collectTree(tab, rootPath, onProgress) {
  const files = [];
  const dirs = [];
  const queue = [{ path: rootPath, relative: '' }];
  let scanned = 0;
  while (queue.length) {
    const current = queue.shift();
    let offset = 0;
    while (true) {
      const data = await api(
        '/api/list',
        { path: current.path, offset, limit: 1000 },
        'POST',
        tab,
      );
      const batch = Array.isArray(data.entries) ? data.entries : [];
      for (const entry of batch) {
        const relative = current.relative
          ? `${current.relative}/${entry.name}`
          : entry.name;
        if (entry.is_dir && !entry.is_symlink) {
          dirs.push(relative);
          queue.push({ path: joinPath(current.path, entry.name), relative });
        } else if (!entry.is_symlink) {
          files.push({
            relative,
            path: joinPath(current.path, entry.name),
            size: Number(entry.size) || 0,
          });
        }
      }
      scanned += batch.length;
      if (onProgress) onProgress(scanned);
      if (!data.has_more || !batch.length) break;
      offset += batch.length;
    }
  }
  return { files, dirs };
}

function refreshTab(tab) {
  if (tab === activeTab()) {
    void loadDir(tab.cwd, { tab, skipHistory: true });
  } else {
    tab.needsRefresh = true;
  }
}

async function pasteClipboard() {
  const clipboard = state.clipboard;
  const items = clipboardItems();
  if (!clipboard || !items.length) return;
  const sourceTab =
    clipboard.tabId === 'standalone'
      ? state.standaloneTab
      : getTab(clipboard.tabId);
  const destTab = activeTab();
  if (!sourceTab || !sourceTab.session) {
    toast('源标签已失效，请重新复制', 'error');
    state.clipboard = null;
    updatePasteButton();
    return;
  }
  if (!destTab.session) {
    toast('请先登录目标标签', 'error');
    return;
  }

  const destPath = normalizeRelPath(destTab.cwd);
  const verb = clipboard.cut ? '移动' : '复制';
  const progress = addTransferItem(
    `${verb} ${items.length} 项`,
    items.length,
    `${targetLabel(sourceTab.target)} → ${targetLabel(destTab.target)}`,
  );
  const copied = [];
  let skipped = 0;
  try {
    const existing = (await listAllEntries(destTab, destTab.cwd)).map(
      (entry) => entry.name,
    );
    for (let index = 0; index < items.length; index += 1) {
      const item = items[index];
      const sourcePath = normalizeRelPath(item.path);
      if (!sourcePath) {
        skipped += 1;
        continue;
      }
      if (
        sourceTab.id === destTab.id &&
        item.isDir &&
        (destPath === sourcePath || destPath.startsWith(`${sourcePath}/`))
      ) {
        skipped += 1;
        toast(`不能把「${item.name}」复制到自身内部`, 'error');
        continue;
      }
      if (
        sourceTab.id === destTab.id &&
        clipboard.cut &&
        parentOf(item.path) === destTab.cwd
      ) {
        skipped += 1;
        continue;
      }

      const destName = uniqueName(item.name, item.isDir, existing);
      existing.push(destName);
      progress.update(index, items.length, `正在处理「${item.name}」…`);

      if (item.isDir) {
        const tree = await collectTree(sourceTab, item.path, (scanned) => {
          progress.update(
            index,
            items.length,
            `${item.name} · 已扫描 ${scanned} 项`,
          );
        });
        const destRoot = joinPath(destTab.cwd, destName);
        const allDirs = [
          destRoot,
          ...tree.dirs.map((relative) => joinPath(destRoot, relative)),
        ];
        await ensureDirsInTab(destTab, allDirs);
        const totalBytes = tree.files.reduce(
          (sum, file) => sum + file.size,
          0,
        );
        let copiedBytes = 0;
        for (const file of tree.files) {
          const relativeDir = dirnameOf(file.relative);
          const fileDestDir = relativeDir
            ? joinPath(destRoot, relativeDir)
            : destRoot;
          const name = file.relative.slice(
            file.relative.lastIndexOf('/') + 1,
          );
          await copyFileAcrossTabs(
            sourceTab,
            file.path,
            destTab,
            fileDestDir,
            name,
            (received) => {
              progress.update(
                index,
                items.length,
                `${item.name}/${file.relative} · ${formatSize(
                  copiedBytes + received,
                )} / ${formatSize(totalBytes)}`,
              );
            },
          );
          copiedBytes += file.size;
        }
      } else {
        await copyFileAcrossTabs(
          sourceTab,
          item.path,
          destTab,
          destTab.cwd,
          destName,
          (received, total) => {
            progress.update(
              index,
              items.length,
              `${item.name} · ${formatSize(received)} / ${formatSize(total)}`,
            );
          },
        );
      }
      copied.push(item);
    }

    if (clipboard.cut && copied.length) {
      for (const item of copied) {
        await api('/api/delete', { path: item.path }, 'POST', sourceTab);
      }
      state.clipboard = null;
      updatePasteButton();
    }
    progress.done(
      `已${verb} ${copied.length} 项${skipped ? `，跳过 ${skipped} 项` : ''}`,
    );
    refreshTab(destTab);
    if (sourceTab !== destTab) refreshTab(sourceTab);
    toast(
      `${verb}完成${skipped ? `（跳过 ${skipped} 项）` : ''}`,
      'success',
    );
  } catch (error) {
    const message = error.message || '粘贴失败';
    progress.fail(message);
    toast(message, 'error');
  }
}

function handleFileShortcuts(event) {
  if (event.defaultPrevented) return;
  const target = event.target;
  const tag = target && target.tagName ? target.tagName.toLowerCase() : '';
  if (
    tag === 'input' ||
    tag === 'textarea' ||
    tag === 'select' ||
    (target && target.isContentEditable) ||
    (target &&
      typeof target.closest === 'function' &&
      target.closest('button, a, input, textarea, select'))
  ) {
    return;
  }
  const modal = $('#modal');
  if (modal && modal.open) return;
  const overlay = $('#server-overlay');
  if (overlay && !overlay.hidden) return;
  const hostkey = $('#hostkey-overlay');
  if (hostkey && !hostkey.hidden) return;
  const menu = $('#context-menu');
  if (menu && !menu.hidden) {
    if (event.key === 'Escape') {
      event.preventDefault();
      closeContextMenu();
    }
    return;
  }

  const tab = activeTab();
  if (!tab.session) return;

  if (event.key === 'ContextMenu' || (event.shiftKey && event.key === 'F10')) {
    const entries = selectedEntries(tab);
    if (!entries.length) return;
    event.preventDefault();
    const row = findRow(tab.selectedPath);
    const rect = row
      ? row.getBoundingClientRect()
      : { left: 16, top: 120, bottom: 120 };
    openRowContextMenu(rect.left + 24, rect.bottom, tab);
    return;
  }

  const key = event.key.toLowerCase();
  const mod = event.metaKey || event.ctrlKey;
  if (mod && !event.altKey && !event.shiftKey) {
    if (key === 'a') {
      event.preventDefault();
      selectAllEntries(tab);
      return;
    }
    if (key === 'c' || key === 'x') {
      const entries = selectedEntries(tab);
      if (!entries.length) return;
      event.preventDefault();
      copyEntries(entries, key === 'x', tab);
      return;
    }
    if (key === 'v') {
      if (!clipboardItems().length) return;
      event.preventDefault();
      void pasteClipboard();
    }
    return;
  }

  if (event.altKey || event.metaKey || event.ctrlKey) return;

  if (key === 'f2') {
    const entries = selectedEntries(tab);
    if (entries.length === 1) {
      event.preventDefault();
      void renameEntry(entries[0], tab);
    }
    return;
  }
  if (event.key === 'Delete') {
    const entries = selectedEntries(tab);
    if (!entries.length) return;
    event.preventDefault();
    void deleteEntries(entries, tab);
    return;
  }
  if (event.key === 'Enter') {
    const entries = selectedEntries(tab);
    if (entries.length === 1) {
      event.preventDefault();
      void openEntry(entries[0], tab);
    }
    return;
  }
  if (event.key === 'Escape') {
    if (tab.selectedPaths.size) {
      event.preventDefault();
      clearSelection(tab);
    }
    return;
  }
  if (event.key === 'F5') {
    event.preventDefault();
    void loadDir(tab.cwd, { tab });
  }
}

/* ---------- Right-click menu ---------- */

function menuButton(icon, label, handler, options = {}) {
  const button = document.createElement('button');
  button.type = 'button';
  button.className = 'context-item' + (options.danger ? ' danger' : '');
  button.setAttribute('role', 'menuitem');
  button.disabled = Boolean(options.disabled);
  button.append(icon ? iconNode(icon) : span('icon'));
  const text = span('menu-label');
  text.textContent = label;
  button.append(text);
  if (options.shortcut) {
    const hint = span('menu-shortcut');
    hint.textContent = options.shortcut;
    button.append(hint);
  }
  button.addEventListener('click', () => {
    closeContextMenu();
    handler();
  });
  return button;
}

function menuSeparator() {
  return div('context-sep');
}

function closeContextMenu() {
  const menu = $('#context-menu');
  if (!menu) return;
  menu.hidden = true;
  menu.replaceChildren();
}

function showContextMenuAt(menu, x, y) {
  menu.hidden = false;
  menu.style.left = '0px';
  menu.style.top = '0px';
  const rect = menu.getBoundingClientRect();
  const left = Math.max(8, Math.min(x, window.innerWidth - rect.width - 8));
  const top = Math.max(8, Math.min(y, window.innerHeight - rect.height - 8));
  menu.style.left = `${left}px`;
  menu.style.top = `${top}px`;
}

function openRowContextMenu(x, y, tab = activeTab()) {
  const menu = $('#context-menu');
  if (!menu) return;
  const entries = selectedEntries(tab);
  if (!entries.length) return;
  menu.replaceChildren();
  const single = entries.length === 1 ? entries[0] : null;

  if (single) {
    const fullPath = entryPath(tab, single);
    menu.append(menuButton('eye', '打开', () => void openEntry(single, tab)));
    if (single.is_dir) {
      menu.append(
        menuButton('download', '下载文件夹 (.tar)', () =>
          startFolderDownload(fullPath, single.name, tab),
        ),
      );
    } else {
      if (previewKindOf(single.name) !== 'none') {
        menu.append(
          menuButton('eye', '预览', () => openPreview(single, tab)),
        );
      }
      menu.append(
        menuButton('download', '下载', () =>
          startDownload(fullPath, single.name, tab),
        ),
      );
    }
    menu.append(menuSeparator());
  }

  const count = entries.length;
  const suffix = count > 1 ? ` ${count} 项` : '';
  menu.append(
    menuButton('copy', `复制${suffix}`, () => copyEntries(entries, false, tab), {
      shortcut: 'Ctrl+C',
    }),
    menuButton('cut', `剪切${suffix}`, () => copyEntries(entries, true, tab), {
      shortcut: 'Ctrl+X',
    }),
  );

  if (single) {
    menu.append(menuSeparator());
    menu.append(
      menuButton('edit', '重命名', () => void renameEntry(single, tab), {
        shortcut: 'F2',
      }),
    );
  } else {
    menu.append(menuSeparator());
  }
  menu.append(
    menuButton(
      'trash',
      count > 1 ? `删除 ${count} 项` : '删除',
      () => void deleteEntries(entries, tab),
      { shortcut: 'Delete', danger: true },
    ),
  );
  showContextMenuAt(menu, x, y);
}

function openBackgroundContextMenu(x, y, tab = activeTab()) {
  const menu = $('#context-menu');
  if (!menu) return;
  clearSelection(tab);
  menu.replaceChildren();
  menu.append(
    menuButton('folder', '新建文件夹', () => void createFolder()),
    menuButton('copy', '粘贴', () => void pasteClipboard(), {
      shortcut: 'Ctrl+V',
      disabled: clipboardItems().length === 0,
    }),
    menuSeparator(),
    menuButton(null, '全选', () => selectAllEntries(tab), {
      shortcut: 'Ctrl+A',
    }),
    menuButton('refresh', '刷新', () => void loadDir(tab.cwd, { tab }), {
      shortcut: 'F5',
    }),
  );
  showContextMenuAt(menu, x, y);
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

async function startDownload(path, name, tab = activeTab()) {
  recordHistory('file', path, name, parentOf(path), tab);
  await streamEncryptedDownload({
    endpoint: '/api/download',
    payload: { path },
    suggestedName: name,
    expectEndMarker: false,
    title: `下载 ${name}`,
    tab,
  });
}

async function startFolderDownload(path, name, tab = activeTab()) {
  recordHistory('dir', path, name, parentOf(path), tab);
  await streamEncryptedDownload({
    endpoint: '/api/download_folder',
    payload: { path },
    suggestedName: `${name}.tar`,
    expectEndMarker: true,
    title: `打包 ${name}`,
    tab,
  });
}

async function streamEncryptedDownload({
  endpoint,
  payload,
  suggestedName,
  expectEndMarker,
  title,
  tab = activeTab(),
}) {
  let target;
  try {
    target = await chooseSaveTarget(suggestedName);
  } catch (error) {
    if (error && error.name === 'AbortError') return;
    target = { writable: null, usePicker: false };
  }

  const progress = progressToast(title);
  const session = tab.session;
  if (!session) {
    progress.fail('会话已失效，请重新登录');
    toast('会话已失效，请重新登录', 'error');
    return;
  }
  try {
    const aad = `POST ${endpoint}`;
    const envelope = await encryptWithKey(session.key, aad, payload);
    const response = await fetch(endpoint, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        Authorization: `Bearer ${session.token}`,
        ...targetHeaders(tab),
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
      if (response.status === 401) handleSessionLost('会话已过期，请重新登录', tab);
      throw new Error(message || `下载失败 (${response.status})`);
    }

    const total = Number(response.headers.get('x-file-size') || '0');
    const chunkSize = Number(response.headers.get('x-chunk-size') || '65536');
    if (!target.usePicker && !expectEndMarker && total > BLOB_DOWNLOAD_LIMIT) {
      throw new Error('当前浏览器不支持流式保存，超过 1 GB 的文件请使用 Chrome 或 Edge 下载');
    }
    if (expectEndMarker) progress.update(null, '正在打包…');

    const parts = target.usePicker ? null : [];
    let pending = new Uint8Array(0);
    let sequence = 0;
    let received = 0;
    let ended = false;

    outer: for await (const value of responseChunks(response)) {
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
              session.key,
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

/** Safari < 14.1 and the WKWebView builds shipped with older macOS versions
 *  expose no streaming body on fetch responses. Buffer the whole response
 *  instead: the blob path below has to hold it in memory anyway. */
async function* responseChunks(response) {
  if (response.body && typeof response.body.getReader === 'function') {
    const reader = response.body.getReader();
    while (true) {
      const { done, value } = await reader.read();
      if (done) return;
      if (value) yield value;
    }
  } else {
    yield new Uint8Array(await response.arrayBuffer());
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

function enqueueUploads(input, dir, directories = [], tab = activeTab()) {
  const items = toUploadItems(input);
  const base = dir || tab.cwd;
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
    void ensureDirectories(base, [...directorySet], tab);
  }

  for (const item of items) {
    const parent = dirnameOf(item.relative);
    uploadQueue.push({
      file: item.file,
      dir: parent ? joinPath(base, parent) : base,
      display: item.relative,
      tab,
    });
  }
  $('#upload-panel').hidden = false;
  void pumpUploads();
}

async function ensureDirectories(base, relatives, tab = activeTab()) {
  try {
    await ensureDirsInTab(
      tab,
      relatives.map((relative) => joinPath(base, relative)),
    );
  } catch (error) {
    // upload_init also creates parent folders lazily, so a failure here does
    // not abort the queue. It is no longer swallowed: surface it so a missing
    // empty directory is not mistaken for a successful upload.
    toast(`创建目录失败：${error?.message || error}`, 'error');
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
      await uploadFile(item.file, item.dir, item.display, item.tab);
    }
  } finally {
    uploadRunning = false;
  }
}

async function uploadFile(file, dir, display, tab = activeTab()) {
  const item = addUploadItem(display || file.name, file.size, dir);
  const session = tab.session;
  let uploadId = null;
  if (!session) {
    item.fail('会话已失效，请重新登录');
    return;
  }
  try {
    const init = await api(
      '/api/upload/init',
      {
        dir,
        name: file.name,
        size: file.size,
      },
      'POST',
      tab,
    );
    uploadId = init.id;

    const chunkSize = 2 * 1024 * 1024;
    let offset = 0;
    let sequence = 0;
    while (offset < file.size) {
      const end = Math.min(offset + chunkSize, file.size);
      const plaintext = new Uint8Array(await readBlob(file.slice(offset, end)));
      const iv = randomBytes(12);
      const path = `/api/upload/chunk/${encodeURIComponent(uploadId)}/${sequence}`;
      const aad = `PUT ${path}`;
      const ciphertext = new Uint8Array(
        await crypto.subtle.encrypt(
          { name: 'AES-GCM', iv, additionalData: encoder.encode(aad) },
          session.key,
          plaintext,
        ),
      );
      const frame = makeFrame(iv, ciphertext);
      const response = await fetch(path, {
        method: 'PUT',
        headers: {
          'Content-Type': 'application/octet-stream',
          Authorization: `Bearer ${session.token}`,
          ...targetHeaders(tab),
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
        if (response.status === 401) handleSessionLost('会话已过期，请重新登录', tab);
        throw new Error(data && data.error ? data.error : `上传失败 (${response.status})`);
      }
      await decryptWithKey(session.key, aad, data);

      offset = end;
      sequence += 1;
      item.update(
        file.size === 0 ? 1 : offset / file.size,
        `${formatSize(offset)} / ${formatSize(file.size)}`,
      );
    }

    const result = await api('/api/upload/finish', { id: uploadId }, 'POST', tab);
    uploadId = null;
    item.done(result.path || file.name);
    toast(`已上传：${display || file.name}`, 'success');
    if (tab === activeTab() && (dir === tab.cwd || dir.startsWith(`${tab.cwd === '/' ? '' : tab.cwd}/`))) {
      await loadDir(tab.cwd, { tab });
    } else {
      tab.needsRefresh = true;
    }
  } catch (error) {
    const message = error.message || '上传失败';
    item.fail(message);
    toast(message, 'error');
    if (uploadId && tab.session) {
      try {
        await api('/api/upload/abort', { id: uploadId }, 'POST', tab);
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

function openPreview(entry, tab = activeTab()) {
  recordHistory('file', joinPath(tab.cwd, entry.name), entry.name, tab.cwd, tab);
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
        startDownload(joinPath(tab.cwd, currentEntry.name), currentEntry.name, tab);

      const generation = ++renderGeneration;
      releaseObjectUrls();
      content.className = 'preview-content';
      const loading = div('preview-loading');
      loading.textContent = '正在准备预览…';
      content.replaceChildren(loading);

      try {
        await renderPreview({ content, entry: nextEntry, kind: nextKind, trackUrl, tab });
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

async function requestPreviewStream(path, tab = activeTab()) {
  const endpoint = '/api/preview/stream';
  const session = tab.session;
  if (!session) throw new Error('会话已失效，请重新登录');
  const aad = `POST ${endpoint}`;
  const envelope = await encryptWithKey(session.key, aad, { path });
  const response = await fetch(endpoint, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${session.token}`,
      ...targetHeaders(tab),
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
    if (response.status === 401) handleSessionLost('会话已过期，请重新登录', tab);
    throw new Error(message || `读取文件失败 (${response.status})`);
  }
  const streamId = response.headers.get('x-stream-id');
  if (!streamId || !response.body) throw new Error('服务器响应缺少加密流信息');
  return {
    reader: response.body.getReader(),
    streamId,
    key: session.key,
    size: Number(response.headers.get('x-file-size') || 0),
    chunkSize: Number(response.headers.get('x-chunk-size') || 65536),
    contentType:
      response.headers.get('x-preview-content-type') || 'application/octet-stream',
  };
}

async function readEncryptedPreview(source, onPlaintext) {
  if (!source.key) throw new Error('会话已失效，请重新登录');
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
              source.key,
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

async function renderPreview({ content, entry, kind, trackUrl, tab = activeTab() }) {
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

  const path = joinPath(tab.cwd, entry.name);

  if (kind === 'image' || kind === 'svg') {
    content.classList.add('media-mode');
    const wrapper = div('preview-media-wrap');
    const status = mediaStatus('正在接收图片…');
    wrapper.append(status);
    content.replaceChildren(wrapper);

    const source = await requestPreviewStream(path, tab);
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

    const source = await requestPreviewStream(path, tab);
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

    const source = await requestPreviewStream(path, tab);
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

    const source = await requestPreviewStream(path, tab);
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

    const source = await requestPreviewStream(path, tab);
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

    const source = await requestPreviewStream(path, tab);
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

    const source = await requestPreviewStream(path, tab);
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

    const source = await requestPreviewStream(path, tab);
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
        const tab = activeTab();
        if (tab.session) tab.session.user = result.user;
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

function recordHistory(type, path, name, parent, tab = activeTab()) {
  const clean = normalizeRelPath(path);
  if (!clean && type !== 'dir') return;
  const entry = {
    type,
    path: clean ? `/${clean}` : '/',
    name: name || basenameOf(path),
    parent: parent || parentOf(path),
    target: tab.id,
    targetName: targetLabel(tab.target),
    at: Date.now(),
  };
  const items = readHistory().filter(
    (item) =>
      !(
        item.type === entry.type &&
        item.path === entry.path &&
        (item.target || '') === entry.target
      ),
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
        meta.textContent = `${item.type === 'dir' ? '文件夹' : '文件'}${
          item.targetName ? ` · ${item.targetName}` : ''
        } · ${item.parent === '/' ? '根目录' : item.parent}`;
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
  let tab = activeTab();
  if (item.target && item.target !== 'standalone') {
    tab = getTab(item.target);
    if (!tab) {
      toast('目标服务器已删除，无法跳转', 'error');
      return;
    }
    await activateTab(tab.id);
    tab = activeTab();
    if (!tab.session) {
      toast('请先登录该服务器后再跳转', 'info');
      return;
    }
  }
  if (item.type === 'dir') {
    await loadDir(item.path, { tab });
    return;
  }
  tab.pendingHighlight = item.path;
  await loadDir(item.parent || '/', { tab });
}

/* ---------- Desktop workspace: local + server tabs ---------- */

function showAppView() {
  $('#login-view').hidden = true;
  $('#app-view').hidden = false;
}

function showTabLogin(tab, message = '') {
  loginTab = tab;
  const passwordless = isPasswordless(tab.target);
  $('#app-view').hidden = true;
  $('#login-view').hidden = false;
  $('#login-desc').textContent = passwordless
    ? tab.target.kind === 'local'
      ? '本机文件服务未就绪。点击下方按钮重新连接本机。'
      : `连接 ${targetLabel(tab.target)}（${targetDetail(tab.target)}），文件会显示在当前标签中。`
    : tab.target.kind === 'standalone'
      ? '登录后即可安全地浏览、上传与下载文件。'
      : `登录 ${targetLabel(tab.target)}，文件会显示在当前标签中。`;
  for (const field of document.querySelectorAll('#login-form .field')) {
    field.hidden = passwordless;
  }
  $('#login-btn').textContent = passwordless
    ? tab.target.kind === 'local'
      ? '重新连接本机'
      : '连接服务器'
    : '加密登录';
  $('#username').value = '';
  $('#password').value = '';
  showLoginError(message || '');
  if (!passwordless) setTimeout(() => $('#username').focus(), 30);
}

function ensureTab(target) {
  let tab = state.tabs.get(target.id);
  if (!tab) {
    tab = makeTab(target);
    state.tabs.set(target.id, tab);
  } else {
    tab.target = target;
  }
  return tab;
}

function syncTabs(info) {
  const ids = new Set((info.targets || []).map((target) => target.id));
  for (const id of [...state.tabs.keys()]) {
    if (!ids.has(id)) state.tabs.delete(id);
  }
  for (const target of info.targets || []) ensureTab(target);
  if (state.activeTabId && !state.tabs.has(state.activeTabId)) {
    state.activeTabId = null;
  }
}

function renderTabs() {
  const bar = $('#workspace-bar');
  if (!state.app) {
    bar.hidden = true;
    return;
  }
  bar.hidden = false;
  const container = $('#tabs');
  container.replaceChildren();
  for (const target of state.app.targets || []) {
    const tab = ensureTab(target);
    const active = target.id === state.activeTabId;
    const button = buttonEl(
      '',
      `workspace-tab ${
        target.kind === 'local'
          ? 'local'
          : target.kind === 'ssh'
            ? 'ssh'
            : 'remote'
      }${tab.session ? '' : ' locked'}${active ? ' active' : ''}`,
    );
    button.dataset.tabId = target.id;
    button.setAttribute('role', 'tab');
    button.setAttribute('aria-selected', active ? 'true' : 'false');
    const dot = span('tab-dot');
    const label = span('workspace-tab-label');
    label.textContent = targetLabel(target);
    button.append(dot, label);
    button.title =
      target.kind === 'local'
        ? '本机文件系统'
        : `${targetDetail(target)}${tab.session ? '' : '（未连接）'}`;
    button.addEventListener('click', () => void activateTab(target.id));
    container.append(button);
  }
  updatePasteButton();
}

async function activateTab(id, options = {}) {
  let tab = getTab(id);
  if (!tab && id === 'standalone') tab = state.standaloneTab;
  if (!tab) return;
  state.activeTabId = state.tabs.has(id) ? id : null;
  loginTab = tab.session ? null : tab;
  renderTabs();
  renderRootSelect();
  updatePasteButton();
  if (!tab.session) {
    if (isPasswordless(tab.target)) {
      try {
        await enterLocalTab(tab);
      } catch (error) {
        showTabLogin(tab, error.message || '连接失败');
      }
      return;
    }
    showTabLogin(tab);
    return;
  }
  showAppView();
  updateTabChrome(tab);
  if (options.reload || !tab.loaded || tab.needsRefresh) {
    await loadDir(tab.cwd, { tab, skipHistory: true });
  } else {
    renderTabContent(tab);
  }
}

function updateTabChrome(tab) {
  const passwordless = isPasswordless(tab.target);
  $('#user-badge').textContent = passwordless
    ? tab.target.kind === 'local'
      ? '本机'
      : targetDetail(tab.target)
    : tab.session?.user || targetLabel(tab.target);
  $('#btn-account').hidden = passwordless || !tab.session;
  $('#btn-logout').hidden = passwordless || !tab.session;
  $('#session-note').textContent = passwordless
    ? tab.target.kind === 'local'
      ? '本机文件通过回环服务访问，不上传任何数据'
      : 'SSH 加密通道直连服务器，文件不经过第三方'
    : '会话密钥仅保存在本页内存，刷新后需重新登录';
  $('#local-root-wrap').hidden = !passwordless;
  renderRootSelect();
}

function renderTabContent(tab) {
  $('#filter').value = tab.filter || '';
  renderBreadcrumbs();
  showListSkeleton(false);
  renderFiles();
  $('#free-space').textContent =
    typeof tab.freeSpace === 'number' ? `剩余 ${formatSize(tab.freeSpace)}` : '';
}

async function enterLocalTab(tab) {
  const response = await fetch('/api/local-login', {
    cache: 'no-store',
    headers: targetHeaders(tab),
  });
  if (!response.ok) {
    let data = null;
    try {
      data = await response.json();
    } catch {
      data = null;
    }
    if (
      data &&
      data.host_key &&
      (data.code === 'host_key_unknown' || data.code === 'host_key_mismatch')
    ) {
      const accepted = await confirmHostKey(
        tab.target,
        data.host_key,
        data.previous_fingerprint,
      );
      if (accepted) return enterLocalTab(tab);
    }
    const failure = new Error(
      (data && (data.error || data.message)) ||
        (response.status === 401 ? '用户名或密码错误' : '连接失败'),
    );
    failure.data = data;
    throw failure;
  }
  const data = await response.json();
  tab.session = {
    key: await importAesKey(base64ToBytes(data.key)),
    token: data.token,
    user: data.user || 'local',
  };
  await refreshTargetRoots(tab);
  renderTabs();
  if (tab === activeTab()) {
    showAppView();
    updateTabChrome(tab);
    await loadDir(tab.cwd || '/', { tab, skipHistory: true });
  }
}

/// SSH roots live in the per-target service, so ask it directly; the local
/// service reports its roots through /app/info.
async function refreshTargetRoots(tab) {
  if (tab.target.kind === 'local') {
    tab.roots = state.app?.local?.roots || null;
    tab.rootPath = state.app?.local?.root || null;
    return;
  }
  if (tab.target.kind !== 'ssh') return;
  try {
    const response = await fetch('/app/info', {
      cache: 'no-store',
      headers: targetHeaders(tab),
    });
    if (!response.ok) return;
    const info = await response.json();
    if (Array.isArray(info.roots)) tab.roots = info.roots;
    if (typeof info.root === 'string') tab.rootPath = info.root;
  } catch {
    /* keep whatever roots we already had */
  }
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
  if (!info || !info.mode) {
    renderTabs();
    return; // standalone server: normal login flow
  }

  state.app = info;
  document.body.classList.add('has-workspace');
  $('#workspace-bar').hidden = false;
  $('#app-version').textContent = `v${info.version || ''}`;
  syncTabs(info);
  renderTabs();
  // Best effort and non-blocking: an offline desktop must stay fully usable.
  void checkForAppUpdate(false);
  const localTarget = (info.targets || []).find((target) => target.kind === 'local');
  const initial = localTarget || (info.targets || [])[0];
  if (!initial) {
    showTabLogin(state.standaloneTab, '尚未配置任何文件位置');
    return;
  }
  state.activeTabId = initial.id;
  const tab = ensureTab(initial);
  renderTabs();
  renderRootSelect();
  if (isPasswordless(tab.target)) {
    try {
      await enterLocalTab(tab);
    } catch (error) {
      showTabLogin(tab, error.message || '无法连接');
    }
  } else {
    showTabLogin(tab);
  }
}

function rootOptionLabel(value) {
  return String(value || '').replace(/^\\\\\?\\/, '');
}

function renderRootSelect() {
  const info = state.app;
  const wrap = $('#local-root-wrap');
  const tab = activeTab();
  if (!info || !tab || !isPasswordless(tab.target)) {
    wrap.hidden = true;
    return;
  }
  const local = tab.target.kind === 'local';
  const roots = local ? info.local && info.local.roots : tab.roots;
  const current = local ? info.local && info.local.root : tab.rootPath;
  if (!Array.isArray(roots) || roots.length === 0) {
    wrap.hidden = true;
    return;
  }
  wrap.hidden = false;
  const select = $('#root-select');
  select.replaceChildren();
  let matched = false;
  for (const root of roots) {
    const option = document.createElement('option');
    option.value = root.path;
    option.textContent = rootOptionLabel(root.name || root.path);
    if (root.path === current) {
      option.selected = true;
      matched = true;
    }
    select.append(option);
  }
  if (!matched && current) {
    const option = document.createElement('option');
    option.value = current;
    option.textContent = rootOptionLabel(current);
    option.selected = true;
    select.append(option);
  }
}

async function refreshAppInfo() {
  try {
    const response = await fetch('/app/info', { cache: 'no-store' });
    if (!response.ok) return null;
    state.app = await response.json();
    syncTabs(state.app);
    renderTabs();
    renderRootSelect();
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
    const error = new Error(
      data && (data.error || data.message) ? data.error || data.message : '操作失败',
    );
    error.data = data;
    error.code = data && data.code;
    throw error;
  }
  return data;
}

/* ---------- Host key confirmation (TOFU) ---------- */

/// Shows the SHA256 fingerprint and resolves true only after the user trusts
/// it. Mismatches offer to forget the old pin instead of silently replacing it.
function confirmHostKey(target, info, previousFingerprint) {
  return new Promise((resolve) => {
    const overlay = $('#hostkey-overlay');
    const panel = $('#hostkey-panel');
    if (!overlay || !panel) {
      resolve(false);
      return;
    }
    const mismatch = Boolean(previousFingerprint);
    const body = div('modal-body');
    const heading = document.createElement('h2');
    heading.textContent = mismatch ? '主机密钥已变更' : '确认主机指纹';

    const text = document.createElement('p');
    text.className = 'modal-text';
    text.textContent = mismatch
      ? `${info.host}:${info.port} 出示的密钥与之前保存的不一致。这可能意味着服务器重装过，也可能有人正在拦截连接。`
      : `首次连接 ${info.host}:${info.port}。请与服务器管理员核对下面的 SHA256 指纹，确认一致后再继续。`;

    const fingerprint = div('hostkey-fingerprint');
    const fingerprintLabel = span('hostkey-label');
    fingerprintLabel.textContent = `SHA256 指纹 · ${info.algorithm || ''}`;
    const code = document.createElement('code');
    code.textContent = info.fingerprint || '';
    fingerprint.append(fingerprintLabel, code);

    if (mismatch && previousFingerprint) {
      const previous = div('hostkey-fingerprint previous');
      const previousLabel = span('hostkey-label');
      previousLabel.textContent = '之前保存的指纹';
      const old = document.createElement('code');
      old.textContent = previousFingerprint;
      previous.append(previousLabel, old);
      body.append(heading, text, fingerprint, previous);
    } else {
      body.append(heading, text, fingerprint);
    }

    const error = div('form-error inline-error');
    const actions = div('form-actions');
    const cancel = buttonEl('取消', 'btn ghost');
    const confirm = buttonEl(mismatch ? '清除旧指纹并重试' : '信任并连接', 'btn primary');
    actions.append(confirm, cancel);
    body.append(error, actions);
    panel.replaceChildren(body);
    overlay.hidden = false;

    let settled = false;
    let onKey = null;
    const finish = (value) => {
      if (settled) return;
      settled = true;
      if (onKey) document.removeEventListener('keydown', onKey);
      overlay.onclick = null;
      overlay.hidden = true;
      panel.replaceChildren();
      resolve(value);
    };
    cancel.addEventListener('click', () => finish(false));
    overlay.onclick = (event) => {
      if (event.target.closest('[data-hostkey-cancel]')) finish(false);
    };
    onKey = (event) => {
      if (event.key === 'Escape') finish(false);
    };
    document.addEventListener('keydown', onKey);
    confirm.addEventListener('click', async () => {
      error.textContent = '';
      confirm.disabled = true;
      try {
        if (mismatch) {
          await serversAction({ action: 'forget_host', id: target.id });
          finish(true);
          return;
        }
        await serversAction({
          action: 'trust_host',
          id: target.id,
          fingerprint: info.fingerprint,
        });
        finish(true);
      } catch (failure) {
        error.textContent = failure.message || '操作失败';
        confirm.disabled = false;
      }
    });
    setTimeout(() => confirm.focus(), 30);
  });
}

/* ---------- Add / manage servers (custom overlay, WKWebView safe) ---------- */

let serverPanelEditing = null;

function closeServerPanel() {
  const overlay = $('#server-overlay');
  if (overlay) overlay.hidden = true;
  const panel = $('#server-panel');
  if (panel) panel.replaceChildren();
  serverPanelEditing = null;
}

function openServersDialog() {
  if (!state.app) return;
  const overlay = $('#server-overlay');
  const panel = $('#server-panel');
  overlay.hidden = false;

  const body = div('modal-body');
  const heading = document.createElement('h2');
  heading.textContent = '连接管理';
  const hint = document.createElement('p');
  hint.className = 'modal-text';
  hint.textContent =
    '本机文件始终显示在第一个标签。这里可以添加 AgentGod Files 服务器（HTTPS / HTTP），或通过 SSH/SFTP 直接管理远程服务器的文件。';

  const close = buttonEl('', 'btn ghost icon-only panel-close');
  close.innerHTML = ICONS.close;
  close.title = '关闭';
  close.setAttribute('aria-label', '关闭');
  close.addEventListener('click', closeServerPanel);

  const list = div('servers-list');
  const form = div('server-form');

  const nameField = labeledInput('名称', 'text', 'off');
  nameField.input.placeholder = '例如：办公室 NAS';

  const kindField = document.createElement('label');
  kindField.className = 'field protocol';
  const kindLabel = span('类型');
  const kindSelect = document.createElement('select');
  kindSelect.className = 'app-select';
  kindSelect.innerHTML =
    '<option value="http">AgentGod 服务器</option><option value="ssh">SSH / SFTP</option>';
  kindField.append(kindLabel, kindSelect);

  const kindRow = div('form-row');
  kindRow.append(kindField, nameField.field);

  /* ---------- AgentGod server fields ---------- */
  const httpGroup = div('server-group');
  const protocolField = document.createElement('label');
  protocolField.className = 'field protocol';
  const protocolLabel = span('协议');
  const protocolSelect = document.createElement('select');
  protocolSelect.className = 'app-select';
  protocolSelect.innerHTML =
    '<option value="https">HTTPS</option><option value="http">HTTP</option>';
  protocolField.append(protocolLabel, protocolSelect);
  const urlField = labeledInput('服务器地址', 'text', 'off');
  urlField.input.placeholder = '192.168.1.10:8090 或完整 URL';
  const httpRow = div('form-row');
  httpRow.append(protocolField, urlField.field);

  const checkRow = document.createElement('label');
  checkRow.className = 'check';
  const checkBox = document.createElement('input');
  checkBox.type = 'checkbox';
  const checkText = span('');
  checkText.textContent = '允许自签名证书（首次连接常见）';
  checkRow.append(checkBox, checkText);

  httpGroup.append(httpRow, checkRow);

  /* ---------- SSH / SFTP fields ---------- */
  const sshGroup = div('server-group');
  const hostField = labeledInput('主机', 'text', 'off');
  hostField.input.placeholder = '192.168.1.20 或 example.com';
  hostField.field.classList.add('grow');
  const portField = labeledInput('端口', 'number', 'off');
  portField.input.value = '22';
  portField.input.min = '1';
  portField.input.max = '65535';
  portField.field.classList.add('port');
  const hostRow = div('form-row');
  hostRow.append(hostField.field, portField.field);

  const userField = labeledInput('用户名', 'text', 'username');
  userField.input.placeholder = 'root';
  const authField = document.createElement('label');
  authField.className = 'field protocol';
  const authLabel = span('认证方式');
  const authSelect = document.createElement('select');
  authSelect.className = 'app-select';
  authSelect.innerHTML =
    '<option value="password">密码</option><option value="key">私钥</option>';
  authField.append(authLabel, authSelect);
  const userRow = div('form-row');
  userRow.append(userField.field, authField);

  const passwordField = labeledInput('密码', 'password', 'current-password');
  const keyField = labeledInput('私钥文件', 'text', 'off');
  keyField.input.placeholder = 'C:\\Users\\you\\.ssh\\id_ed25519';
  const passphraseField = labeledInput('私钥口令（可选）', 'password', 'off');
  const rootField = labeledInput('起始目录（可选）', 'text', 'off');
  rootField.input.placeholder = '/var/www（留空进入主目录）';

  sshGroup.append(
    hostRow,
    userRow,
    passwordField.field,
    keyField.field,
    passphraseField.field,
    rootField.field,
  );

  const errorEl = div('form-error inline-error');
  const formActions = div('form-actions');
  const saveButton = buttonEl('添加服务器', 'btn primary');
  const cancelEdit = buttonEl('取消编辑', 'btn ghost small');
  cancelEdit.hidden = true;
  formActions.append(saveButton, cancelEdit);

  form.append(kindRow, httpGroup, sshGroup, errorEl, formActions);

  const footer = div('modal-actions');
  const done = buttonEl('关闭', 'btn ghost');
  done.addEventListener('click', closeServerPanel);
  footer.append(done);

  body.append(heading, hint, close, list, form, footer);
  panel.replaceChildren(body);

  let editingTarget = null;

  const updateAuthFields = () => {
    const usesKey = authSelect.value === 'key';
    passwordField.field.hidden = usesKey;
    keyField.field.hidden = !usesKey;
    passphraseField.field.hidden = !usesKey;
  };

  const applyKind = (kind) => {
    httpGroup.hidden = kind !== 'http';
    sshGroup.hidden = kind !== 'ssh';
    if (kind === 'ssh') updateAuthFields();
    if (!editingTarget) {
      saveButton.textContent = kind === 'ssh' ? '添加 SSH 连接' : '添加服务器';
    }
  };

  const setKind = (kind) => {
    kindSelect.value = kind;
    kindSelect.disabled = Boolean(serverPanelEditing);
    applyKind(kind);
  };

  const resetForm = () => {
    serverPanelEditing = null;
    editingTarget = null;
    nameField.input.value = '';
    urlField.input.value = '';
    protocolSelect.value = 'https';
    checkBox.checked = false;
    hostField.input.value = '';
    portField.input.value = '22';
    userField.input.value = '';
    authSelect.value = 'password';
    passwordField.input.value = '';
    passwordField.input.placeholder = '';
    keyField.input.value = '';
    passphraseField.input.value = '';
    passphraseField.input.placeholder = '';
    rootField.input.value = '';
    saveButton.textContent = '添加服务器';
    cancelEdit.hidden = true;
    errorEl.textContent = '';
    setKind('http');
  };

  const beginEdit = (target) => {
    serverPanelEditing = target.id;
    editingTarget = target;
    errorEl.textContent = '';
    nameField.input.value = target.name || '';
    if (target.kind === 'ssh') {
      setKind('ssh');
      hostField.input.value = target.host || '';
      portField.input.value = String(target.port || 22);
      userField.input.value = target.user || '';
      authSelect.value = target.auth === 'key' ? 'key' : 'password';
      keyField.input.value = target.key_path || '';
      rootField.input.value = target.root || '';
      passwordField.input.value = '';
      passphraseField.input.value = '';
      passwordField.input.placeholder = '留空保持不变';
      passphraseField.input.placeholder = '留空保持不变';
      updateAuthFields();
    } else {
      setKind('http');
      const url = String(target.url || '');
      protocolSelect.value = url.startsWith('http://') ? 'http' : 'https';
      urlField.input.value = url.replace(/^https?:\/\//, '');
      checkBox.checked = Boolean(target.allow_invalid_certs);
    }
    saveButton.textContent = '保存修改';
    cancelEdit.hidden = false;
    nameField.input.focus();
  };

  const drawList = () => {
    list.replaceChildren();
    const remotes = (state.app.targets || []).filter(
      (target) => target.kind !== 'local',
    );
    if (!remotes.length) {
      const empty = div('servers-empty');
      empty.textContent = '还没有保存的连接，请在下方添加。';
      list.append(empty);
      return;
    }
    for (const target of remotes) {
      const row = div('server-row');
      const info = div('server-info');
      const title = span('server-name');
      title.textContent = target.name || targetDetail(target);
      if (target.id === state.activeTabId) {
        const badge = span('server-badge');
        badge.textContent = '当前';
        title.append(badge);
      }
      const kindBadge = span('server-badge kind');
      kindBadge.textContent = target.kind === 'ssh' ? 'SSH' : 'HTTP';
      title.append(kindBadge);

      const meta = span('server-url');
      meta.textContent =
        target.kind === 'ssh'
          ? `${targetDetail(target)}${target.root ? ` · ${target.root}` : ''}`
          : target.url || '';
      info.append(title, meta);

      const actions = div('server-actions');
      const test = buttonEl('测试', 'btn ghost small');
      test.addEventListener('click', () => void testTarget(target));
      const edit = buttonEl('编辑', 'btn ghost small');
      edit.addEventListener('click', () => beginEdit(target));
      const remove = buttonEl('删除', 'btn ghost small danger-text');
      remove.addEventListener('click', async () => {
        errorEl.textContent = '';
        try {
          await serversAction({ action: 'remove', id: target.id });
          const wasActive = state.activeTabId === target.id;
          state.tabs.delete(target.id);
          if (wasActive) state.activeTabId = null;
          await refreshAppInfo();
          drawList();
          resetForm();
          if (wasActive) {
            const local = (state.app.targets || []).find(
              (item) => item.kind === 'local',
            );
            if (local) await activateTab(local.id);
          }
        } catch (error) {
          errorEl.textContent = error.message || '删除失败';
        }
      });
      actions.append(test, edit, remove);
      row.append(info, actions);
      list.append(row);
    }
  };

  const testTarget = async (target, attempt = 0) => {
    errorEl.textContent = '';
    try {
      await serversAction({ action: 'test', id: target.id });
      toast(
        target.kind === 'ssh'
          ? `SSH 连接成功：${targetDetail(target)}`
          : '服务器连接成功',
        'info',
      );
      return true;
    } catch (failure) {
      const data = failure.data || {};
      const hostKeyIssue =
        data.code === 'host_key_unknown' || data.code === 'host_key_mismatch';
      if (hostKeyIssue && data.host_key && attempt < 2) {
        const accepted = await confirmHostKey(
          target,
          data.host_key,
          data.previous_fingerprint,
        );
        if (accepted) return testTarget(target, attempt + 1);
        return false;
      }
      errorEl.textContent = failure.message || '连接失败';
      return false;
    }
  };

  const submit = async () => {
    errorEl.textContent = '';
    const kind = kindSelect.value;
    const name = nameField.input.value.trim();
    if (!name) {
      errorEl.textContent = '请填写名称';
      return;
    }

    let payload;
    if (kind === 'ssh') {
      const host = hostField.input.value.trim();
      const username = userField.input.value.trim();
      const port = Number(portField.input.value);
      const auth = authSelect.value === 'key' ? 'key' : 'password';
      const keyPath = keyField.input.value.trim();
      const root = rootField.input.value.trim();
      if (!host) {
        errorEl.textContent = '请填写主机地址';
        return;
      }
      if (!username) {
        errorEl.textContent = '请填写用户名';
        return;
      }
      if (!Number.isInteger(port) || port < 1 || port > 65535) {
        errorEl.textContent = '端口必须在 1-65535 之间';
        return;
      }
      if (auth === 'key' && !keyPath) {
        errorEl.textContent = '请选择私钥文件';
        return;
      }
      if (
        auth === 'password' &&
        !serverPanelEditing &&
        !passwordField.input.value
      ) {
        errorEl.textContent = '请输入登录密码';
        return;
      }
      payload = {
        name,
        host,
        port,
        username,
        auth,
        key_path: keyPath,
        root: root,
        password: auth === 'password' ? passwordField.input.value : '',
        passphrase: auth === 'key' ? passphraseField.input.value : '',
      };
      if (serverPanelEditing) {
        payload.action = 'update_ssh';
        payload.id = serverPanelEditing;
      } else {
        payload.action = 'add_ssh';
      }
    } else {
      let url = urlField.input.value.trim();
      if (!url) {
        errorEl.textContent = '请填写服务器地址';
        return;
      }
      if (!url.includes('://')) url = `${protocolSelect.value}://${url}`;
      payload = { name, url, allow_invalid_certs: checkBox.checked };
      if (serverPanelEditing) {
        payload.action = 'update';
        payload.id = serverPanelEditing;
      } else {
        payload.action = 'add';
      }
    }

    const editing = serverPanelEditing;
    saveButton.disabled = true;
    try {
      const result = await serversAction(payload);
      const id = editing || result.id;
      await refreshAppInfo();
      const tab = getTab(id);
      if (tab) {
        tab.session = null;
        tab.loaded = false;
        tab.needsRefresh = false;
        tab.roots = null;
        tab.rootPath = null;
      }
      resetForm();
      drawList();
      closeServerPanel();
      renderTabs();
      if (id) await activateTab(id);
    } catch (error) {
      errorEl.textContent = error.message || '保存失败';
    } finally {
      saveButton.disabled = false;
    }
  };

  saveButton.addEventListener('click', () => void submit());
  cancelEdit.addEventListener('click', resetForm);
  kindSelect.addEventListener('change', () => applyKind(kindSelect.value));
  authSelect.addEventListener('change', updateAuthFields);
  for (const field of [
    nameField,
    urlField,
    hostField,
    portField,
    userField,
    passwordField,
    keyField,
    passphraseField,
    rootField,
  ]) {
    field.input.addEventListener('keydown', (event) => {
      if (event.key === 'Enter') {
        event.preventDefault();
        void submit();
      }
    });
    field.input.addEventListener('input', () => {
      if (errorEl.textContent) errorEl.textContent = '';
    });
  }

  drawList();
  resetForm();
  setTimeout(() => nameField.input.focus(), 30);
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
  $('#btn-upload-folder').addEventListener('click', () => {
    // Safari / WKWebView older than 11.1 ignores `webkitdirectory` and would
    // open a regular file picker (or nothing at all). Tell the user instead of
    // failing silently.
    if (!('webkitdirectory' in document.createElement('input'))) {
      toast('当前浏览器不支持选择文件夹，请把文件夹直接拖到窗口中', 'error');
      return;
    }
    $('#dir-input').click();
  });
  $('#dir-input').addEventListener('change', (event) => {
    const files = Array.from(event.target.files || []);
    event.target.value = '';
    if (files.length) enqueueUploads(files, state.cwd);
  });
  $('#btn-mkdir').addEventListener('click', createFolder);
  $('#btn-refresh').addEventListener('click', () => loadDir(state.cwd));
  $('#btn-history').addEventListener('click', openHistoryDialog);
  $('#btn-add-server').addEventListener('click', openServersDialog);
  $('#btn-paste').addEventListener('click', () => void pasteClipboard());
  $('#toggle-hidden').checked = state.showHidden;
  $('#toggle-hidden').addEventListener('change', (event) => {
    state.showHidden = event.target.checked;
    try {
      localStorage.setItem(SHOW_HIDDEN_KEY, state.showHidden ? '1' : '0');
    } catch {
      /* storage can be unavailable in a hardened webview */
    }
    const tab = activeTab();
    if (tab.session) {
      void loadDir(tab.cwd, { tab, skipHistory: true });
    }
  });
  $('#server-overlay').addEventListener('click', (event) => {
    if (event.target.closest('[data-server-close]')) closeServerPanel();
  });
  document.addEventListener('keydown', (event) => {
    if (event.key !== 'Escape') return;
    const overlay = $('#server-overlay');
    if (overlay && !overlay.hidden) closeServerPanel();
  });
  document.addEventListener('click', (event) => {
    if (!event.target.closest || !event.target.closest('#context-menu')) {
      closeContextMenu();
    }
  });
  $('#context-menu').addEventListener('keydown', (event) => {
    if (event.key === 'Escape') {
      event.preventDefault();
      closeContextMenu();
    }
  });
  window.addEventListener('blur', closeContextMenu);
  window.addEventListener('resize', closeContextMenu);
  document.addEventListener('scroll', closeContextMenu, true);
  for (const selector of ['#file-list', '#empty-state']) {
    const area = $(selector);
    if (!area) continue;
    area.addEventListener('contextmenu', (event) => {
      if (event.target.closest('#file-list .file-row')) return;
      const tab = activeTab();
      if (!tab.session) return;
      event.preventDefault();
      openBackgroundContextMenu(event.clientX, event.clientY, tab);
    });
  }
  $('#root-select').addEventListener('change', async (event) => {
    const path = event.target.value;
    const tab = activeTab();
    if (!isPasswordless(tab.target)) return;
    try {
      const response = await fetch('/app/local/root', {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          ...targetHeaders(tab),
        },
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
      if (tab.target.kind === 'local') await refreshAppInfo();
      await refreshTargetRoots(tab);
      tab.cwd = '/';
      tab.entries = [];
      tab.list = emptyList();
      tab.loaded = false;
      tab.needsRefresh = true;
      await loadDir('/', { tab, skipHistory: true });
      renderRootSelect();
    } catch (error) {
      toast(error.message || '切换目录失败', 'error');
      renderRootSelect();
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
  $('#btn-check-update').addEventListener('click', () => {
    void checkForAppUpdate(true);
  });
  $('#btn-update-close').addEventListener('click', hideUpdatePanel);
  $('#btn-update-skip').addEventListener('click', () => {
    if (updateInfo && updateInfo.latest_version) {
      rememberSkippedUpdate(updateInfo.latest_version);
    }
    hideUpdatePanel();
  });
  $('#btn-update-download').addEventListener('click', () => {
    void openUpdateDownload();
  });
  $('#btn-update-silent').addEventListener('click', () => {
    void startSilentUpdate();
  });
  $('#btn-update-restart').addEventListener('click', () => {
    void restartAfterUpdate();
  });
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

/* ---------- Desktop app updates ---------- */

const UPDATE_SKIP_KEY = 'agentgod.skipped-update';
let updateCheckBusy = false;
let updatePollTimer = null;
let updateInfo = null;

function skippedUpdateVersion() {
  try {
    return localStorage.getItem(UPDATE_SKIP_KEY) || '';
  } catch {
    return '';
  }
}

function rememberSkippedUpdate(version) {
  try {
    localStorage.setItem(UPDATE_SKIP_KEY, version);
  } catch {
    /* storage can be unavailable in a hardened webview */
  }
}

function updatePlatformLabel(info) {
  const os =
    { macos: 'macOS', windows: 'Windows', linux: 'Linux' }[info.platform] ||
    info.platform ||
    '当前系统';
  const arch =
    { x86_64: 'x64', aarch64: 'ARM64' }[info.arch] || info.arch || '';
  return arch ? `${os} · ${arch}` : os;
}

async function updateRequest(path, options = {}) {
  const response = await fetch(path, { cache: 'no-store', ...options });
  let data = null;
  try {
    data = await response.json();
  } catch {
    data = null;
  }
  if (!response.ok) {
    throw new Error((data && data.error) || `请求失败 (${response.status})`);
  }
  return data || {};
}

async function checkForAppUpdate(force = false) {
  if (!state.app || !state.app.mode || updateCheckBusy) return;
  updateCheckBusy = true;
  const button = $('#btn-check-update');
  const label = $('#update-check-label');
  const spinner = $('#update-check-spinner');
  if (button) button.disabled = true;
  if (label) label.textContent = '检查中…';
  if (spinner) spinner.hidden = false;
  try {
    const info = await updateRequest(
      `/app/update/check${force ? '?force=1' : ''}`,
    );
    updateInfo = info;
    if (info.error) {
      if (force) toast(info.error, 'error');
      return;
    }
    if (!info.has_update || !info.latest_version) {
      if (force) toast(`已是最新版本 v${info.current_version}`, 'success');
      return;
    }
    if (!force && skippedUpdateVersion() === info.latest_version) return;
    showUpdatePanel(info);
  } catch (error) {
    // Startup checks must never block the file browser. Only manual checks
    // surface connectivity problems.
    if (force) toast(error.message || '检查更新失败', 'error');
  } finally {
    updateCheckBusy = false;
    if (button) button.disabled = false;
    if (label) label.textContent = '检查更新';
    if (spinner) spinner.hidden = true;
  }
}

function showUpdatePanel(info) {
  $('#update-title').textContent = `AgentGod Files v${info.latest_version} 已发布`;
  $('#update-summary').textContent = `当前 v${info.current_version} · ${updatePlatformLabel(info)}`;
  const unsupportedPlatform = info.platform && info.platform !== 'macos';
  $('#update-asset').textContent = info.asset
    ? `已匹配本机安装包：${info.asset}`
    : unsupportedPlatform
      ? '当前仅发布 macOS 安装包，Windows / Linux 版本暂不提供'
      : '未找到匹配当前系统的安装包，可在发布页手动选择';
  const notesWrap = $('#update-notes-wrap');
  notesWrap.hidden = !info.notes;
  $('#update-notes').textContent = info.notes || '';
  $('#btn-update-download').textContent = info.download_url
    ? '下载安装包'
    : '打开发布页';
  $('#btn-update-download').disabled = false;
  $('#btn-update-silent').hidden = !info.silent;
  $('#btn-update-silent').disabled = false;
  $('#btn-update-restart').hidden = true;
  $('#update-progress').hidden = true;
  $('#update-panel').hidden = false;
}

function stopUpdatePolling() {
  if (updatePollTimer) {
    clearInterval(updatePollTimer);
    updatePollTimer = null;
  }
}

function hideUpdatePanel() {
  $('#update-panel').hidden = true;
  stopUpdatePolling();
}

async function openUpdateDownload() {
  try {
    await updateRequest('/app/update/open', { method: 'POST' });
  } catch (error) {
    toast(error.message || '无法打开下载链接', 'error');
  }
}

async function startSilentUpdate() {
  try {
    await updateRequest('/app/update/install', { method: 'POST' });
  } catch (error) {
    toast(error.message || '无法启动应用内更新', 'error');
    return;
  }
  $('#btn-update-silent').disabled = true;
  $('#btn-update-download').disabled = true;
  $('#update-progress').hidden = false;
  $('#update-progress-bar').value = 0;
  $('#update-progress-text').textContent = '准备中…';
  stopUpdatePolling();
  updatePollTimer = setInterval(() => {
    void pollUpdateStatus();
  }, 800);
  void pollUpdateStatus();
}

async function pollUpdateStatus() {
  try {
    renderUpdateStatus(await updateRequest('/app/update/status'));
  } catch {
    // Windows exits the process while the installer takes over.
    stopUpdatePolling();
  }
}

function renderUpdateStatus(status) {
  $('#update-progress-bar').value = Number(status.percent) || 0;
  $('#update-progress-text').textContent = status.message || '';
  if (status.phase === 'done') {
    stopUpdatePolling();
    $('#btn-update-silent').hidden = true;
    $('#btn-update-restart').hidden = false;
    toast('更新已安装，重启应用后生效', 'success');
  } else if (status.phase === 'error') {
    stopUpdatePolling();
    $('#btn-update-silent').disabled = false;
    $('#btn-update-download').disabled = false;
    toast(status.message || '更新失败', 'error');
  }
}

async function restartAfterUpdate() {
  try {
    await updateRequest('/app/update/restart', { method: 'POST' });
  } catch (error) {
    toast(error.message || '重启失败，请手动重新打开应用', 'error');
  }
}

document.addEventListener('DOMContentLoaded', () => {
  bindEvents();
  document.addEventListener('keydown', handlePreviewKeys);
  document.addEventListener('keydown', handleFileShortcuts);
  void boot();
});
