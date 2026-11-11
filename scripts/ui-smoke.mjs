#!/usr/bin/env node
/**
 * Browser smoke test over Chrome DevTools Protocol. Exercises the real web UI:
 * WebCrypto login, directory listing, create-folder dialog, upload queue and
 * delete, while collecting page exceptions.
 *
 * Usage:
 *   CHROME_DEBUG_URL=http://127.0.0.1:9223 UI_URL=https://127.0.0.1:18443/ \
 *   ADMIN_PASSWORD=... node scripts/ui-smoke.mjs
 */

const DEBUG_URL = (process.env.CHROME_DEBUG_URL || 'http://127.0.0.1:9223').replace(/\/+$/, '');
const PAGE_URL = process.env.UI_URL || 'https://127.0.0.1:18443/';
const ADMIN_USER = process.env.ADMIN_USER || 'admin';
const ADMIN_PASSWORD = process.env.ADMIN_PASSWORD || '';
const DOWNLOAD_DIR = process.env.DOWNLOAD_DIR || '/tmp/agentgod-ui-downloads';
const TEST_VIDEO = process.env.TEST_VIDEO_PATH || '/tmp/agentgod-ui-test.webm';

import { readdir, rm, stat } from 'node:fs/promises';
import { readFileSync } from 'node:fs';

if (!ADMIN_PASSWORD) {
  console.error('ADMIN_PASSWORD is required');
  process.exit(2);
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const CRC_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let index = 0; index < 256; index += 1) {
    let value = index;
    for (let bit = 0; bit < 8; bit += 1) {
      value = value & 1 ? 0xedb88320 ^ (value >>> 1) : value >>> 1;
    }
    table[index] = value >>> 0;
  }
  return table;
})();

function crc32(buffer) {
  let crc = 0xffffffff;
  for (const byte of buffer) {
    crc = CRC_TABLE[(crc ^ byte) & 0xff] ^ (crc >>> 8);
  }
  return (crc ^ 0xffffffff) >>> 0;
}

/** Builds a store-only (uncompressed) zip, which is a valid .docx container. */
function buildZip(files) {
  const localParts = [];
  const centralParts = [];
  let offset = 0;
  for (const file of files) {
    const name = Buffer.from(file.name, 'utf8');
    const data = Buffer.from(file.data, 'utf8');
    const crc = crc32(data);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4);
    local.writeUInt32LE(crc, 14);
    local.writeUInt32LE(data.length, 18);
    local.writeUInt32LE(data.length, 22);
    local.writeUInt16LE(name.length, 26);
    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x02014b50, 0);
    central.writeUInt16LE(20, 4);
    central.writeUInt16LE(20, 6);
    central.writeUInt32LE(crc, 16);
    central.writeUInt32LE(data.length, 20);
    central.writeUInt32LE(data.length, 24);
    central.writeUInt16LE(name.length, 28);
    central.writeUInt32LE(offset, 42);
    localParts.push(local, name, data);
    centralParts.push(central, name);
    offset += local.length + name.length + data.length;
  }
  const central = Buffer.concat(centralParts);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(files.length, 8);
  end.writeUInt16LE(files.length, 10);
  end.writeUInt32LE(central.length, 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...localParts, central, end]);
}

function buildDocx(paragraph) {
  const escaped = paragraph.replace(/&/g, '&amp;').replace(/</g, '&lt;');
  return buildZip([
    {
      name: '[Content_Types].xml',
      data: '<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>',
    },
    {
      name: '_rels/.rels',
      data: '<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>',
    },
    {
      name: 'word/document.xml',
      data: `<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>${escaped}</w:t></w:r></w:p></w:body></w:document>`,
    },
  ]);
}

async function createTarget() {
  const response = await fetch(
    `${DEBUG_URL}/json/new?${encodeURIComponent(PAGE_URL)}`,
    { method: 'PUT' },
  );
  if (!response.ok) throw new Error(`cannot create Chrome target: ${response.status}`);
  return response.json();
}

