#!/usr/bin/env node
/**
 * Range streaming smoke test.
 *
 * Verifies the ticket-gated, range-capable byte stream that media previews and
 * the new thumbnail URL rely on, and measures both transports on the same file
 * so a regression in throughput is visible rather than felt.
 *
 * Usage (local mode fixture server):
 *   BASE_URL=http://127.0.0.1:18090 node scripts/stream-smoke.mjs
 */

import { createHash, webcrypto } from 'node:crypto';
import { readFileSync } from 'node:fs';

const subtle = webcrypto.subtle;
const encoder = new TextEncoder();
const decoder = new TextDecoder();
const BASE_URL = (process.env.BASE_URL || '').replace(/\/+$/, '');
const FIXTURE = process.env.STREAM_FIXTURE || '';
/// When set, requests go through the desktop companion's proxy for that target,
/// which is how the app reaches a remote server (and how `__target` is used).
const TARGET = process.env.STREAM_TARGET || '';

function targetHeader() {
  return TARGET ? { 'X-RockBadger-Target': TARGET } : {};
}

function targetQuery(separator = '&') {
  return TARGET ? `${separator}__target=${encodeURIComponent(TARGET)}` : '';
}

if (!BASE_URL) {
  console.error('BASE_URL is required');
  process.exit(2);
}

let failures = 0;
function check(name, ok, detail = '') {
  if (ok) {
    console.log(`PASS ${name}${detail ? ` — ${detail}` : ''}`);
  } else {
    failures += 1;
    console.log(`FAIL ${name}${detail ? ` — ${detail}` : ''}`);
  }
}

const b64 = (bytes) => Buffer.from(bytes).toString('base64');
const unb64 = (text) => new Uint8Array(Buffer.from(text, 'base64'));
const randomBytes = (length) => {
  const bytes = new Uint8Array(length);
  webcrypto.getRandomValues(bytes);
  return bytes;
};

async function localSession() {
  const response = await fetch(`${BASE_URL}/api/local-login`, { headers: targetHeader() });
  if (!response.ok) throw new Error(`local login failed: ${response.status}`);
  const body = await response.json();
  const key = await subtle.importKey('raw', unb64(body.key), 'AES-GCM', false, [
    'encrypt',
    'decrypt',
  ]);
  return { token: body.token, key };
}

async function encrypt(session, aad, value) {
  const iv = randomBytes(12);
  const ciphertext = await subtle.encrypt(
    { name: 'AES-GCM', iv, additionalData: encoder.encode(aad) },
    session.key,
    encoder.encode(JSON.stringify(value)),
  );
  return { iv: b64(iv), ct: b64(ciphertext) };
}

async function decrypt(session, aad, envelope) {
  const plaintext = await subtle.decrypt(
    {
      name: 'AES-GCM',
      iv: unb64(envelope.iv),
      additionalData: encoder.encode(aad),
    },
    session.key,
    unb64(envelope.ct),
  );
  return JSON.parse(decoder.decode(plaintext));
}

