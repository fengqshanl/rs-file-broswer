#!/usr/bin/env node
/**
 * Starts the desktop companion (`dev_app`) without the Tauri CLI.
 *
 * The Tauri window is only a native shell around this loopback front end; the
 * same UI and API run here and open in the default browser. Use this when
 * `cargo tauri` is not installed or when a full Tauri build is too heavy for
 * the current machine.
 *
 * Usage:
 *   node scripts/dev-ui.mjs [--no-open]
 *
 * Environment overrides:
 *   ROCKBADGER_DATA_DIR, ROCKBADGER_LOCAL_ROOT, ROCKBADGER_MEMORY_SECRETS
 */

import { spawn, spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const manifest = join(root, 'desktop', 'common', 'Cargo.toml');
const binary = join(
  root,
  'desktop',
  'common',
  'target',
  'debug',
  process.platform === 'win32' ? 'dev_app.exe' : 'dev_app',
);

if (!existsSync(binary)) {
  console.log('dev_app 尚未构建，正在执行 cargo build --bin dev_app …');
  const build = spawnSync(
    'cargo',
    ['build', '--manifest-path', manifest, '--bin', 'dev_app'],
    { stdio: 'inherit' },
  );
  if (build.status !== 0) {
    console.error('构建失败：请先确认 cargo 在 PATH 中，并保留足够的内存/页面文件。');
    process.exit(build.status ?? 1);
  }
}

const env = {
  ...process.env,
  ROCKBADGER_DATA_DIR:
    process.env.ROCKBADGER_DATA_DIR || join(tmpdir(), 'rockbadger-dev-data'),
  ROCKBADGER_LOCAL_ROOT:
    process.env.ROCKBADGER_LOCAL_ROOT ||
    (process.env.SystemDrive ? `${process.env.SystemDrive}\\` : '/'),
  ROCKBADGER_MEMORY_SECRETS:
    process.env.ROCKBADGER_MEMORY_SECRETS || '1',
};

console.log(`数据目录: ${env.ROCKBADGER_DATA_DIR}`);
console.log(`本机根目录: ${env.ROCKBADGER_LOCAL_ROOT}`);

const child = spawn(binary, [], { env, stdio: ['ignore', 'pipe', 'inherit'] });
let opened = false;

function openBrowser(url) {
  if (process.argv.includes('--no-open')) return;
  const command =
    process.platform === 'win32'
      ? ['cmd', ['/c', 'start', '', url]]
      : process.platform === 'darwin'
        ? ['open', [url]]
        : ['xdg-open', [url]];
  spawn(command[0], command[1], {
    stdio: 'ignore',
    detached: true,
    windowsHide: true,
  }).unref();
}

child.stdout.setEncoding('utf8');
let buffered = '';
child.stdout.on('data', (chunk) => {
  buffered += chunk;
  const lines = buffered.split(/\r?\n/);
  buffered = lines.pop() ?? '';
  for (const line of lines) {
    if (!opened && /^https?:\/\//.test(line.trim())) {
      opened = true;
      const url = line.trim();
      console.log(`\nRockBadger 界面: ${url}\n`);
      openBrowser(url);
    } else if (line.trim()) {
      console.log(line);
    }
  }
});

child.on('error', (error) => {
  console.error(`无法启动 dev_app: ${error.message}`);
  process.exit(1);
});

child.on('exit', (code) => {
  if (!opened) {
    console.error('dev_app 未输出访问地址，启动失败。');
  }
  process.exit(code ?? 1);
});

for (const signal of ['SIGINT', 'SIGTERM']) {
  process.on(signal, () => {
    child.kill(signal);
  });
}
