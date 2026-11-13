#!/usr/bin/env node
// Builds the static Tauri updater manifest for the desktop app.
//
// The app publishes one MSI per release. This script reads the release assets,
// pairs the MSI with its `.sig`, and writes `latest.json` into OUTPUT_DIR. Only
// Windows MSI packages are considered. When no signed MSI is present (e.g. the
// signing secret has not been configured yet) it writes nothing, and users
// simply download the MSI manually.

import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const REPO = 'fengqshanl/rs-file-broswer';
const MANIFEST_NAME = 'latest.json';

// Only Windows MSI installers are published for now. Add a rule here when
// another platform is re-enabled in the release workflow. Order matters: the
// first matching bundle wins for a platform.
const PLATFORM_RULES = [
  {
    // The MSI is x64; Windows on ARM installs it through emulation and its
    // updater reports windows-x86_64, but some Tauri versions look up the
    // native key first, so publish both entries pointing at the same package.
    keys: ['windows-x86_64', 'windows-aarch64'],
    patterns: [/\.msi$/i],
  },
];

/** True for assets produced by this app (`AgentGod.Files_0.2.0_x64.msi`). */
export function isAppAsset(name) {
  const compact = String(name).toLowerCase().replace(/[\s._-]+/g, '');
  // Reject the retired per-app builds (`AgentGod.Files.Local_...`).
  if (compact.startsWith('agentgodfileslocal')) return false;
  if (compact.startsWith('agentgodfilesclient')) return false;
  return compact.startsWith('agentgodfiles');
}

export function versionFromTag(tag) {
  const match = String(tag).match(/(\d+\.\d+\.\d+)/);
  return match ? match[1] : null;
}

/** Pure asset-pairing logic, exercised by `--self-test`. */
export function buildManifestSpec(release) {
  const assets = Array.isArray(release.assets) ? release.assets : [];
  const byName = new Map(assets.map((asset) => [asset.name, asset]));
  const platforms = {};
  for (const rule of PLATFORM_RULES) {
    let bundle = null;
    for (const pattern of rule.patterns) {
      const candidates = assets.filter(
        (asset) =>
          isAppAsset(asset.name) &&
          pattern.test(asset.name) &&
          byName.has(`${asset.name}.sig`),
      );
      if (!candidates.length) continue;
      // Prefer the ASCII asset name so the manifest URL stays stable.
      candidates.sort(
        (a, b) =>
          (a.name.match(/\s/g) || []).length - (b.name.match(/\s/g) || []).length ||
          a.name.localeCompare(b.name),
      );
      bundle = candidates[0];
      break;
    }
    if (!bundle) continue;
    const signature = byName.get(`${bundle.name}.sig`);
    for (const key of rule.keys) {
      platforms[key] = {
        url: bundle.browser_download_url,
        signature_url: signature.browser_download_url,
      };
    }
  }
  return platforms;
}

async function github(pathname) {
  const headers = {
    'User-Agent': 'agentgod-files-release',
    Accept: 'application/vnd.github+json',
    // GitHub's API can serve a cached asset list for up to a minute. The
    // manifest job runs right after the uploads, so bypass that cache.
    'Cache-Control': 'no-cache',
  };
  if (process.env.GH_TOKEN) {
    headers.Authorization = `Bearer ${process.env.GH_TOKEN}`;
  }
  const separator = pathname.includes('?') ? '&' : '?';
  const url = `https://api.github.com${pathname}${separator}ts=${Date.now()}`;
  const response = await fetch(url, { headers });
  if (!response.ok) {
    throw new Error(`GitHub API ${response.status}: ${await response.text()}`);
  }
  return response.json();
}

async function readSignature(url) {
  // A freshly uploaded asset can take a moment to become visible on the CDN.
  let lastError;
  for (let attempt = 0; attempt < 4; attempt += 1) {
    try {
      const response = await fetch(url, { headers: { 'Cache-Control': 'no-cache' } });
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      const signature = (await response.text()).trim();
      if (!signature) throw new Error('签名文件为空');
      return signature;
    } catch (error) {
      lastError = error;
      await new Promise((resolve) => setTimeout(resolve, 1500 * (attempt + 1)));
    }
  }
  throw new Error(`无法读取签名文件 ${url}: ${lastError?.message || lastError}`);
}

/** Only a signed bundle for a currently published platform counts. */
export function hasSignedAssets(release) {
  return Object.keys(buildManifestSpec(release)).length > 0;
}

export async function buildManifest(release, tag, outputDir) {
  const version = versionFromTag(tag);
  if (!version) throw new Error(`无法从 tag 解析版本号：${tag}`);
  const specs = buildManifestSpec(release);
  const entries = Object.entries(specs);
  if (entries.length === 0) {
    console.log(`skip ${MANIFEST_NAME}: 没有已签名的更新包`);
    return [];
  }
  const platforms = {};
  for (const [target, spec] of entries) {
    platforms[target] = {
      signature: await readSignature(spec.signature_url),
      url: spec.url,
    };
  }
  const manifest = {
    version,
    notes: `AgentGod Files 桌面版 v${version}`,
    pub_date: release.published_at || new Date().toISOString(),
    platforms,
  };
  await mkdir(outputDir, { recursive: true });
  const file = path.join(outputDir, MANIFEST_NAME);
  await writeFile(file, `${JSON.stringify(manifest, null, 2)}\n`);
  console.log(`wrote ${file} (${entries.length} platforms)`);
  return [file];
}