async function api(session, path, payload, method = 'POST') {
  const aad = `${method} ${path}`;
  const envelope = await encrypt(session, aad, payload);
  const response = await fetch(`${BASE_URL}${path}`, {
    method,
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${session.token}`,
      ...targetHeader(),
    },
    body: JSON.stringify(envelope),
  });
  const body = await response.json().catch(() => null);
  if (!response.ok) throw new Error(`${path}: ${body?.error || response.status}`);
  return decrypt(session, aad, body);
}

function sha256(bytes) {
  return createHash('sha256').update(Buffer.from(bytes)).digest('hex');
}

async function timedDownload(url) {
  const started = process.hrtime.bigint();
  const response = await fetch(url);
  const bytes = new Uint8Array(await response.arrayBuffer());
  const seconds = Number(process.hrtime.bigint() - started) / 1e9;
  return { response, bytes, seconds };
}

const session = await localSession();
const listing = await api(session, '/api/list', { path: '' });
const media = listing.entries.find((entry) => /\.(bin|mp4)$/i.test(entry.name));

/// The fixture keeps its photo one level down so the directory ticket has a
/// real subtree to cover.
let imagePath = null;
let imageEntry = null;
for (const folder of ['', ...listing.entries.filter((e) => e.is_dir).map((e) => e.name)]) {
  const listingOf = folder ? await api(session, '/api/list', { path: folder }) : listing;
  const found = listingOf.entries.find((entry) => /\.(png|jpe?g)$/i.test(entry.name));
  if (found) {
    imageEntry = found;
    imagePath = folder ? `${folder}/${found.name}` : found.name;
    break;
  }
}
check('fixture image is listed', Boolean(imageEntry), imagePath || 'missing');

// --- directory ticket -------------------------------------------------------
const imageDir = imagePath && imagePath.includes('/') ? imagePath.slice(0, imagePath.lastIndexOf('/')) : '';
const dirTicket = await api(session, '/api/stream/ticket', { path: imageDir, dir: true });
check(
  'directory ticket issued',
  Boolean(dirTicket.ticket),
  `valid for ${Math.round((dirTicket.expires_at * 1000 - Date.now()) / 1000)}s`,
);

if (imageEntry) {
  const base = `/api/stream?path=${encodeURIComponent(imagePath)}&ticket=${encodeURIComponent(dirTicket.ticket)}${targetQuery()}`;
  const full = await fetch(`${BASE_URL}${base}`);
  const body = new Uint8Array(await full.arrayBuffer());
  check('directory ticket covers a child file', full.status === 200, `status ${full.status}`);
  check(
    'streamed bytes match the file on disk',
    !FIXTURE || sha256(body) === sha256(readFileSync(`${FIXTURE}/${imagePath}`)),
  );
  check(
    'range support is advertised',
    full.headers.get('accept-ranges') === 'bytes',
    full.headers.get('content-type') || '',
  );

  const ranged = await fetch(`${BASE_URL}${base}`, { headers: { Range: 'bytes=10-19' } });
  const rangeBody = new Uint8Array(await ranged.arrayBuffer());
  check('range request returns 206', ranged.status === 206, ranged.headers.get('content-range') || '');
  check(
    'range request returns exactly the requested slice',
    rangeBody.length === 10 && sha256(rangeBody) === sha256(body.subarray(10, 20)),
  );

  const unsatisfiable = await fetch(`${BASE_URL}${base}`, {
    headers: { Range: `bytes=${imageEntry.size + 5}-` },
  });
  check('out-of-range request returns 416', unsatisfiable.status === 416);

  const thumb = await fetch(
    `${BASE_URL}/api/thumb?path=${encodeURIComponent(imagePath)}&ticket=${encodeURIComponent(dirTicket.ticket)}&size=256${targetQuery()}`,
  );
  check(
    'thumbnail GET returns a JPEG',
    thumb.status === 200 && thumb.headers.get('content-type') === 'image/jpeg',
    `${(await thumb.arrayBuffer()).byteLength} bytes`,
  );

  const escaped = await fetch(
    `${BASE_URL}/api/stream?path=..%2Fetc%2Fpasswd&ticket=${encodeURIComponent(dirTicket.ticket)}${targetQuery()}`,
  );
  check('ticket refuses a path outside its folder', escaped.status === 403);

  // A file-scoped ticket must not be widened to its siblings.
  const fileTicket = await api(session, '/api/stream/ticket', { path: imagePath });
  const sibling = await fetch(
    `${BASE_URL}/api/stream?path=${encodeURIComponent(imagePath.replace(/\.\w+$/, '.sibling.jpg'))}&ticket=${encodeURIComponent(fileTicket.ticket)}${targetQuery()}`,
  );
  check('file ticket is bound to its own path', sibling.status === 403);

  const tampered = await fetch(
    `${BASE_URL}/api/stream?path=${encodeURIComponent(imagePath)}&ticket=${encodeURIComponent(`${dirTicket.ticket.slice(0, -2)}xy`)}${targetQuery()}`,
  );
  check('tampered signature is rejected', tampered.status === 403);
}

// --- throughput -------------------------------------------------------------
if (media) {
  const ticket = await api(session, '/api/stream/ticket', { path: media.name });
  const raw = await timedDownload(
    `${BASE_URL}/api/stream?path=${encodeURIComponent(media.name)}&ticket=${encodeURIComponent(ticket.ticket)}${targetQuery()}`,
  );
  check('media file streams without decryption', raw.bytes.length === media.size, `${media.size} bytes`);

  const envelope = await encrypt(session, 'POST /api/preview/stream', { path: media.name });
  const encryptedStarted = process.hrtime.bigint();
  const encrypted = await fetch(`${BASE_URL}/api/preview/stream`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${session.token}`,
      ...targetHeader(),
    },
    body: JSON.stringify(envelope),
  });
  const cipherBytes = new Uint8Array(await encrypted.arrayBuffer());
  const encryptedSeconds = Number(process.hrtime.bigint() - encryptedStarted) / 1e9;
  // Only the transfer is compared here; the browser additionally pays a
  // WebCrypto decrypt per frame on the encrypted path.
  const toMB = (seconds) => (media.size / 1024 / 1024 / Math.max(seconds, 1e-6)).toFixed(1);
  console.log(
    `INFO raw stream ${toMB(raw.seconds)} MB/s · encrypted frames ${toMB(encryptedSeconds)} MB/s ` +
      `(${(cipherBytes.length / raw.bytes.length).toFixed(2)}x wire bytes)`,
  );
  check(
    'raw stream is at least as fast as the encrypted frame stream',
    raw.seconds <= encryptedSeconds,
    `${raw.seconds.toFixed(3)}s vs ${encryptedSeconds.toFixed(3)}s`,
  );
}

if (failures) {
  console.log(`\n${failures} check(s) failed`);
  process.exit(1);
}
console.log('\nall stream checks passed');