function connect(webSocketUrl) {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(webSocketUrl);
    const pending = new Map();
    const handlers = new Set();
    const errors = [];
    let nextId = 0;

    const send = (method, params = {}) =>
      new Promise((resolveCommand, rejectCommand) => {
        const id = ++nextId;
        pending.set(id, { resolve: resolveCommand, reject: rejectCommand });
        socket.send(JSON.stringify({ id, method, params }));
      });

    socket.addEventListener('open', () =>
      resolve({ socket, send, errors }),
    );
    socket.addEventListener('error', reject);
    socket.addEventListener('message', (event) => {
      const message = JSON.parse(event.data);
      if (message.id && pending.has(message.id)) {
        const { resolve: resolveCommand, reject: rejectCommand } = pending.get(message.id);
        pending.delete(message.id);
        if (message.error) rejectCommand(new Error(message.error.message));
        else resolveCommand(message.result);
        return;
      }
      if (message.method === 'Runtime.exceptionThrown') {
        const detail = message.params.exceptionDetails;
        errors.push(detail.exception?.description || detail.text || 'page exception');
      }
      if (message.method === 'Log.entryAdded' && message.params.entry.level === 'error') {
        errors.push(`console: ${message.params.entry.text}`);
      }
      for (const handler of handlers) handler(message);
    });
  });
}

