#!/usr/bin/env node
/**
 * Media preview smoke test over Chrome DevTools Protocol.
 *
 * Proves the fast path is actually used end to end: grid thumbnails and the
 * image preview must reach the server through `/api/thumb` and `/api/stream`
 * (plain, range-capable responses) rather than decrypting frames in JS.
 *
 * Fixture (see scripts/stream-smoke.mjs): <root>/photo/sample.png (640x480).
 *
 * Usage:
 *   FRONT_URL=http://127.0.0.1:<port>/ CHROME_DEBUG_URL=http://127.0.0.1:9223 \
 *   node scripts/ui-media-smoke.mjs
 */

const DEBUG_URL = (process.env.CHROME_DEBUG_URL || 'http://127.0.0.1:9223').replace(/\/+$/, '');
const PAGE_URL = process.env.FRONT_URL || '';
const FOLDER = process.env.PHOTO_FOLDER || 'photo';
const PHOTO = process.env.PHOTO_NAME || 'sample.png';
const PHOTO_WIDTH = Number(process.env.PHOTO_WIDTH || 640);
const TEXT_FILE = process.env.TEXT_FILE || '';
const AUDIO_FILE = process.env.AUDIO_FILE || '';

if (!PAGE_URL) {
  console.error('FRONT_URL is required');
  process.exit(2);
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const target = await (
  await fetch(`${DEBUG_URL}/json/new?${encodeURIComponent(PAGE_URL)}`, { method: 'PUT' })
).json();
const socket = new WebSocket(target.webSocketDebuggerUrl);
const pending = new Map();
const errors = [];
let nextId = 0;

socket.addEventListener('message', (event) => {
  const message = JSON.parse(event.data);
  if (message.id && pending.has(message.id)) {
    const { resolve, reject } = pending.get(message.id);
    pending.delete(message.id);
    if (message.error) reject(new Error(message.error.message));
    else resolve(message.result);
    return;
  }
  if (message.method === 'Runtime.exceptionThrown') {
    const detail = message.params.exceptionDetails;
    errors.push(detail.exception?.description || detail.text || 'page exception');
  }
  if (message.method === 'Log.entryAdded' && message.params.entry.level === 'error') {
    errors.push(`console: ${message.params.entry.text}`);
  }
});

await new Promise((resolve, reject) => {
  socket.addEventListener('open', resolve);
  socket.addEventListener('error', reject);
});

const send = (method, params = {}) =>
  new Promise((resolve, reject) => {
    const id = ++nextId;
    pending.set(id, { resolve, reject });
    socket.send(JSON.stringify({ id, method, params }));
  });

await send('Runtime.enable');
await send('Page.enable');
await send('Log.enable');

const evaluate = async (expression) => {
  const result = await send('Runtime.evaluate', {
    expression,
    awaitPromise: true,
    returnByValue: true,
  });
  if (result.exceptionDetails) {
    throw new Error(
      result.exceptionDetails.exception?.description ||
        result.exceptionDetails.text ||
        'evaluation failed',
    );
  }
  return result.result.value;
};

const waitFor = async (expression, timeout = 20_000, label = 'condition') => {
  const started = Date.now();
  while (Date.now() - started < timeout) {
    if (await evaluate(expression)) return;
    await sleep(120);
  }
  throw new Error(`timed out waiting for ${label}`);
};

const pass = (label) => console.log(`PASS  ${label}`);

try {
  await waitFor(
    "document.readyState === 'complete' && !document.querySelector('#workspace-bar').hidden",
    25_000,
    'workspace boot',
  );
  pass('workspace booted');

  // The tab remembers the last folder; start from the root so the fixture is
  // always reachable regardless of what an earlier run left behind.
  await waitFor("!!document.querySelector('#breadcrumbs .crumb')", 20_000, 'breadcrumbs');
  await evaluate("document.querySelector('#breadcrumbs .crumb').click(); true");
  await waitFor(
    `[...document.querySelectorAll('#file-list .file-row .name-btn')].length > 0 ||
     [...document.querySelectorAll('#file-list .card-name')].length > 0`,
    20_000,
    'root listing',
  );

  // The saved layout decides whether the fixture is a row or a card.
  const names = `[...document.querySelectorAll('#file-list .name-btn, #file-list .card-name, #file-list .file-name')]`;
  await waitFor(
    `${names}.some((node) => node.textContent === ${JSON.stringify(FOLDER)})`,
    20_000,
    'folder entry',
  );
  await evaluate(`(() => {
    ${names}.find((node) => node.textContent === ${JSON.stringify(FOLDER)}).click();
    return true;
  })()`);
  await waitFor(
    `${names}.some((node) => node.textContent === ${JSON.stringify(PHOTO)})`,
    20_000,
    'photo listed',
  );
  pass('navigated into the photo folder');

  await evaluate("document.querySelector('#view-switch [data-view-mode=\"grid\"]').click(); true");
  await waitFor("!!document.querySelector('#file-list .file-card')", 10_000, 'grid layout');
  pass('grid layout rendered');

  await waitFor(
    `(() => {
       const image = document.querySelector('#file-list .file-card .thumb-image');
       return !!image && image.complete && image.naturalWidth > 0;
     })()`,
    25_000,
    'thumbnail painted',
  );
  const thumbnail = await evaluate(`(() => {
    const image = document.querySelector('#file-list .file-card .thumb-image');
    return { src: image.getAttribute('src'), width: image.naturalWidth };
  })()`);
  if (!/\/api\/thumb\?/.test(thumbnail.src)) {
    throw new Error(`thumbnail did not use the direct endpoint: ${thumbnail.src}`);
  }
  pass(`thumbnail painted straight from /api/thumb (${thumbnail.width}px)`);

  await evaluate(`(() => {
    const name = document.querySelector('#file-list .file-card .card-name.previewable');
    name.click();
    return true;
  })()`);
  await waitFor("!!document.querySelector('#modal[open]')", 10_000, 'preview dialog');
  await waitFor(
    `(() => {
       const image = document.querySelector('#modal .preview-image');
       return !!image && image.complete && image.naturalWidth > 0;
     })()`,
    25_000,
    'preview image decoded',
  );
  const preview = await evaluate(`(() => {
    const dialog = document.querySelector('#modal');
    const image = dialog.querySelector('.preview-image');
    const resources = performance
      .getEntriesByType('resource')
      .filter((entry) => entry.name.includes('/api/stream?') || entry.name.includes('/api/thumb?'))
      .map((entry) => ({
        path: new URL(entry.name).pathname,
        status: entry.responseStatus,
        bytes: entry.transferSize || entry.encodedBodySize,
      }));
    return {
      width: image.naturalWidth,
      height: image.naturalHeight,
      src: image.getAttribute('src'),
      statusLeft: !!dialog.querySelector('.preview-media-status'),
      resources,
    };
  })()`);

  if (preview.width !== PHOTO_WIDTH) {
    throw new Error(`unexpected preview width: ${preview.width}`);
  }
  if (!/\/api\/stream\?/.test(preview.src)) {
    throw new Error(`preview did not stream: ${preview.src}`);
  }
  if (preview.statusLeft) throw new Error('loading label still visible after load');
  const streamed = preview.resources.filter(
    (entry) => entry.path === '/api/stream' && entry.status === 200,
  );
  if (!streamed.length) {
    throw new Error(`no 200 /api/stream request recorded: ${JSON.stringify(preview.resources)}`);
  }
  pass(
    `full image decoded from /api/stream (${preview.width}x${preview.height}, ` +
      `${streamed[0].bytes} bytes on the wire)`,
  );

  // Seeking a video must be possible without downloading it first: the server
  // answers range requests, which the smoke test in scripts/stream-smoke.mjs
  // asserts at the protocol level. Here we only make sure no frame-by-frame
  // encrypted preview ran for the image path.
  const legacy = preview.resources.filter((entry) => entry.path === '/api/preview/stream');
  if (legacy.length) throw new Error('preview still fell back to the encrypted stream');
  pass('no encrypted frame stream involved');

  if (AUDIO_FILE) {
    // A media element only needs the header to learn the duration, then it asks
    // for ranges as playback advances — impossible before, when previews had to
    // download and decrypt the whole file first.
    await evaluate("document.querySelector('#modal').close(); true");
    await waitFor("!document.querySelector('#modal[open]')", 10_000, 'dialog closed');
    await evaluate("document.querySelector('#breadcrumbs .crumb').click(); true");
    await waitFor(
      `${names}.some((node) => node.textContent === ${JSON.stringify(AUDIO_FILE)})`,
      20_000,
      'audio file listed',
    );
    await evaluate(`(() => {
      ${names}.find((node) => node.textContent === ${JSON.stringify(AUDIO_FILE)}).click();
      return true;
    })()`);
    await waitFor(
      `(() => {
         const audio = document.querySelector('#modal audio');
         return !!audio && audio.readyState >= 1 && Number.isFinite(audio.duration);
       })()`,
      25_000,
      'audio metadata',
    );
    const audio = await evaluate(`(() => {
      const element = document.querySelector('#modal audio');
      const entry = performance
        .getEntriesByType('resource')
        .filter((item) => item.name.includes('/api/stream?'))
        .find((item) => /path=(%2F)?song/.test(item.name));
      return {
        duration: element.duration,
        streamed: !!entry,
        preload: element.preload,
      };
    })()`);
    if (!audio.streamed) throw new Error('audio preview did not use /api/stream');
    pass(
      `audio metadata from a ranged stream (${audio.duration.toFixed(1)}s, preload=${audio.preload})`,
    );
  }

  if (TEXT_FILE) {
    // Text previews pull the same raw stream into memory instead of decrypting
    // frames, so they cover the other consumer of /api/stream.
    await evaluate("document.querySelector('#modal').close(); true");
    await waitFor("!document.querySelector('#modal[open]')", 10_000, 'dialog closed');
    await evaluate("document.querySelector('#breadcrumbs .crumb').click(); true");
    await waitFor(
      `[...document.querySelectorAll('#file-list .card-name')].some(
         (node) => node.textContent === ${JSON.stringify(TEXT_FILE)},
       )`,
      20_000,
      'text file listed',
    );
    await evaluate(`(() => {
      const name = [...document.querySelectorAll('#file-list .card-name')]
        .find((node) => node.textContent === ${JSON.stringify(TEXT_FILE)});
      name.click();
      return true;
    })()`);
    await waitFor(
      `(() => {
         const pre = document.querySelector('#modal .preview-text');
         return !!pre && pre.textContent.includes('hello rockbadger');
       })()`,
      20_000,
      'text preview content',
    );
    const textEntry = await evaluate(`(() => {
      const entries = performance
        .getEntriesByType('resource')
        .filter((item) => item.name.includes('/api/stream?'))
        .map((item) => ({ name: new URL(item.name).pathname + new URL(item.name).search.split('&')[0], status: item.responseStatus }));
      return entries;
    })()`);
    if (!textEntry.some((entry) => entry.status === 200 && /path=(%2F)?notes/.test(entry.name))) {
      throw new Error(`text preview did not use /api/stream: ${JSON.stringify(textEntry)}`);
    }
    pass('text preview read through /api/stream');
  }

  if (errors.length) throw new Error(`page errors: ${errors.join(' | ')}`);
  pass('no console errors');

  console.log('\nmedia preview smoke passed');
} catch (error) {
  console.error(`FAIL  ${error.message}`);
  if (errors.length) console.error(`page errors: ${errors.join(' | ')}`);
  process.exitCode = 1;
} finally {
  await fetch(`${DEBUG_URL}/json/close/${target.id}`).catch(() => {});
  socket.close();
}
