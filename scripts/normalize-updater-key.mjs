#!/usr/bin/env node
/**
 * Normalizes TAURI_SIGNING_PRIVATE_KEY into the exact `.key` payload that
 * tauri-cli accepts, and writes it to the path given as the first argument.
 *
 * `cargo tauri signer generate` writes the secret key as one line of base64
 * whose payload is the minisign "untrusted comment + key" text. The CLI
 * base64-decodes the *entire* file, so:
 *   - a trailing newline fails with "Invalid symbol 10"
 *   - pasting the decoded two-line text fails with "Invalid symbol 32"
 *   - a URL-encoded paste fails with "Invalid symbol 37" (`%`)
 *
 * All of those forms are recovered here. The output is always canonical
 * base64 of the minisign text, which is what the file on disk looks like.
 *
 * Usage:
 *   SIGNING_KEY=... node scripts/normalize-updater-key.mjs <output.key>
 *   SIGNING_KEY=... node scripts/normalize-updater-key.mjs --classify
 *
 * `--classify` prints `<kind> <length>`, where kind is one of
 *   private-key | public-key | signature | unknown | empty
 * and never reveals the secret itself.
 */

import { writeFileSync } from 'node:fs';

const args = process.argv.slice(2);
const classifyOnly = args.includes('--classify');
const outputPath = args.find((value) => !value.startsWith('--'));

const PRIVATE_MARKER = 'rsign encrypted secret key';
const PUBLIC_MARKER = 'minisign public key';
const SIGNATURE_MARKER = 'signature from tauri secret key';

/** GitHub secrets are sometimes pasted with the shell quotes intact. */
function stripQuotes(value) {
  if (
    value.length > 1 &&
    ((value.startsWith('"') && value.endsWith('"')) ||
      (value.startsWith("'") && value.endsWith("'")))
  ) {
    return value.slice(1, -1).trim();
  }
  return value;
}

function padBase64(value) {
  const remainder = value.length % 4;
  return remainder === 0 ? value : value + '='.repeat(4 - remainder);
}

function urlDecode(value) {
  try {
    return decodeURIComponent(value);
  } catch {
    return null;
  }
}

/**
 * Produces the candidate strings a pasted secret might be hiding in.
 * Ordered from least to most destructive.
 */
function decodeCandidates(raw) {
  const candidates = [];
  const add = (value) => {
    if (value && !candidates.includes(value)) candidates.push(value);
  };

  add(raw);
  const compact = raw.replace(/\s+/g, '');
  add(compact);

  if (compact.includes('%') || raw.includes('%')) {
    // A URL-encoded paste: `=` became `%3D`, `+` became `%2B`, ...
    add(urlDecode(compact));
    // Trailing junk after the key: keep the part before the first escape.
    add(padBase64(compact.slice(0, compact.indexOf('%'))));
    const firstPercent = raw.indexOf('%');
    if (firstPercent > 0) add(padBase64(raw.slice(0, firstPercent)));
  }

  return candidates;
}

function classifyText(text) {
  if (text.includes(PRIVATE_MARKER)) return 'private-key';
  if (text.includes(PUBLIC_MARKER)) return 'public-key';
  if (text.includes(SIGNATURE_MARKER)) return 'signature';
  return null;
}

/**
 * Node's base64 decoder silently skips characters it does not understand, so
 * a polluted paste can still "decode" into a string containing the marker.
 * Only accept text that is structurally a minisign secret key: a comment
 * line, no decoding artifacts, and a payload with the Ed25519 header.
 */
function isValidPrivateKeyText(text) {
  if (!text.startsWith('untrusted comment:')) return false;
  if (!text.includes(PRIVATE_MARKER)) return false;
  if (text.includes('�')) return false;
  const payload = text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .find((line) => line && !line.startsWith('untrusted comment:'));
  if (!payload) return false;
  const blob = Buffer.from(payload, 'base64');
  return blob.length >= 100 && blob.subarray(0, 2).toString() === 'Ed';
}

/**
 * Inspects the secret and returns the canonical key payload when the material
 * is a private key. Nothing here is logged except the classification.
 */
function inspect() {
  const raw = stripQuotes((process.env.SIGNING_KEY || '').trim());
  if (!raw) return { kind: 'empty', canonical: null };

  // The secret may already be the decoded minisign text.
  if (isValidPrivateKeyText(raw)) {
    const text = `${raw.replace(/\r\n/g, '\n').trimEnd()}\n`;
    return { kind: 'private-key', canonical: Buffer.from(text, 'utf8').toString('base64') };
  }

  let kind = classifyText(raw);
  for (const candidate of decodeCandidates(raw)) {
    const text = Buffer.from(candidate, 'base64').toString('utf8');
    if (isValidPrivateKeyText(text)) {
      // Re-encode what we decoded so the CLI sees only valid base64, whatever
      // the paste looked like.
      return { kind: 'private-key', canonical: Buffer.from(text, 'utf8').toString('base64') };
    }
    kind = kind || classifyText(text);
  }

  return { kind: kind || 'unknown', canonical: null };
}

const material = inspect();

if (classifyOnly) {
  // Never print the material itself, only its shape and size.
  const size = stripQuotes((process.env.SIGNING_KEY || '').trim()).length;
  console.log(`${material.kind} ${size}`);
  process.exit(0);
}

if (!outputPath) {
  console.error(
    'usage: SIGNING_KEY=... node scripts/normalize-updater-key.mjs <output.key>',
  );
  process.exit(2);
}

switch (material.kind) {
  case 'empty':
    console.error('::error::TAURI_SIGNING_PRIVATE_KEY 为空');
    process.exit(1);
  case 'public-key':
    console.error(
      '::error::TAURI_SIGNING_PRIVATE_KEY 里是公钥（agentgod-files.key.pub），' +
        '请改贴私钥文件 agentgod-files.key 的完整内容',
    );
    process.exit(1);
  case 'signature':
    console.error(
      '::error::TAURI_SIGNING_PRIVATE_KEY 里是签名文件（.sig），请改贴私钥 agentgod-files.key 的完整内容',
    );
    process.exit(1);
  case 'unknown':
    console.error(
      '::error::TAURI_SIGNING_PRIVATE_KEY 既不是 minisign 私钥也不是其 base64 形式，' +
        '请粘贴 ~/.tauri/agentgod-files.key 的全部内容（应以 dW50cnVzdGVk 开头）',
    );
    process.exit(1);
  default:
    break;
}

if (!material.canonical) {
  console.error(
    '::error::TAURI_SIGNING_PRIVATE_KEY 看起来是私钥，但无法解码出有效的 minisign 内容',
  );
  process.exit(1);
}

// No trailing newline: the CLI base64-decodes the entire file.
writeFileSync(outputPath, material.canonical, { mode: 0o600 });
console.log(`normalized updater signing key -> ${outputPath}`);