async function main() {
  const target = await createTarget();
  const { socket, send, errors } = await connect(target.webSocketDebuggerUrl);

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

  const waitFor = async (expression, timeoutMs = 15_000, label = expression) => {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      if (await evaluate(expression)) return;
      await sleep(200);
    }
    throw new Error(`timeout waiting for: ${label}`);
  };

  await send('Page.navigate', { url: PAGE_URL });
  await waitFor(
    "document.readyState === 'complete' && !!document.querySelector('#login-form')",
    15_000,
    'login page',
  );

  const secure = await evaluate(
    "typeof crypto.subtle === 'object' && typeof crypto.getRandomValues === 'function'",
  );
  if (!secure) throw new Error('crypto.subtle is unavailable — page is not a secure context');
  console.log('PASS  page loaded as secure context');

  const cspErrors = errors.filter((message) => /Content Security Policy/i.test(message));
  if (cspErrors.length) throw new Error(`CSP violations: ${cspErrors.join(' | ')}`);
  console.log('PASS  no CSP violations');

  await evaluate(`(() => {
    document.querySelector('#username').value = ${JSON.stringify(ADMIN_USER)};
    document.querySelector('#password').value = ${JSON.stringify(ADMIN_PASSWORD)};
    document.querySelector('#login-btn').click();
    return true;
  })()`);

  await waitFor(
    "!document.querySelector('#app-view').hidden && document.querySelector('#login-view').hidden",
    15_000,
    'login to finish',
  );
  const badge = await evaluate("document.querySelector('#user-badge').textContent");
  if (!badge.includes(ADMIN_USER)) throw new Error(`unexpected user badge: ${badge}`);
  console.log(`PASS  UI login and directory load (user=${badge})`);

  const folder = `ui-smoke-${Date.now()}`;
  await evaluate("document.querySelector('#btn-mkdir').click()");
  await waitFor("!!document.querySelector('#modal[open] input')", 5_000, 'new-folder dialog');
  await evaluate(`(() => {
    const input = document.querySelector('#modal input');
    input.value = ${JSON.stringify(folder)};
    const buttons = [...document.querySelectorAll('#modal .modal-actions button')];
    buttons[buttons.length - 1].click();
    return true;
  })()`);
  await waitFor(
    `[...document.querySelectorAll('.file-row .name-btn')].some((node) => node.textContent === ${JSON.stringify(folder)})`,
    10_000,
    'folder row to appear',
  );
  console.log('PASS  create-folder dialog');

  await evaluate(`(() => {
    const data = new Uint8Array(1024 * 1024);
    for (let offset = 0; offset < data.length; offset += 65536) {
      crypto.getRandomValues(data.subarray(offset, Math.min(offset + 65536, data.length)));
    }
    const transfer = new DataTransfer();
    transfer.items.add(new File([data], 'ui-test.bin', { type: 'application/octet-stream' }));
    enqueueUploads(transfer.files, '/');
    return true;
  })()`);
  await waitFor(
    "[...document.querySelectorAll('.upload-item.done')].some((node) => node.textContent.includes('ui-test.bin'))",
    30_000,
    'UI upload to complete',
  );
  await waitFor(
    "[...document.querySelectorAll('.file-row .file-name')].some((node) => node.textContent === 'ui-test.bin')",
    10_000,
    'uploaded file row',
  );
  console.log('PASS  drag-and-drop upload path (queue + encryption)');

  await rm(DOWNLOAD_DIR, { recursive: true, force: true });
  await send('Browser.setDownloadBehavior', {
    behavior: 'allow',
    downloadPath: DOWNLOAD_DIR,
    eventsEnabled: true,
  });
  const pickerAvailable = await evaluate("typeof window.showSaveFilePicker === 'function'");
  if (pickerAvailable) {
    await evaluate("window.showSaveFilePicker = undefined; true");
  }
  await evaluate("startDownload('/ui-test.bin', 'ui-test.bin').then(() => true)");
  let downloadedFile = null;
  for (let attempt = 0; attempt < 100; attempt += 1) {
    try {
      const names = await readdir(DOWNLOAD_DIR);
      const candidate = names.find((name) => name === 'ui-test.bin');
      if (candidate) {
        const info = await stat(`${DOWNLOAD_DIR}/${candidate}`);
        if (info.size === 1024 * 1024) {
          downloadedFile = `${DOWNLOAD_DIR}/${candidate}`;
          break;
        }
      }
    } catch {
      /* download directory may not exist yet */
    }
    await sleep(200);
  }
  if (!downloadedFile) throw new Error('downloaded file was not written or has wrong size');
  const successToast = await evaluate(
    "[...document.querySelectorAll('.toast.success')].some((node) => node.textContent.includes('下载完成'))",
  );
  if (!successToast) throw new Error('download completion toast missing');
  console.log('PASS  browser streaming decrypt + download (1 MiB)');

  /* ---------- folder upload (nested relative paths) ---------- */

  await evaluate(`(() => {
    const makeFile = (content, name) => new File([content], name, { type: 'text/plain' });
    enqueueUploads(
      [
        { file: makeFile('嵌套内容', 'deep.txt'), relative: 'ui-folder/sub/deep.txt' },
        { file: makeFile('顶层内容', 'top.txt'), relative: 'ui-folder/top.txt' },
      ],
      '/',
      ['ui-folder', 'ui-folder/sub'],
    );
    return true;
  })()`);
  await waitFor(
    "[...document.querySelectorAll('.upload-item.done')].filter((node) => node.textContent.includes('ui-folder/')).length >= 2",
    45_000,
    'folder upload to complete',
  );
  const nestedCheck = await evaluate(`(async () => {
    const listing = await api('/api/list', { path: '/ui-folder/sub' });
    const entry = listing.entries.find((item) => item.name === 'deep.txt');
    return entry ? entry.size : -1;
  })()`);
  if (nestedCheck <= 0) throw new Error('folder upload did not create nested structure');
  console.log('PASS  folder upload with nested directories');

  /* ---------- preview: text + image ---------- */

  await evaluate(`(() => {
    const note = new File(['你好，这是预览测试。'], 'ui-note.txt', { type: 'text/plain' });
    enqueueUploads([{ file: note, relative: 'ui-note.txt' }], '/');
    const png = Uint8Array.from(atob('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII='), (c) => c.charCodeAt(0));
    const image = new File([png], 'ui-pixel.png', { type: 'image/png' });
    enqueueUploads([{ file: image, relative: 'ui-pixel.png' }], '/');
    return true;
  })()`);
  await waitFor(
    "[...document.querySelectorAll('.file-row .file-name')].some((node) => node.textContent === 'ui-note.txt') && [...document.querySelectorAll('.file-row .file-name')].some((node) => node.textContent === 'ui-pixel.png')",
    30_000,
    'preview fixtures uploaded',
  );

  await evaluate("openPreview({ name: 'ui-note.txt', size: 30, modified: Date.now(), is_dir: false }); true");
  await waitFor(
    "!!document.querySelector('.preview-text') && document.querySelector('.preview-text').textContent.includes('预览测试')",
    15_000,
    'text preview content',
  );
  await evaluate("document.querySelector('#modal').close(); true");
  console.log('PASS  text file preview');

  await evaluate("openPreview({ name: 'ui-pixel.png', size: 68, modified: Date.now(), is_dir: false }); true");
  await waitFor(
    "(() => { const image = document.querySelector('.preview-image'); return !!image && image.complete && image.naturalWidth > 0; })()",
    15_000,
    'image preview to render',
  );
  await evaluate("document.querySelector('#modal').close(); true");
  console.log('PASS  image preview');

  /* ---------- image navigation with arrow keys ---------- */

  const pixelPng =
    'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=';
  await evaluate(`(() => {
    const makeImage = (name) => {
      const bytes = Uint8Array.from(atob(${JSON.stringify(pixelPng)}), (c) => c.charCodeAt(0));
      return new File([bytes], name, { type: 'image/png' });
    };
    enqueueUploads(
      [
        { file: makeImage('ui-photo-a.png'), relative: 'ui-photo-a.png' },
        { file: makeImage('ui-photo-b.png'), relative: 'ui-photo-b.png' },
      ],
      '/',
    );
    return true;
  })()`);
  await waitFor(
    "[...document.querySelectorAll('.file-row .file-name')].some((node) => node.textContent === 'ui-photo-b.png')",
    30_000,
    'image fixtures uploaded',
  );

  const dispatchKey = async (key, code, virtualKeyCode) => {
    await send('Input.dispatchKeyEvent', {
      type: 'keyDown',
      key,
      code,
      windowsVirtualKeyCode: virtualKeyCode,
      nativeVirtualKeyCode: virtualKeyCode,
    });
    await send('Input.dispatchKeyEvent', {
      type: 'keyUp',
      key,
      code,
      windowsVirtualKeyCode: virtualKeyCode,
      nativeVirtualKeyCode: virtualKeyCode,
    });
  };

  await evaluate("openPreview({ name: 'ui-photo-a.png', size: 68, modified: Date.now(), is_dir: false }); true");
  await waitFor(
    "(() => { const image = document.querySelector('.preview-image'); return document.querySelector('.preview-title strong').textContent === 'ui-photo-a.png' && !!image && image.naturalWidth > 0 && document.querySelector('.preview-hint').textContent.includes('切换图片'); })()",
    15_000,
    'first image preview',
  );
  await dispatchKey('ArrowRight', 'ArrowRight', 39);
  await waitFor(
    "document.querySelector('.preview-title strong').textContent === 'ui-photo-b.png' && (() => { const image = document.querySelector('.preview-image'); return !!image && image.naturalWidth > 0; })()",
    15_000,
    'next image after ArrowRight',
  );
  await dispatchKey('ArrowLeft', 'ArrowLeft', 37);
  await waitFor(
    "document.querySelector('.preview-title strong').textContent === 'ui-photo-a.png'",
    15_000,
    'previous image after ArrowLeft',
  );
  await evaluate("document.querySelector('#modal').close(); true");
  console.log('PASS  arrow keys switch photos');

  /* ---------- video keyboard shortcuts ---------- */

  let videoBase64;
  try {
    videoBase64 = readFileSync(TEST_VIDEO).toString('base64');
  } catch {
    throw new Error(
      `missing test video ${TEST_VIDEO}; run scripts/make-test-video.sh (needs ffmpeg)`,
    );
  }
  await evaluate(`(() => {
    const bytes = Uint8Array.from(atob(${JSON.stringify(videoBase64)}), (c) => c.charCodeAt(0));
    enqueueUploads(
      [{ file: new File([bytes], 'ui-video.webm', { type: 'video/webm' }), relative: 'ui-video.webm' }],
      '/',
    );
    return true;
  })()`);
  await waitFor(
    "[...document.querySelectorAll('.file-row .file-name')].some((node) => node.textContent === 'ui-video.webm')",
    45_000,
    'video fixture uploaded',
  );
  await evaluate("openPreview({ name: 'ui-video.webm', size: 131072, modified: Date.now(), is_dir: false }); true");
  await waitFor(
    "(() => { const video = document.querySelector('.preview-video'); return !!video && video.readyState >= 1 && Number.isFinite(video.duration) && video.duration > 5; })()",
    25_000,
    'video metadata',
  );
  const hintText = await evaluate("document.querySelector('.preview-hint').textContent");
  if (!hintText.includes('5 秒') || !hintText.includes('空格')) {
    throw new Error(`video shortcut hint missing: ${hintText}`);
  }

  await evaluate("(() => { const video = document.querySelector('.preview-video'); video.pause(); video.currentTime = 0; return true; })()");
  await sleep(400);
  await dispatchKey('ArrowRight', 'ArrowRight', 39);
  await waitFor(
    "document.querySelector('.preview-video').currentTime >= 4.5",
    5_000,
    'ArrowRight seeks forward 5s',
  );
  await dispatchKey('ArrowLeft', 'ArrowLeft', 37);
  await waitFor(
    "document.querySelector('.preview-video').currentTime <= 0.5",
    5_000,
    'ArrowLeft seeks back 5s',
  );
  const pausedBefore = await evaluate("document.querySelector('.preview-video').paused");
  await dispatchKey(' ', 'Space', 32);
  await sleep(700);
  const pausedAfter = await evaluate("document.querySelector('.preview-video').paused");
  if (pausedBefore === pausedAfter) throw new Error('Space did not toggle playback');
  await evaluate("(() => { const video = document.querySelector('.preview-video'); video.pause(); return true; })()");
  await evaluate("document.querySelector('#modal').close(); true");
  console.log('PASS  video arrow-key seek and Space play/pause');

  /* ---------- preview: Word (.docx) and Excel (.xlsx) ---------- */

  const docxBase64 = buildDocx('AgentGod Word 预览测试').toString('base64');
  await evaluate(`(() => {
    const bytes = Uint8Array.from(atob(${JSON.stringify(docxBase64)}), (c) => c.charCodeAt(0));
    enqueueUploads(
      [{ file: new File([bytes], 'ui-doc.docx'), relative: 'ui-doc.docx' }],
      '/',
    );
    return true;
  })()`);
  await waitFor(
    "[...document.querySelectorAll('.file-row .file-name')].some((node) => node.textContent === 'ui-doc.docx')",
    30_000,
    'docx upload',
  );
  await evaluate("openPreview({ name: 'ui-doc.docx', size: 2048, modified: Date.now(), is_dir: false }); true");
  await waitFor(
    "!!document.querySelector('.preview-docx') && document.querySelector('.preview-docx').textContent.includes('AgentGod Word 预览测试')",
    30_000,
    'docx preview content',
  );
  await evaluate("document.querySelector('#modal').close(); true");
  console.log('PASS  Word (.docx) preview via local parser');

  const sheetCreated = await evaluate(`(async () => {
    await loadScript('/vendor/xlsx.js');
    const workbook = XLSX.utils.book_new();
    const sheet = XLSX.utils.aoa_to_sheet([
      ['名称', '数量'],
      ['alpha', 3],
      ['beta', 7],
    ]);
    XLSX.utils.book_append_sheet(workbook, sheet, '数据');
    const output = XLSX.write(workbook, { bookType: 'xlsx', type: 'array' });
    enqueueUploads(
      [{ file: new File([output], 'ui-sheet.xlsx'), relative: 'ui-sheet.xlsx' }],
      '/',
    );
    return output.byteLength;
  })()`);
  if (!sheetCreated) throw new Error('failed to build xlsx fixture');
  await waitFor(
    "[...document.querySelectorAll('.file-row .file-name')].some((node) => node.textContent === 'ui-sheet.xlsx')",
    30_000,
    'xlsx upload',
  );
  await evaluate("openPreview({ name: 'ui-sheet.xlsx', size: 4096, modified: Date.now(), is_dir: false }); true");
  await waitFor(
    "(() => { const table = document.querySelector('.sheet-table'); return !!table && table.textContent.includes('alpha') && table.textContent.includes('beta'); })()",
    30_000,
    'xlsx preview table',
  );
  await evaluate("document.querySelector('#modal').close(); true");
  console.log('PASS  Excel (.xlsx) preview via local parser');

  /* ---------- folder download (tar through the encrypted stream) ---------- */

  await rm(`${DOWNLOAD_DIR}/ui-folder.tar`, { force: true });
  await evaluate("startFolderDownload('/ui-folder', 'ui-folder').then(() => true)");
  let folderArchive = 0;
  for (let attempt = 0; attempt < 150; attempt += 1) {
    try {
      const info = await stat(`${DOWNLOAD_DIR}/ui-folder.tar`);
      if (info.size > 512) {
        folderArchive = info.size;
        break;
      }
    } catch {
      /* not written yet */
    }
    await sleep(200);
  }
  if (!folderArchive) throw new Error('folder tar download did not complete');
  console.log(`PASS  folder download as tar (${folderArchive} bytes)`);

  /* ---------- account settings ---------- */

  const changedUser = `${ADMIN_USER}_ui`;
  const changedPassword = 'ui-test-password-123';
  try {
    await evaluate("document.querySelector('#btn-account').click(); true");
    await waitFor("document.querySelectorAll('#modal input').length === 4", 10_000, 'account dialog');
    await evaluate(`(() => {
      const inputs = [...document.querySelectorAll('#modal input')];
      inputs[0].value = ${JSON.stringify(ADMIN_PASSWORD)};
      inputs[1].value = ${JSON.stringify(changedUser)};
      inputs[2].value = ${JSON.stringify(changedPassword)};
      inputs[3].value = ${JSON.stringify(changedPassword)};
      [...document.querySelectorAll('#modal .modal-actions button')].pop().click();
      return true;
    })()`);
    await waitFor(
      `document.querySelector('#user-badge').textContent === ${JSON.stringify(changedUser)}`,
      20_000,
      'username badge after update',
    );
    console.log('PASS  account dialog changes username and password');
  } finally {
    const restored = await evaluate(`(async () => {
      const attempts = [
        { current_password: ${JSON.stringify(changedPassword)}, new_username: ${JSON.stringify(ADMIN_USER)}, new_password: ${JSON.stringify(ADMIN_PASSWORD)} },
        { current_password: ${JSON.stringify(ADMIN_PASSWORD)}, new_username: ${JSON.stringify(ADMIN_USER)}, new_password: '' },
      ];
      for (const payload of attempts) {
        try {
          await api('/api/account/update', payload);
          state.user = ${JSON.stringify(ADMIN_USER)};
          document.querySelector('#user-badge').textContent = ${JSON.stringify(ADMIN_USER)};
          return true;
        } catch {
          /* try the next credential pair */
        }
      }
      return false;
    })()`);
    if (!restored) throw new Error('failed to restore original credentials');
  }

  const cleanup = await evaluate(`Promise.all([
    api('/api/delete', { path: '/ui-test.bin' }).catch(() => null),
    api('/api/delete', { path: '/ui-note.txt' }).catch(() => null),
    api('/api/delete', { path: '/ui-pixel.png' }).catch(() => null),
    api('/api/delete', { path: '/ui-photo-a.png' }).catch(() => null),
    api('/api/delete', { path: '/ui-photo-b.png' }).catch(() => null),
    api('/api/delete', { path: '/ui-video.webm' }).catch(() => null),
    api('/api/delete', { path: '/ui-doc.docx' }).catch(() => null),
    api('/api/delete', { path: '/ui-sheet.xlsx' }).catch(() => null),
    api('/api/delete', { path: '/ui-folder' }).catch(() => null),
    api('/api/delete', { path: ${JSON.stringify(`/${folder}`)} }).catch(() => null)
  ]).then(() => true)`);
  if (!cleanup) throw new Error('cleanup failed');
  await rm(DOWNLOAD_DIR, { recursive: true, force: true });
  console.log('PASS  UI cleanup');

  const relevantErrors = errors.filter(
    (message) => !/favicon/i.test(message) && !/Failed to load resource/i.test(message),
  );
  if (relevantErrors.length) {
    throw new Error(`page errors: ${relevantErrors.join(' | ')}`);
  }
  console.log('PASS  no JS exceptions');

  socket.close();
  await fetch(`${DEBUG_URL}/json/close/${target.id}`).catch(() => {});

  console.log('\nUI SMOKE TEST PASSED');
}

main().catch((error) => {
  console.error(`\nUI SMOKE TEST FAILED: ${error.message}`);
  process.exit(1);
});
