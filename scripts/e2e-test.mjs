#!/usr/bin/env node
/**
 * End-to-end protocol test for AgentGod Files.
 *
 * It speaks the exact same RSA-OAEP / AES-256-GCM protocol as the browser UI,
 * so a PASS means login, encrypted API calls, chunked upload, encrypted
 * download/preview streams and the file operations all work against a live
 * server. Transfer tests also assert that plaintext never appears on the wire.
 *
 * Usage:
 *   BASE_URL=https://127.0.0.1:18443 ADMIN_PASSWORD=... \
 *   NODE_TLS_REJECT_UNAUTHORIZED=0 node scripts/e2e-test.mjs
 */

import { createHash, webcrypto } from 'node:crypto';

const subtle = webcrypto.subtle;
const encoder = new TextEncoder();
const decoder = new TextDecoder();

const BASE_URL = (process.env.BASE_URL || '').replace(/\/+$/, '');
const ADMIN_USER = process.env.ADMIN_USER || 'admin';
const ADMIN_PASSWORD = process.env.ADMIN_PASSWORD || '';
const SIZE_MB = Number(process.env.TEST_SIZE_MB || '6');
const CHUNK = 2 * 1024 * 1024;

if (!BASE_URL || !ADMIN_PASSWORD) {
  console.error('BASE_URL and ADMIN_PASSWORD are required');
  process.exit(2);
}

function b64(bytes) {
  return Buffer.from(bytes).toString('base64');
}

function unb64(text) {
  return new Uint8Array(Buffer.from(text, 'base64'));
}

function randomBytes(length) {
  const bytes = new Uint8Array(length);
  webcrypto.getRandomValues(bytes);
  return bytes;
}

function pemToDer(pem) {
  const body = pem
    .replace(/-----BEGIN [^-]+-----/g, '')
    .replace(/-----END [^-]+-----/g, '')
    .replace(/\s+/g, '');
  return unb64(body);
}

function sha256(bytes) {
  return createHash('sha256').update(Buffer.from(bytes)).digest('hex');
}

function readCString(buffer) {
  const zero = buffer.indexOf(0);
  return Buffer.from(zero === -1 ? buffer : buffer.subarray(0, zero)).toString('utf8');
}

/** Minimal tar reader: good enough for verifying the server's archive output. */
function parseTar(buffer) {
  const entries = new Map();
  let offset = 0;
  let pendingName = null;
  while (offset + 512 <= buffer.length) {
    const header = buffer.subarray(offset, offset + 512);
    if (header.every((byte) => byte === 0)) break;
    const rawName = readCString(header.subarray(0, 100));
    const prefix = readCString(header.subarray(345, 500));
    const size = parseInt(readCString(header.subarray(124, 136)).trim() || '0', 8);
    const type = String.fromCharCode(header[156] || 0x30);
    let name = prefix ? `${prefix}/${rawName}` : rawName;
    offset += 512;
    const dataEnd = offset + size;
    if (type === 'L') {
      pendingName = readCString(buffer.subarray(offset, dataEnd));
    } else if (type !== 'x' && type !== 'g') {
      if (pendingName) {
        name = pendingName;
        pendingName = null;
      }
      entries.set(name, { type, size, data: buffer.subarray(offset, dataEnd) });
    }
    offset += Math.ceil(size / 512) * 512;
  }
  return entries;
}

async function readJson(response) {
  try {
    return await response.json();
  } catch {
    return null;
  }
}

let currentSession = null;

async function uploadChunkBlob(uploadId, sequence, plaintext) {
  const key = currentSession.key;
  const path = `/api/upload/chunk/${encodeURIComponent(uploadId)}/${sequence}`;
  const aad = `PUT ${path}`;
  const iv = randomBytes(12);
  const ciphertext = new Uint8Array(
    await subtle.encrypt(
      { name: 'AES-GCM', iv, additionalData: encoder.encode(aad) },
      key,
      plaintext,
    ),
  );
  const frame = Buffer.alloc(4 + 12 + ciphertext.length);
  frame.writeUInt32BE(ciphertext.length, 0);
  frame.set(iv, 4);
  frame.set(ciphertext, 16);
  if (plaintext.length && frame.includes(Buffer.from(plaintext))) {
    throw new Error('upload chunk leaked plaintext on the wire');
  }
  const response = await fetch(`${BASE_URL}${path}`, {
    method: 'PUT',
    headers: {
      'Content-Type': 'application/octet-stream',
      Authorization: `Bearer ${currentSession.token}`,
    },
    body: frame,
  });
  const data = await readJson(response);
  if (!response.ok) {
    throw new Error(`upload chunk failed: ${data?.error || response.status}`);
  }
  await makeCipher(key).decrypt(aad, data);
}

