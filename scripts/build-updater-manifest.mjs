#!/usr/bin/env node
// Builds the static Tauri updater manifests for both desktop apps.
//
// Both apps are published into ONE GitHub Release, so they cannot share a
// single `latest.json`. This script reads the release assets, pairs each signed
// updater bundle with its `.sig`, and writes `latest-local.json` and
// `latest-client.json` into OUTPUT_DIR. Only macOS updater bundles are
// considered. When no signed macOS artifacts are present (e.g. the signing
// secret has not been configured yet) it writes nothing, and the apps simply
// fall back to the "download matching .dmg" channel.

import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const REPO = 'fengqshanl/rs-file-broswer';
const APPS = ['local', 'client'];

// Only macOS installers are published for now. Add a rule here when another
// platform is re-enabled in the release workflow. Order matters: the first
// matching bundle wins for a platform.
const PLATFORM_RULES = [
  {
    keys: ['darwin-x86_64', 'darwin-aarch64'],
    patterns: [/\.app\.tar\.gz$/i],
  },
];

export function appSlug(name) {
  const lower = name.toLowerCase();
  // tauri-cli names updater bundles without a version suffix (e.g.
  // `AgentGod.Files.Local.app.tar.gz`), while tauri-action renames the
  // installers it uploads. Accept both shapes.
  if (lower.startsWith('agentgod.files.local')) return 'local';
  if (lower.startsWith('agentgod.files.client')) return 'client';
  return null;
}

export function versionFromTag(tag) {
  const match = String(tag).match(/(\d+\.\d+\.\d+)/);
  return match ? match[1] : null;
}

/** Pure asset-pairing logic, exercised by `--self-test`. */
export function buildManifestSpecs(release) {
  const assets = Array.isArray(release.assets) ? release.assets : [];
  const byName = new Map(assets.map((asset) => [asset.name, asset]));
  const specs = { local: {}, client: {} };
  for (const app of APPS) {
    for (const rule of PLATFORM_RULES) {
      let bundle = null;
      for (const pattern of rule.patterns) {
        bundle =
          assets.find(
            (asset) =>
              appSlug(asset.name) === app &&
              pattern.test(asset.name) &&
              byName.has(`${asset.name}.sig`),
          ) || null;
        if (bundle) break;
      }
      if (!bundle) continue;
      const signature = byName.get(`${bundle.name}.sig`);
      for (const key of rule.keys) {
        specs[app][key] = {
          url: bundle.browser_download_url,
          signature_url: signature.browser_download_url,
        };
      }
    }
  }
  return specs;
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
  const specs = buildManifestSpecs(release);
  return Object.values(specs).some((targets) => Object.keys(targets).length > 0);
}

export async function buildManifests(release, tag, outputDir) {
  const version = versionFromTag(tag);
  if (!version) throw new Error(`无法从 tag 解析版本号：${tag}`);
  const specs = buildManifestSpecs(release);
  await mkdir(outputDir, { recursive: true });
  const written = [];

  for (const app of APPS) {
    const entries = Object.entries(specs[app]);
    if (entries.length === 0) {
      console.log(`skip latest-${app}.json: 该应用没有已签名的更新包`);
      continue;
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
    const file = path.join(outputDir, `latest-${app}.json`);
    await writeFile(file, `${JSON.stringify(manifest, null, 2)}\n`);
    console.log(`wrote ${file} (${entries.length} platforms)`);
    written.push(file);
  }
  if (written.length === 0) {
    console.log('未找到签名更新包，跳过静默更新清单；普通安装包仍可正常下载。');
  }
  return written;
}

async function selfTest() {
  const asset = (name) => ({
    name,
    browser_download_url: `https://example.com/${name}`,
  });
  const release = {
    published_at: '2026-11-12T06:00:00Z',
    assets: [
      asset('AgentGod.Files.Local_0.1.3_universal.app.tar.gz'),
      asset('AgentGod.Files.Local_0.1.3_universal.app.tar.gz.sig'),
      // Legacy Windows/Linux bundles are ignored even when they are signed.
      asset('AgentGod.Files.Local_0.1.3_x64-setup.exe'),
      asset('AgentGod.Files.Local_0.1.3_x64-setup.exe.sig'),
      asset('AgentGod.Files.Local_0.1.3_amd64.AppImage'),
      asset('AgentGod.Files.Local_0.1.3_amd64.AppImage.sig'),
      asset('AgentGod.Files.Client_0.1.3_universal.app.tar.gz'),
      asset('AgentGod.Files.Client_0.1.3_universal.app.tar.gz.sig'),
      asset('latest-local.json'),
    ],
  };
  const darwinKeys = ['darwin-aarch64', 'darwin-x86_64'];
  const specs = buildManifestSpecs(release);
  assert.deepEqual(Object.keys(specs.local).sort(), darwinKeys);
  assert.deepEqual(Object.keys(specs.client).sort(), darwinKeys);
  assert.match(specs.local['darwin-aarch64'].url, /universal\.app\.tar\.gz$/);
  assert.equal(
    specs.local['darwin-x86_64'].signature_url,
    'https://example.com/AgentGod.Files.Local_0.1.3_universal.app.tar.gz.sig',
  );
  // A macOS bundle without its .sig must not produce a manifest entry.
  assert.deepEqual(
    buildManifestSpecs({
      assets: [asset('AgentGod.Files.Local_0.1.3_universal.app.tar.gz')],
    }),
    { local: {}, client: {} },
  );
  assert.equal(versionFromTag('desktop-v0.1.3'), '0.1.3');
  assert.equal(appSlug('latest-local.json'), null);
  assert.equal(appSlug('AgentGod.Files.Local_universal.app.tar.gz'), 'local');
  assert.equal(appSlug('AgentGod.Files.Client.app.tar.gz'), 'client');
  assert.deepEqual(buildManifestSpecs({ assets: [] }), { local: {}, client: {} });
  assert.equal(hasSignedAssets(release), true);
  assert.equal(
    hasSignedAssets({
      assets: [
        asset('AgentGod.Files.Local_0.1.3_x64-setup.exe'),
        asset('AgentGod.Files.Local_0.1.3_x64-setup.exe.sig'),
      ],
    }),
    false,
  );
  console.log('updater manifest self-test OK (macOS only)');
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
        '已配置 TAURI_SIGNING_PRIVATE_KEY，但 Release 中没有 macOS 签名更新包，' +
          '请检查构建日志中的签名步骤',
      );
    }
  }

  await buildManifests(release, tag, outputDir);
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
