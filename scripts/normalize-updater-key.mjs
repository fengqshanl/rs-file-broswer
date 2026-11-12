#!/usr/bin/env node
/**
 * Normalizes TAURI_SIGNING_PRIVATE_KEY into the on-disk `.key` format that
 * tauri-cli expects and writes it to the path given as the first argument.
 *
 * `cargo tauri signer generate` writes the secret key as a base64 string whose
 * payload is the minisign "untrusted comment + key" text. The CLI, however,
 * does NOT accept that decoded two-line text as a key file:
 *
 *   Error failed to decode base64 secret key: Invalid symbol 32, offset 9.
 *
 * GitHub secrets are often pasted either as the `.key` file contents or as the
 * decoded text, so normalize both forms back to base64 before building.
 *
 * Usage:
 *   SIGNING_KEY=... node scripts/normalize-updater-key.mjs <output.key>
 */

import { writeFileSync } from 'node:fs';

const outputPath = process.argv[2];
if (!outputPath) {
  console.error('usage: SIGNING_KEY=... node scripts/normalize-updater-key.mjs <output.key>');
  process.exit(2);
}

const raw = (() => {
  let value = (process.env.SIGNING_KEY || '').trim();
  // GitHub secrets are sometimes pasted with the shell quotes intact.
  if (
    value.length > 1 &&
    ((value.startsWith('"') && value.endsWith('"')) ||
      (value.startsWith("'") && value.endsWith("'")))
  ) {
    value = value.slice(1, -1).trim();
  }
  return value;
})();
if (!raw) {
  console.error('::error::TAURI_SIGNING_PRIVATE_KEY 为空');
  process.exit(1);
}

const decoded = Buffer.from(raw.replace(/\s+/g, ''), 'base64').toString('utf8');
let content;
if (raw.includes('untrusted comment')) {
  // Pasted as decoded minisign text: normalize line endings, then re-encode
  // into the `.key` file format.
  const text = raw.replace(/\r\n/g, '\n');
  content = Buffer.from(`${text}\n`, 'utf8').toString('base64');
} else if (decoded.includes('untrusted comment')) {
  // Pasted as the `.key` file contents: strip line breaks and keep base64.
  content = raw.replace(/\s+/g, '');
} else {
  console.error(
    '::error::TAURI_SIGNING_PRIVATE_KEY 不是 Tauri 更新签名私钥：' +
      '请粘贴 ~/.tauri/agentgod-files.key 的完整内容（base64），不要粘贴 .pub 公钥',
  );
  process.exit(1);
}

// IMPORTANT: tauri-cli decodes the whole file as base64, so a trailing newline
// becomes "Invalid symbol 10". Write the payload with no trailing whitespace.
writeFileSync(outputPath, content, { mode: 0o600 });
console.log(`normalized updater signing key -> ${outputPath}`);
