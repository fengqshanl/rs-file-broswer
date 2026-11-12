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
 * decoded text, so normalize both forms back to newline-free base64.
 *
 * Usage:
 *   SIGNING_KEY=... node scripts/normalize-updater-key.mjs <output.key>
 *   SIGNING_KEY=... node scripts/normalize-updater-key.mjs --classify
 *
 * `--classify` prints what the secret looks like without revealing it:
 *   private-key | public-key | signature | unknown
 */

import { writeFileSync } from 'node:fs';

const args = process.argv.slice(2);
const classifyOnly = args.includes('--classify');
const outputPath = args.find((value) => !value.startsWith('--'));

function classifyMaterial() {
  // GitHub secrets are sometimes pasted with the shell quotes intact.
  let raw = (process.env.SIGNING_KEY || '').trim();
  if (
    raw.length > 1 &&
    ((raw.startsWith('"') && raw.endsWith('"')) ||
      (raw.startsWith("'") && raw.endsWith("'")))
  ) {
    raw = raw.slice(1, -1).trim();
  }
  if (!raw) return { kind: 'empty', raw, text: '' };

  const decoded = Buffer.from(raw.replace(/\s+/g, ''), 'base64').toString('utf8');
  const looksLikeText = raw.includes('untrusted comment');
  const text = looksLikeText ? raw : decoded;

  if (text.includes('rsign encrypted secret key')) {
    return { kind: 'private-key', raw, text, encoded: !looksLikeText };
  }
  if (text.includes('minisign public key')) {
    return { kind: 'public-key', raw, text };
  }
  if (text.includes('signature from tauri secret key')) {
    return { kind: 'signature', raw, text };
  }
  return { kind: 'unknown', raw, text };
}

const material = classifyMaterial();

if (classifyOnly) {
  // Never print the material itself, only its shape and size.
  console.log(`${material.kind} ${material.raw.length}`);
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

// The CLI base64-decodes the entire file, so the payload must have no trailing
// newline (otherwise: "Invalid symbol 10").
const content =
  material.kind === 'private-key' && material.encoded
    ? material.raw.replace(/\s+/g, '')
    : Buffer.from(`${material.text.replace(/\r\n/g, '\n')}\n`, 'utf8').toString(
        'base64',
      );

writeFileSync(outputPath, content, { mode: 0o600 });
console.log(`normalized updater signing key -> ${outputPath}`);
