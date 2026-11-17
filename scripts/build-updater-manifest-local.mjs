#!/usr/bin/env node
/**
 * Builds `latest.json` from local build artifacts.
 *
 * The Windows build job already has the MSI and its `.sig` on disk. Generating
 * the manifest there avoids reading assets back from a draft release, which the
 * Actions `GITHUB_TOKEN` cannot access through the REST releases API.
 *
 * Usage:
 *   VERSION=0.3.4 GITHUB_REPOSITORY=owner/repo \
 *     node scripts/build-updater-manifest-local.mjs <bundle.msi> <bundle.msi.sig> <out.json>
 */

import { readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';

const [msiPath, sigPath, outPath] = process.argv.slice(2);
const version = process.env.VERSION;
const repo = process.env.GITHUB_REPOSITORY;

if (!msiPath || !sigPath || !outPath || !version || !repo) {
  console.error(
    'usage: VERSION=<version> GITHUB_REPOSITORY=<owner/repo> ' +
      'node scripts/build-updater-manifest-local.mjs <msi> <sig> <out.json>',
  );
  process.exit(2);
}

const signature = (await readFile(sigPath, 'utf8')).trim();
if (!signature) {
  throw new Error(`签名文件为空：${sigPath}`);
}
if (!msiPath.toLowerCase().endsWith('.msi')) {
  throw new Error(`不是 MSI 安装包：${msiPath}`);
}

// The workflow uploads the MSI under this deterministic ASCII name.
const assetName = `RockBadger_${version}_x64.msi`;
const url = `https://github.com/${repo}/releases/download/rockbadger-v${version}/${assetName}`;
const platforms = {};
for (const key of ['windows-x86_64', 'windows-aarch64']) {
  platforms[key] = { signature, url };
}

const manifest = {
  version,
  notes: `RockBadger 桌面版 v${version}`,
  pub_date: new Date().toISOString(),
  platforms,
};
await writeFile(outPath, `${JSON.stringify(manifest, null, 2)}\n`);
console.log(`wrote ${outPath} (${path.basename(msiPath)} -> ${assetName})`);
