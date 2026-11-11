#!/usr/bin/env node
/**
 * End-to-end protocol test for AgentGod Files.
 *
 * It speaks the exact same RSA-OAEP / AES-256-GCM protocol as the browser UI,
 * so a PASS means login, encrypted API calls, chunked upload, encrypted
 * streaming download, rename and delete all work against a live server.
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

async function fetchPublicKey() {  const response = await fetch(`${BASE_URL}/api/pubkey`, { cache: 'no-store' });
  if (!response.ok) throw new Error(`pubkey HTTP ${response.status}`);
  const data = await readJson(response);
  if (!data || !data.public_key) throw new Error('pubkey response missing key');
  return data.public_key;
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

async function login(username, password, expectFailure = false) {
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
  const ciphertext = await subtle.encrypt(
    { name: 'AES-GCM', iv, additionalData: encoder.encode('login') },
    key,
    encoder.encode(JSON.stringify({ username, password, ts: Date.now() })),
  );
  const wrapped = await subtle.encrypt({ name: 'RSA-OAEP' }, publicKey, rawKey);
  const response = await fetch(`${BASE_URL}/api/login`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      ek: b64(wrapped),
      iv: b64(iv),
      ct: b64(ciphertext),
    }),
  });
  const data = await readJson(response);
  if (expectFailure) {
    if (response.ok) throw new Error('wrong password was accepted');
    return { rejected: true, error: data?.error };
  }
  if (!response.ok) throw new Error(`login failed: ${data?.error || response.status}`);
  const result = await makeCipher(key).decrypt('login-resp', data);
  if (!result.token) throw new Error('login response missing token');
  return { ...result, key };
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

  async function api(path, payload = {}, method = 'POST') {
    const aad = `${method} ${path}`;
    const envelope = await cipher.encrypt(aad, payload);
    const response = await fetch(`${BASE_URL}${path}`, {
      method,
      headers: {
        'Content-Type': 'application/json',
        Authorization: `Bearer ${session.token}`,
      },
      body: JSON.stringify(envelope),
    });
    const data = await readJson(response);
    if (!response.ok) {
      throw new Error(`${method} ${path} failed: ${data?.error || response.status}`);
    }
    return cipher.decrypt(aad, data);
  }

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

  /* ---------- preview + HTTP Range ---------- */

  const previewToken = await api('/api/preview/token', {
    path: `/${nestedRoot}/docs/2026/note.txt`,
  });
  const previewFull = await fetch(`${BASE_URL}${previewToken.url}`);
  if (!previewFull.ok) throw new Error(`preview failed: ${previewFull.status}`);
  if (!previewFull.headers.get('accept-ranges')) throw new Error('preview missing Accept-Ranges');
  if (!String(previewFull.headers.get('content-type')).startsWith('text/plain')) {
    throw new Error('preview content type mismatch');
  }
  const previewText = await previewFull.text();
  if (previewText !== '嵌套目录上传测试\n') throw new Error('preview content mismatch');

  const rangeResponse = await fetch(`${BASE_URL}${previewToken.url}`, {
    headers: { Range: 'bytes=0-5' },
  });
  if (rangeResponse.status !== 206) throw new Error('range request did not return 206');
  if (!rangeResponse.headers.get('content-range')?.startsWith('bytes 0-5/')) {
    throw new Error('range response Content-Range mismatch');
  }
  const rangeBytes = new Uint8Array(await rangeResponse.arrayBuffer());
  // '嵌' and '套' are 3 UTF-8 bytes each, so bytes 0-5 decode to "嵌套".
  if (Buffer.from(rangeBytes).toString('utf8') !== '嵌套') {
    throw new Error('range response bytes mismatch');
  }
  record('preview token + HTTP range', '206 Partial Content honoured');

  const htmlBody = '<!doctype html><html><body><h1>预览测试</h1><script>document.title="x"</script></body></html>';
  const htmlBytes = encoder.encode(htmlBody);
  const htmlInit = await api('/api/upload/init', {
    dir: `/${nestedRoot}`,
    name: 'preview.html',
    size: htmlBytes.length,
  });
  await uploadChunkBlob(htmlInit.id, 0, htmlBytes);
  await api('/api/upload/finish', { id: htmlInit.id });
  const htmlToken = await api('/api/preview/token', { path: `/${nestedRoot}/preview.html` });
  const htmlResponse = await fetch(`${BASE_URL}${htmlToken.url}`);
  const htmlCsp = htmlResponse.headers.get('content-security-policy') || '';
  if (!htmlCsp.includes('sandbox')) {
    throw new Error('HTML preview is not sandboxed');
  }
  if ((await htmlResponse.text()) !== htmlBody) throw new Error('HTML preview body mismatch');
  record('sandboxed HTML preview', 'CSP sandbox enforced');

  const noTokenResponse = await fetch(`${BASE_URL}/preview/not-a-real-token`);
  if (noTokenResponse.status !== 404) throw new Error('unknown preview token was accepted');
  record('preview token rejection', '404 for unknown token');

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