async function selfTest() {
  const asset = (name) => ({
    name,
    browser_download_url: `https://example.com/${name}`,
  });
  const release = {
    published_at: '2026-11-13T06:00:00Z',
    assets: [
      asset('AgentGod.Files_0.2.0_x64.msi'),
      asset('AgentGod.Files_0.2.0_x64.msi.sig'),
      // Retired macOS/Linux/NSIS bundles and pre-merge per-app builds are
      // ignored even when signed.
      asset('AgentGod.Files_0.2.0_universal.dmg'),
      asset('AgentGod.Files_0.2.0_universal.app.tar.gz'),
      asset('AgentGod.Files_0.2.0_universal.app.tar.gz.sig'),
      asset('AgentGod.Files_0.2.0_x64-setup.exe'),
      asset('AgentGod.Files_0.2.0_x64-setup.exe.sig'),
      asset('AgentGod.Files.Local_0.1.6_x64.msi'),
      asset('AgentGod.Files.Local_0.1.6_x64.msi.sig'),
      asset('AgentGod.Files.Client_0.1.6_x64.msi'),
      asset('AgentGod.Files.Client_0.1.6_x64.msi.sig'),
      asset('latest.json'),
      asset('latest-local.json'),
    ],
  };
  const windowsKeys = ['windows-aarch64', 'windows-x86_64'];
  const specs = buildManifestSpec(release);
  assert.deepEqual(Object.keys(specs).sort(), windowsKeys);
  assert.match(specs['windows-x86_64'].url, /AgentGod\.Files_0\.2\.0_x64\.msi$/);
  assert.equal(
    specs['windows-aarch64'].signature_url,
    'https://example.com/AgentGod.Files_0.2.0_x64.msi.sig',
  );
  // An MSI without its .sig must not produce a manifest entry.
  assert.deepEqual(
    buildManifestSpec({ assets: [asset('AgentGod.Files_0.2.0_x64.msi')] }),
    {},
  );
  // Pre-merge per-app assets must never be offered to the merged app.
  assert.deepEqual(
    buildManifestSpec({
      assets: [
        asset('AgentGod.Files.Local_0.1.6_x64.msi'),
        asset('AgentGod.Files.Local_0.1.6_x64.msi.sig'),
      ],
    }),
    {},
  );
  assert.equal(versionFromTag('desktop-v0.2.0'), '0.2.0');
  assert.equal(isAppAsset('AgentGod.Files_0.2.0_x64.msi'), true);
  assert.equal(isAppAsset('AgentGod.Files_0.2.0_x64.msi.sig'), true);
  assert.equal(isAppAsset('AgentGod.Files.Local_0.1.6_x64.msi'), false);
  assert.equal(isAppAsset('latest-local.json'), false);
  assert.deepEqual(buildManifestSpec({ assets: [] }), {});
  assert.equal(hasSignedAssets(release), true);
  assert.equal(
    hasSignedAssets({
      assets: [
        asset('AgentGod.Files_0.2.0_universal.dmg'),
        asset('AgentGod.Files_0.2.0_universal.app.tar.gz'),
        asset('AgentGod.Files_0.2.0_universal.app.tar.gz.sig'),
      ],
    }),
    false,
  );
  console.log('updater manifest self-test OK (single app, Windows MSI only)');
}

async function main() {
  const tag = process.env.RELEASE_TAG;
  if (!tag) throw new Error('缺少 RELEASE_TAG 环境变量');
  const outputDir = process.env.OUTPUT_DIR || process.cwd();
  const expectSigned = process.env.EXPECT_SIGNED === '1';
  const releasePath = `/repos/${REPO}/releases/tags/${encodeURIComponent(tag)}`;
  let release = await github(releasePath);

  // When the signing secret is configured, missing signatures mean the build
  // silently degraded. Wait for the upload to become visible, then fail loudly
  // instead of publishing a release without the hot-update channel.
  if (expectSigned) {
    for (let attempt = 0; attempt < 8 && !hasSignedAssets(release); attempt += 1) {
      console.log(`waiting for signed assets to appear (${attempt + 1}/8)...`);
      await new Promise((resolve) => setTimeout(resolve, 5000));
      release = await github(releasePath);
    }
    if (!hasSignedAssets(release)) {
      throw new Error(
        '已配置 TAURI_SIGNING_PRIVATE_KEY，但 Release 中没有签名的 Windows 更新包，' +
          '请检查构建日志中的签名步骤',
      );
    }
  }

  await buildManifest(release, tag, outputDir);
}

const invokedDirectly =
  process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href;
if (invokedDirectly) {
  if (process.argv.includes('--self-test')) {
    selfTest().catch((error) => {
      console.error(error);
      process.exitCode = 1;
    });
  } else {
    main().catch((error) => {
      console.error(error);
      process.exitCode = 1;
    });
  }
}
