#!/usr/bin/env node
/**
 * Verifies that the standalone RockBadger server is API-only:
 * the web UI is gone, the API and health check still work.
 *
 * Usage:
 *   BASE_URL=https://127.0.0.1:8090 INSECURE=1 node scripts/api-only-smoke.mjs
 */

const BASE_URL = (process.env.BASE_URL || 'https://127.0.0.1:8090').replace(/\/+$/, '');
if (process.env.INSECURE === '1') {
  process.env.NODE_TLS_REJECT_UNAUTHORIZED = '0';
}

let failed = 0;
function check(label, condition, detail = '') {
  if (condition) {
    console.log(`PASS  ${label}`);
  } else {
    failed += 1;
    console.error(`FAIL  ${label}${detail ? ` (${detail})` : ''}`);
  }
}

const root = await fetch(`${BASE_URL}/`);
const rootBody = await root.text();
check('GET / returns 410 Gone', root.status === 410, `status ${root.status}`);
check(
  'GET / points to the desktop app',
  rootBody.includes('桌面') && rootBody.includes('/healthz'),
  rootBody.trim().slice(0, 80),
);

for (const path of ['/app.js', '/style.css', '/theme-init.js', '/favicon.svg', '/vendor/mammoth.js']) {
  const response = await fetch(`${BASE_URL}${path}`);
  check(`${path} is not served`, response.status === 404, `status ${response.status}`);
}

const health = await fetch(`${BASE_URL}/healthz`);
check('/healthz returns 200', health.status === 200, `status ${health.status}`);
check('/healthz body is ok', (await health.text()).trim() === 'ok');

const challenge = await fetch(`${BASE_URL}/api/challenge`);
check('/api/challenge returns 200', challenge.status === 200, `status ${challenge.status}`);
const challengeBody = await challenge.json().catch(() => null);
check(
  '/api/challenge returns a challenge',
  Boolean(challengeBody && (challengeBody.challenge || challengeBody.token)),
  JSON.stringify(challengeBody).slice(0, 80),
);

if (failed) {
  console.error(`\nAPI-ONLY SMOKE TEST FAILED (${failed})`);
  process.exit(1);
}
console.log('\nAPI-ONLY SMOKE TEST PASSED');
