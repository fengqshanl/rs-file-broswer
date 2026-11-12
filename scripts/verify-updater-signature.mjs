#!/usr/bin/env node
/**
 * Verifies that a Tauri updater bundle was signed by the private key matching
 * the public key embedded in tauri.conf.json.
 *
 * Tauri's updater refuses to install an update whose signature does not verify
 * against the configured `plugins.updater.pubkey`. A release can therefore
 * build and upload successfully while every installed app still fails at the
 * last step. This script reproduces that verification at release time.
 *
 * Usage:
 *   node scripts/verify-updater-signature.mjs <bundle> <bundle.sig> <tauri.conf.json>
 *
 * The signature file may be either Tauri's base64 `.sig` file or the decoded
 * minisign-style text. Exits non-zero with an actionable message on mismatch.
 */

import { readFileSync } from 'node:fs';
import { createHash, createPublicKey, verify } from 'node:crypto';

const SPKI_ED25519_PREFIX = Buffer.from('302a300506032b6570032100', 'hex');

function fail(message) {
  console.error(`::error::${message}`);
  process.exit(1);
}

function decodeMaybeBase64(text) {
  const trimmed = text.trim();
  if (trimmed.includes('untrusted comment')) return trimmed;
  const decoded = Buffer.from(trimmed.replace(/\s+/g, ''), 'base64').toString('utf8');
  return decoded.includes('untrusted comment') ? decoded : trimmed;
}

function firstPayloadLine(text, prefix) {
  for (const line of text.split(/\r?\n/)) {
    if (line.startsWith(prefix) || line.startsWith(`untrusted comment: ${prefix}`)) {
      return line;
    }
  }
  return null;
}

function parsePublicKey(configPath) {
  let config;
  try {
    config = JSON.parse(readFileSync(configPath, 'utf8'));
  } catch (error) {
    fail(`无法读取 ${configPath}: ${error.message}`);
  }
  const configured = config?.plugins?.updater?.pubkey;
  if (typeof configured !== 'string' || !configured.trim()) {
    fail(`${configPath} 缺少 plugins.updater.pubkey`);
  }
  const text = decodeMaybeBase64(configured);
  const lines = text.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
  const keyLine = lines.find((line) => !line.startsWith('untrusted comment:'));
  if (!keyLine) fail(`${configPath} 的 pubkey 不是有效的 minisign 公钥`);
  const blob = Buffer.from(keyLine, 'base64');
  if (blob.length !== 42 || blob.subarray(0, 2).toString() !== 'Ed') {
    fail(`${configPath} 的 pubkey 不是有效的 Ed25519 minisign 公钥（长度 ${blob.length}）`);
  }
  return { keyId: blob.subarray(2, 10), publicKey: blob.subarray(10, 42) };
}

function parseSignature(signaturePath) {
  let text;
  try {
    text = decodeMaybeBase64(readFileSync(signaturePath, 'utf8'));
  } catch (error) {
    fail(`无法读取签名文件 ${signaturePath}: ${error.message}`);
  }
  const lines = text.split(/\r?\n/).map((line) => line.trim());
  const payload = lines.find(
    (line) =>
      line &&
      !line.startsWith('untrusted comment:') &&
      !line.startsWith('trusted comment:'),
  );
  if (!payload) fail(`签名文件 ${signaturePath} 为空或格式不正确`);
  const blob = Buffer.from(payload, 'base64');
  if (blob.length !== 74) {
    fail(`签名文件 ${signaturePath} 长度异常（${blob.length} 字节）`);
  }
  const algorithm = blob.subarray(0, 2).toString();
  if (algorithm !== 'ED' && algorithm !== 'Ed') {
    fail(`不支持的签名算法 ${JSON.stringify(algorithm)}`);
  }
  return {
    keyId: blob.subarray(2, 10),
    signature: blob.subarray(10, 74),
    prehashed: algorithm === 'ED',
  };
}

function main() {
  const [bundlePath, signaturePath, configPath] = process.argv.slice(2);
  if (!bundlePath || !signaturePath || !configPath) {
    console.error(
      'usage: node scripts/verify-updater-signature.mjs <bundle> <bundle.sig> <tauri.conf.json>',
    );
    process.exit(2);
  }

  const { keyId, publicKey } = parsePublicKey(configPath);
  const signature = parseSignature(signaturePath);

  if (!keyId.equals(signature.keyId)) {
    fail(
      '签名私钥与 tauri.conf.json 中的 pubkey 不匹配：' +
        `公钥 ID ${keyId.toString('hex')}，签名 ID ${signature.keyId.toString('hex')}。` +
        '请把生成 pubkey 时使用的同一把私钥写入 TAURI_SIGNING_PRIVATE_KEY；' +
        '如果旧私钥已丢失，请更换 pubkey 并让旧版本用户手动安装一次。',
    );
  }

  const data = readFileSync(bundlePath);
  const message = signature.prehashed
    ? createHash('blake2b512').update(data).digest()
    : data;
  const key = createPublicKey({
    key: Buffer.concat([SPKI_ED25519_PREFIX, publicKey]),
    format: 'der',
    type: 'spki',
  });
  let valid = false;
  try {
    valid = verify(null, message, key, signature.signature);
  } catch (error) {
    fail(`签名校验过程失败：${error.message}`);
  }
  if (!valid) {
    fail(`${bundlePath} 的签名无效，已安装的应用会在下载后拒绝该更新`);
  }
  console.log(
    `signature OK: ${bundlePath} (key ${keyId.toString('hex')})`,
  );
}

main();