async function readEncryptedFrames(response, key, expectEndMarker = true) {
  const chunkSize = Number(response.headers.get('x-chunk-size') || '65536');
  const reader = response.body.getReader();
  let pending = Buffer.alloc(0);
  let sequence = 0;
  const parts = [];
  let ended = false;

  outer: while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    pending = Buffer.concat([pending, Buffer.from(value)]);
    while (pending.length >= 4) {
      const length = pending.readUInt32BE(0);
      if (length > chunkSize + 16 + 1024) throw new Error('bad download frame length');
      if (pending.length < 16 + length) break;
      const iv = pending.subarray(4, 16);
      const ciphertext = pending.subarray(16, 16 + length);
      let plaintext;
      try {
        plaintext = Buffer.from(
          await subtle.decrypt(
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
        throw new Error('download frame authentication failed');
      }
      sequence += 1;
      pending = pending.subarray(16 + length);
      if (expectEndMarker && plaintext.toString('utf8') === 'AGF_TAR_END_v1') {
        ended = true;
        break outer;
      }
      parts.push(plaintext);
    }
  }
  return { data: Buffer.concat(parts), ended };
}

async function readPreviewStream(path) {
  const endpoint = '/api/preview/stream';
  const aad = `POST ${endpoint}`;
  const cipher = makeCipher(currentSession.key);
  const envelope = await cipher.encrypt(aad, { path });
  const response = await fetch(`${BASE_URL}${endpoint}`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${currentSession.token}`,
    },
    body: JSON.stringify(envelope),
  });
  if (!response.ok) {
    const data = await readJson(response);
    throw new Error(`preview failed: ${data?.error || response.status}`);
  }
  if (String(response.headers.get('content-type')) !== 'application/octet-stream') {
    throw new Error('preview response is not an encrypted binary stream');
  }
  const streamId = response.headers.get('x-stream-id');
  if (!streamId) throw new Error('preview response missing x-stream-id');
  const chunkSize = Number(response.headers.get('x-chunk-size') || '65536');
  const raw = Buffer.from(await response.arrayBuffer());
  let offset = 0;
  let sequence = 0;
  const parts = [];
  while (offset < raw.length) {
    if (offset + 4 > raw.length) throw new Error('trailing preview frame header');
    const length = raw.readUInt32BE(offset);
    offset += 4;
    if (length > chunkSize + 16 + 1024) throw new Error('preview frame too large');
    if (offset + 12 + length > raw.length) throw new Error('truncated preview frame');
    const iv = raw.subarray(offset, offset + 12);
    offset += 12;
    const ciphertext = raw.subarray(offset, offset + length);
    offset += length;
    let plaintext;
    try {
      plaintext = Buffer.from(
        await subtle.decrypt(
          {
            name: 'AES-GCM',
            iv,
            additionalData: encoder.encode(`pv:${streamId}:${sequence}`),
          },
          currentSession.key,
          ciphertext,
        ),
      );
    } catch {
      throw new Error('preview frame authentication failed');
    }
    parts.push(plaintext);
    sequence += 1;
  }
  return {
    raw,
    data: Buffer.concat(parts),
    frames: sequence,
    contentType: String(response.headers.get('x-preview-content-type') || ''),
    size: Number(response.headers.get('x-file-size') || '0'),
  };
}

async function fetchPublicKey() {
  const response = await fetch(`${BASE_URL}/api/pubkey`, { cache: 'no-store' });
  if (!response.ok) throw new Error(`pubkey HTTP ${response.status}`);
  const data = await readJson(response);
  if (!data || !data.public_key) throw new Error('pubkey response missing key');
  return data.public_key;
}

async function fetchChallenge() {
  const response = await fetch(`${BASE_URL}/api/challenge`, { cache: 'no-store' });
  if (!response.ok) throw new Error(`challenge HTTP ${response.status}`);
  const data = await readJson(response);
  if (!data || !data.challenge) throw new Error('challenge response missing value');
  return data.challenge;
}

async function postLogin(body) {
  const response = await fetch(`${BASE_URL}/api/login`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body,
  });
  return { response, data: await readJson(response) };
}

function makeCipher(key) {
  return {
    async encrypt(aad, value) {
      const iv = randomBytes(12);
      const ciphertext = await subtle.encrypt(
        { name: 'AES-GCM', iv, additionalData: encoder.encode(aad) },
        key,
        encoder.encode(JSON.stringify(value)),
      );
      return { iv: b64(iv), ct: b64(ciphertext) };
    },
    async decrypt(aad, envelope) {
      const plaintext = await subtle.decrypt(
        {
          name: 'AES-GCM',
          iv: unb64(envelope.iv),
          additionalData: encoder.encode(aad),
        },
        key,
        unb64(envelope.ct),
      );
      return JSON.parse(decoder.decode(plaintext));
    },
  };
}

async function login(username, password, expectFailure = false, options = {}) {
  const publicPem = await fetchPublicKey();
  const publicKey = await subtle.importKey(
    'spki',
    pemToDer(publicPem),
    { name: 'RSA-OAEP', hash: 'SHA-256' },
    false,
    ['encrypt'],
  );
  const rawKey = randomBytes(32);
  const key = await subtle.importKey('raw', rawKey, 'AES-GCM', false, [
    'encrypt',
    'decrypt',
  ]);
  const iv = randomBytes(12);
  const challenge = options.skipChallenge
    ? ''
    : (options.challenge ?? await fetchChallenge());
  const payload = {
    username,
    password,
    ts: options.ts ?? Date.now(),
  };
  if (challenge) payload.challenge = challenge;
  const ciphertext = await subtle.encrypt(
    { name: 'AES-GCM', iv, additionalData: encoder.encode('login') },
    key,
    encoder.encode(JSON.stringify(payload)),
  );
  const wrapped = await subtle.encrypt({ name: 'RSA-OAEP' }, publicKey, rawKey);
  const body = JSON.stringify({
    ek: b64(wrapped),
    iv: b64(iv),
    ct: b64(ciphertext),
  });
  const { response, data } = await postLogin(body);
  if (expectFailure) {
    if (response.ok) throw new Error('login request was unexpectedly accepted');
    return { rejected: true, error: data?.error, body };
  }
  if (!response.ok) throw new Error(`login failed: ${data?.error || response.status}`);
  const result = await makeCipher(key).decrypt('login-resp', data);
  if (!result.token) throw new Error('login response missing token');
  return { ...result, key, body };
}

async function main() {
  const results = [];
  const record = (name, detail) => {
    results.push({ name, detail });
    console.log(`PASS  ${name}${detail ? ` — ${detail}` : ''}`);
  };

  const pub = await fetchPublicKey();
  if (!pub.includes('BEGIN PUBLIC KEY')) throw new Error('invalid public key PEM');
  record('public key', 'RSA-OAEP-SHA256 key served');

  const wrong = await login(ADMIN_USER, `${ADMIN_PASSWORD}-definitely-wrong`, true);
  record('wrong password rejected', wrong.error || 'HTTP error');

  const session = await login(ADMIN_USER, ADMIN_PASSWORD);
  currentSession = session;
  const cipher = makeCipher(session.key);
  record('encrypted login', `user=${session.user}`);

  async function postEncrypted(path, envelope, method = 'POST') {
    const response = await fetch(`${BASE_URL}${path}`, {
      method,
      headers: {
        'Content-Type': 'application/json',
        Authorization: `Bearer ${session.token}`,
      },
      body: JSON.stringify(envelope),
    });
    return { response, data: await readJson(response) };
  }

  async function api(path, payload = {}, method = 'POST') {
    const aad = `${method} ${path}`;
    const envelope = await cipher.encrypt(aad, payload);
    const { response, data } = await postEncrypted(path, envelope, method);
    if (!response.ok) {
      throw new Error(`${method} ${path} failed: ${data?.error || response.status}`);
    }
    return cipher.decrypt(aad, data);
  }

  /* ---------- replay / freshness hardening ---------- */
  const replayedLogin = await postLogin(session.body);
  if (replayedLogin.response.ok) {
    throw new Error('captured login envelope was accepted a second time');
  }
  record('login replay rejected', replayedLogin.data?.error || 'HTTP error');

  const stale = await login(ADMIN_USER, ADMIN_PASSWORD, true, {
    ts: Date.now() - 3_600_000,
  });
  record('stale login timestamp rejected', stale.error || 'HTTP error');

  const noChallenge = await login(ADMIN_USER, ADMIN_PASSWORD, true, {
    skipChallenge: true,
  });
  record('missing login challenge rejected', noChallenge.error || 'HTTP error');

  const listAad = 'POST /api/list';
  const listEnvelope = await cipher.encrypt(listAad, { path: '' });
  const acceptedList = await postEncrypted('/api/list', listEnvelope);
  if (!acceptedList.response.ok) throw new Error('initial list request failed');
  await cipher.decrypt(listAad, acceptedList.data);
  const duplicateList = await postEncrypted('/api/list', listEnvelope);
  if (duplicateList.response.ok) {
    throw new Error('API envelope replay was accepted');
  }
  record('api replay rejected', duplicateList.data?.error || 'HTTP error');

  let traversalRejected = false;
  try {
    await api('/api/list', { path: '../outside' });
  } catch (error) {
    traversalRejected = /路径|非法|越界/.test(String(error.message));
    if (!traversalRejected) throw error;
  }
  if (!traversalRejected) throw new Error('path traversal was accepted');
  record('path traversal rejected');

  const root = await api('/api/list', { path: '' });
  record('encrypted directory listing', `${root.entries.length} entries at ${root.path}`);

  const dirName = `e2e-${Date.now()}`;
  await api('/api/mkdir', { path: `/${dirName}` });
  record('create folder', `/${dirName}`);

  const size = SIZE_MB * 1024 * 1024;
  const original = new Uint8Array(size);
  for (let offset = 0; offset < size; offset += 65536) {
    webcrypto.getRandomValues(original.subarray(offset, Math.min(offset + 65536, size)));
  }
  const originalHash = sha256(original);
  const fileName = 'payload.bin';

  const init = await api('/api/upload/init', {
    dir: `/${dirName}`,
    name: fileName,
    size,
  });
  const uploadId = init.id;
  if (!uploadId) throw new Error('upload init missing id');

  let sequence = 0;
  for (let offset = 0; offset < size; offset += CHUNK) {
    const end = Math.min(offset + CHUNK, size);
    await uploadChunkBlob(uploadId, sequence, original.subarray(offset, end));
    sequence += 1;
  }
  const finished = await api('/api/upload/finish', { id: uploadId });
  if (finished.size !== size) throw new Error('uploaded size mismatch');
  record('chunked encrypted upload', `${SIZE_MB} MiB in ${sequence} chunks`);
  record('upload ciphertext on wire', 'chunk frames contain no plaintext');

  const listed = await api('/api/list', { path: `/${dirName}` });
  const uploaded = listed.entries.find((entry) => entry.name === fileName);
  if (!uploaded || uploaded.size !== size) throw new Error('uploaded file not listed');
  record('listed uploaded file', `${uploaded.name} (${uploaded.size} bytes)`);

  const downloadAad = 'POST /api/download';
  const downloadEnvelope = await cipher.encrypt(downloadAad, {
    path: `/${dirName}/${fileName}`,
  });
  const downloadResponse = await fetch(`${BASE_URL}/api/download`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${session.token}`,
    },
    body: JSON.stringify(downloadEnvelope),
  });
  if (!downloadResponse.ok) {
    const data = await readJson(downloadResponse);
    throw new Error(`download failed: ${data?.error || downloadResponse.status}`);
  }
  const expectedSize = Number(downloadResponse.headers.get('x-file-size'));
  const chunkSize = Number(downloadResponse.headers.get('x-chunk-size'));
  if (expectedSize !== size) throw new Error('download size header mismatch');

  const wireBytes = Buffer.from(await downloadResponse.clone().arrayBuffer());
  if (wireBytes.includes(Buffer.from(original.subarray(0, 32)))) {
    throw new Error('download leaked plaintext on the wire');
  }
  record('download ciphertext on wire', `${wireBytes.length} encrypted bytes`);

  const reader = downloadResponse.body.getReader();
  let pending = new Uint8Array(0);
  let received = 0;
  let chunkSequence = 0;
  const downloadedHash = createHash('sha256');
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    const combined = new Uint8Array(pending.length + value.length);
    combined.set(pending);
    combined.set(value, pending.length);
    pending = combined;
    while (pending.length >= 4) {
      const length = new DataView(
        pending.buffer,
        pending.byteOffset,
        4,
      ).getUint32(0, false);
      if (length > chunkSize + 16 + 1024) throw new Error('bad download frame length');
      if (pending.length < 16 + length) break;
      const iv = pending.subarray(4, 16);
      const ciphertext = pending.subarray(16, 16 + length);
      let plaintext;
      try {
        plaintext = new Uint8Array(
          await subtle.decrypt(
            {
              name: 'AES-GCM',
              iv,
              additionalData: encoder.encode(`dl:${chunkSequence}`),
            },
            session.key,
            ciphertext,
          ),
        );
      } catch {
        throw new Error('download chunk authentication failed');
      }
      downloadedHash.update(Buffer.from(plaintext));
      received += plaintext.length;
      chunkSequence += 1;
      pending = pending.subarray(16 + length);
    }
  }
  if (pending.length !== 0) throw new Error('trailing download bytes');
  if (received !== size) throw new Error('downloaded size mismatch');
  if (downloadedHash.digest('hex') !== originalHash) {
    throw new Error('downloaded content hash mismatch');
  }
  record(
    'encrypted streaming download',
    `${received} bytes, SHA-256 identical`,
  );

  const renamed = await api('/api/rename', {
    path: `/${dirName}/${fileName}`,
    new_name: 'renamed.bin',
  });
  if (!renamed.path.endsWith('/renamed.bin')) throw new Error('rename path mismatch');
  record('rename', renamed.path);

  // Empty file support.
  const emptyInit = await api('/api/upload/init', {
    dir: `/${dirName}`,
    name: 'empty.txt',
    size: 0,
  });
  await api('/api/upload/finish', { id: emptyInit.id });
  const emptyDownloadEnvelope = await cipher.encrypt('POST /api/download', {
    path: `/${dirName}/empty.txt`,
  });
  const emptyResponse = await fetch(`${BASE_URL}/api/download`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${session.token}`,
    },
    body: JSON.stringify(emptyDownloadEnvelope),
  });
  if (!emptyResponse.ok) throw new Error('empty file download failed');
  const emptyBytes = new Uint8Array(await emptyResponse.arrayBuffer());
  if (emptyBytes.length !== 0) throw new Error('empty download was not empty');
  record('empty file round-trip');

  await api('/api/delete', { path: `/${dirName}` });
  const after = await api('/api/list', { path: '' });
  if (after.entries.some((entry) => entry.name === dirName)) {
    throw new Error('test directory still present after delete');
  }
  record('recursive delete', `/${dirName} removed`);

  /* ---------- nested directory creation + folder upload path ---------- */

  const nestedRoot = `e2e-tree-${Date.now()}`;
  await api('/api/ensure_dirs', {
    paths: [
      `/${nestedRoot}/docs/2026`,
      `/${nestedRoot}/media`,
      `/${nestedRoot}/empty-folder`,
    ],
  });
  const nestedList = await api('/api/list', { path: `/${nestedRoot}/docs` });
  if (!nestedList.entries.some((entry) => entry.name === '2026' && entry.is_dir)) {
    throw new Error('ensure_dirs did not create nested directory');
  }
  record('create nested folders', `${nestedRoot}/docs/2026`);

  const nestedText = encoder.encode('嵌套目录上传测试\n');
  const nestedInit = await api('/api/upload/init', {
    dir: `/${nestedRoot}/docs/2026`,
    name: 'note.txt',
    size: nestedText.length,
  });
  await uploadChunkBlob(nestedInit.id, 0, nestedText);
  await api('/api/upload/finish', { id: nestedInit.id });
  record('upload into nested folder', `${nestedRoot}/docs/2026/note.txt`);

  /* ---------- paginated directory listing ---------- */

  const pageOne = await api('/api/list', {
    path: `/${nestedRoot}`,
    offset: 0,
    limit: 2,
  });
  if (!Array.isArray(pageOne.entries) || pageOne.entries.length !== 2) {
    throw new Error('first page did not honour limit=2');
  }
  if (pageOne.total < 3 || !pageOne.has_more) {
    throw new Error('pagination metadata missing');
  }
  const pageTwo = await api('/api/list', {
    path: `/${nestedRoot}`,
    offset: 2,
    limit: 2,
  });
  if (pageTwo.entries.length < 1 || pageTwo.has_more) {
    throw new Error('second page returned wrong slice');
  }
  const fullList = await api('/api/list', {
    path: `/${nestedRoot}`,
    offset: 0,
    limit: 100,
  });
  if (fullList.entries.length !== fullList.total) {
    throw new Error('full listing length mismatch');
  }
  record('paginated listing', `${fullList.total} entries, pages verified`);

  /* ---------- folder download as encrypted tar ---------- */

  const folderAad = 'POST /api/download_folder';
  const folderEnvelope = await cipher.encrypt(folderAad, { path: `/${nestedRoot}` });
  const folderResponse = await fetch(`${BASE_URL}/api/download_folder`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${session.token}`,
    },
    body: JSON.stringify(folderEnvelope),
  });
  if (!folderResponse.ok) {
    const data = await readJson(folderResponse);
    throw new Error(`folder download failed: ${data?.error || folderResponse.status}`);
  }
  const folderFrames = await readEncryptedFrames(folderResponse, session.key);
  if (!folderFrames.ended) throw new Error('folder archive end marker missing');
  const tarEntries = parseTar(folderFrames.data);
  const names = [...tarEntries.keys()];
  const prefix = `${nestedRoot}/`;
  if (!names.some((name) => name === nestedRoot || name === prefix)) {
    throw new Error('tar missing archive root');
  }
  const tarNote =
    tarEntries.get(`${prefix}docs/2026/note.txt`) ||
    tarEntries.get(`docs/2026/note.txt`);
  if (!tarNote || Buffer.from(tarNote.data).toString('utf8') !== '嵌套目录上传测试\n') {
    throw new Error('tar missing nested file content');
  }
  if (!names.some((name) => name.replace(/\/$/, '') === `${prefix}empty-folder`)) {
    throw new Error('tar missing empty directory entry');
  }
  record(
    'encrypted folder download (tar)',
    `${tarEntries.size} entries, end marker verified`,
  );

  /* ---------- encrypted preview stream ---------- */

  const notePreview = await readPreviewStream(`/${nestedRoot}/docs/2026/note.txt`);
  if (notePreview.raw.includes(nestedText)) {
    throw new Error('preview leaked plaintext on the wire');
  }
  if (notePreview.data.toString('utf8') !== '嵌套目录上传测试\n') {
    throw new Error('preview plaintext mismatch');
  }
  if (notePreview.contentType !== 'text/plain') {
    throw new Error(`preview content type mismatch: ${notePreview.contentType}`);
  }
  record(
    'encrypted text preview',
    `${notePreview.frames} AES-GCM frames, no plaintext on wire`,
  );

  const bigSize = 6 * 1024 * 1024;
  const bigBytes = new Uint8Array(bigSize);
  for (let index = 0; index < bigSize; index += 1) {
    bigBytes[index] = index & 0xff;
  }
  const bigInit = await api('/api/upload/init', {
    dir: `/${nestedRoot}`,
    name: 'big-range.bin',
    size: bigSize,
  });
  for (let offset = 0, sequence = 0; offset < bigSize; offset += CHUNK, sequence += 1) {
    await uploadChunkBlob(
      bigInit.id,
      sequence,
      bigBytes.subarray(offset, Math.min(offset + CHUNK, bigSize)),
    );
  }
  await api('/api/upload/finish', { id: bigInit.id });

  const bigPreview = await readPreviewStream(`/${nestedRoot}/big-range.bin`);
  if (bigPreview.size !== bigSize || bigPreview.data.length !== bigSize) {
    throw new Error('large encrypted preview size mismatch');
  }
  if (bigPreview.raw.includes(bigBytes.subarray(0, 64))) {
    throw new Error('large preview leaked plaintext on the wire');
  }
  if (sha256(bigPreview.data) !== sha256(bigBytes)) {
    throw new Error('large encrypted preview content mismatch');
  }
  record(
    'large encrypted preview',
    `${bigSize} bytes across ${bigPreview.frames} frames, SHA-256 identical`,
  );

  const htmlBody =
    '<!doctype html><html><body><h1>预览测试</h1><script>document.title="x"</script></body></html>';
  const htmlBytes = encoder.encode(htmlBody);
  const htmlInit = await api('/api/upload/init', {
    dir: `/${nestedRoot}`,
    name: 'preview.html',
    size: htmlBytes.length,
  });
  await uploadChunkBlob(htmlInit.id, 0, htmlBytes);
  await api('/api/upload/finish', { id: htmlInit.id });
  const htmlPreview = await readPreviewStream(`/${nestedRoot}/preview.html`);
  if (htmlPreview.contentType !== 'text/html') {
    throw new Error('HTML preview content type mismatch');
  }
  if (htmlPreview.data.toString('utf8') !== htmlBody) {
    throw new Error('HTML preview body mismatch');
  }
  if (htmlPreview.raw.includes(Buffer.from('<h1>预览测试</h1>'))) {
    throw new Error('HTML preview leaked plaintext on the wire');
  }
  record('encrypted HTML preview', 'ciphertext only, frontend injects sandbox CSP');

  const unauthenticated = await fetch(`${BASE_URL}/api/preview/stream`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: '{}',
  });
  if (unauthenticated.status !== 401) {
    throw new Error(`unauthenticated preview returned ${unauthenticated.status}`);
  }
  const retiredRawRoute = await fetch(`${BASE_URL}/preview/anything`);
  if (retiredRawRoute.status !== 404) {
    throw new Error('legacy plaintext preview route still exists');
  }
  record('preview access control', '401 without session, legacy raw route removed');

  /* ---------- account update ---------- */

  const secondSession = await login(ADMIN_USER, ADMIN_PASSWORD);
  const updatedUser = `${ADMIN_USER}_e2e`;
  const updatedPassword = `${ADMIN_PASSWORD}-rotated`;

  const updateResult = await api('/api/account/update', {
    current_password: ADMIN_PASSWORD,
    new_username: updatedUser,
    new_password: updatedPassword,
  });
  if (updateResult.user !== updatedUser) throw new Error('account update returned wrong user');
  if (!updateResult.password_changed) throw new Error('password change not reported');

  const oldLoginFailed = await login(ADMIN_USER, ADMIN_PASSWORD, true);
  if (!oldLoginFailed.rejected) throw new Error('old credentials still work after update');
  const newSessionCheck = await login(updatedUser, updatedPassword);
  if (!newSessionCheck.token) throw new Error('new credentials rejected');

  const secondListAad = 'POST /api/list';
  const secondEnvelope = await makeCipher(secondSession.key).encrypt(secondListAad, { path: '' });
  const secondListResponse = await fetch(`${BASE_URL}/api/list`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${secondSession.token}`,
    },
    body: JSON.stringify(secondEnvelope),
  });
  if (secondListResponse.status !== 401) {
    throw new Error('other sessions were not invalidated after credential change');
  }

  const restore = await api('/api/account/update', {
    current_password: updatedPassword,
    new_username: ADMIN_USER,
    new_password: ADMIN_PASSWORD,
  });
  if (restore.user !== ADMIN_USER) throw new Error('failed to restore original credentials');
  const restoredLogin = await login(ADMIN_USER, ADMIN_PASSWORD);
  if (!restoredLogin.token) throw new Error('restored credentials rejected');
  record(
    'account update',
    'username+password changed, other sessions revoked, originals restored',
  );

  await api('/api/delete', { path: `/${nestedRoot}` });
  record('cleanup nested tree', `/${nestedRoot} removed`);

  await api('/api/logout', {});
  record('logout');

  console.log(`\nALL ${results.length} CHECKS PASSED`);
}

main().catch((error) => {
  console.error(`\nFAIL: ${error.message}`);
  process.exit(1);
});
