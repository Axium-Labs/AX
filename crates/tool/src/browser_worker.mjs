// Embedded AX source; no model-provided code or shell commands are evaluated.
import readline from 'node:readline';
import { createRequire } from 'node:module';
import path from 'node:path';
import os from 'node:os';
import { randomUUID } from 'node:crypto';

const home = process.env.AX_BROWSER_HOME;
const require = createRequire(path.join(home, 'browser', 'package.json'));
let activeOrigin = null, browser;
const sessions = new Map();
let blocked = new Set(), transportError;
// Playwright call logs may include Cookie/Authorization headers. Return only
// the operation error, never those request diagnostics.
function errorMessage(error) { return String(error.message ?? error).split('\nCall log:')[0].slice(0, 2000); }
// Rust cancellation closes the pipes. Close Chromium before exiting on EPIPE.
process.stdout.on('error', async () => { await browser?.close().catch(() => {}); process.exit(0); });
function origin(url) { try { const u = new URL(url); return ['http:', 'https:'].includes(u.protocol) && !u.username && !u.password ? u.origin : null; } catch { return null; } }
async function start() {
  if (browser) return;
  let playwright;
  const candidates = [process.env.AX_BROWSER_PLAYWRIGHT, 'playwright', process.env.APPDATA && path.join(process.env.APPDATA, 'npm', 'node_modules', 'playwright')].filter(Boolean);
  for (const candidate of candidates) { try { playwright = require(candidate); break; } catch {} }
  if (!playwright) throw Error('Install Playwright with npm install --prefix "<AX_HOME>/browser" playwright, then npx --prefix "<AX_HOME>/browser" playwright install chromium.');
  browser = await playwright.chromium.launch({ headless: process.env.AX_BROWSER_HEADLESS === 'true', channel: process.env.AX_BROWSER_CHANNEL || undefined, chromiumSandbox: true, timeout: 20000 });
}
async function session(name, create) {
  if (sessions.has(name)) return sessions.get(name);
  if (!create) throw Error('Browser session does not exist; open a URL first');
  await start();
  const context = await browser.newContext({ acceptDownloads: false, serviceWorkers: 'block', permissions: [], viewport: { width: 1280, height: 900 } });
  context.setDefaultTimeout(10000);
  await context.route('**/*', async route => {
    const target = origin(route.request().url());
    if (!activeOrigin || target !== activeOrigin) { if (target) blocked.add(target); return route.abort('blockedbyclient'); }
    // Playwright routing alone does not intercept every redirect hop. Fetch
    // with redirects disabled and validate each hop before sending it.
    let url = route.request().url();
    try {
      for (let count = 0; count < 20; count++) {
        const response = await route.fetch({ url, maxRedirects: 0, timeout: 15000 });
        const location = response.headers().location;
        if (response.status() >= 300 && response.status() < 400 && location) {
          url = new URL(location, url).href;
          if (origin(url) !== activeOrigin) { const denied = origin(url); if (denied) blocked.add(denied); return route.abort('blockedbyclient'); }
          continue;
        }
        const headers = { ...response.headers() };
        // A fetched response is served as a completed body, not a live stream.
        delete headers['transfer-encoding'];
        return route.fulfill({ response, headers });
      }
      return route.abort('blockedbyclient');
    } catch (error) { transportError = errorMessage(error); return route.abort('blockedbyclient'); }
  });
  if (!context.routeWebSocket) { await context.close(); throw Error('Update Playwright: WebSocket blocking support is required'); }
  await context.routeWebSocket('**/*', socket => socket.close());
  const page = await context.newPage();
  context.on('page', other => { if (other !== page) void other.close(); });
  page.on('download', download => void download.cancel());
  page.on('dialog', dialog => void dialog.dismiss());
  const entry = { context, page }; sessions.set(name, entry); return entry;
}
async function dispatch({ input, origin: permitted }) {
  const name = input.session ?? 'main';
  if (input.action === 'list_sessions') return { sessions: [...sessions].map(([session, item]) => ({ session, origin: origin(item.page.url()) })) };
  if (input.action === 'close') { const item = sessions.get(name); if (item) await item.context.close(); sessions.delete(name); if (!sessions.size) { await browser?.close(); browser = undefined; } return { status: 'closed' }; }
  if (input.action === 'probe') { const item = sessions.get(name); if (!item) throw Error('No owned browser session; open a URL first'); return { url: item.page.url() }; }
  if (!permitted || origin(permitted) !== permitted) throw Error('Missing authorized browser origin');
  const { page, context } = await session(name, ['open', 'goto'].includes(input.action));
  if (!['open', 'goto'].includes(input.action) && origin(page.url()) !== permitted) throw Error('Browser origin changed; request access again');
  if (['open', 'goto'].includes(input.action) && origin(input.url) !== permitted) throw Error('Navigation target differs from approved origin');
  activeOrigin = permitted; blocked = new Set(); transportError = undefined;
  try {
    switch (input.action) {
      case 'open': case 'goto': await page.goto(input.url, { waitUntil: 'domcontentloaded', timeout: 15000 }); break;
      case 'snapshot': case 'screenshot': break;
      case 'click': await page.locator(input.target).click(); break;
      case 'fill': await page.locator(input.target).fill(input.text); break;
      case 'type': await page.keyboard.insertText(input.text); break;
      case 'press': await page.keyboard.press(input.key); break;
      case 'go_back': await page.goBack({ waitUntil: 'domcontentloaded' }); break;
      case 'go_forward': await page.goForward({ waitUntil: 'domcontentloaded' }); break;
      case 'reload': await page.reload({ waitUntil: 'domcontentloaded' }); break;
      default: throw Error('Unknown browser action');
    }
    if (origin(page.url()) !== permitted) { throw Error('Navigation left the authorized origin; page closed before inspection'); }
    const result = { status: 'completed', url: page.url().slice(0, 8192), title: (await page.title()).slice(0, 2000), session: name, blocked_origins: [...blocked] };
    if (input.action === 'screenshot') {
      if (await page.locator('input[type="password"]').count()) result.screenshot_skipped = 'password_field';
      else { const file = path.join(os.tmpdir(), `ax-browser-${randomUUID()}.png`); await page.screenshot({ path: file }); result.screenshot_path = file; }
    } else {
      // Password values must never reach the agent through an accessibility snapshot.
      if (await page.locator('input[type="password"]').count()) result.snapshot_skipped = 'password_field';
      else result.snapshot = (await page.locator('body').ariaSnapshot()).slice(0, 32000);
    }
    return result;
  } catch (error) {
    if (['open', 'goto', 'reload', 'go_back', 'go_forward'].includes(input.action) || origin(page.url()) !== permitted) {
      activeOrigin = null;
      await context.close().catch(() => {}); sessions.delete(name);
      if (!sessions.size) { await browser?.close(); browser = undefined; }
    }
    throw error;
  } finally { activeOrigin = null; }
}
const lines = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
for await (const line of lines) {
  let timer;
  try {
    if (line.length > 1024 * 1024) throw Error('Browser request exceeds limit');
    const result = await Promise.race([
      dispatch(JSON.parse(line)),
      new Promise((_, reject) => { timer = setTimeout(async () => { await browser?.close().catch(() => {}); browser = undefined; sessions.clear(); reject(Error('Browser operation timed out; owned sessions closed')); }, 25000); }),
    ]);
    process.stdout.write(JSON.stringify(result) + '\n');
  }
  catch (error) { process.stdout.write(JSON.stringify({ error: errorMessage(error) + (transportError ? `\nBrowser transport: ${transportError}` : '') + (blocked.size ? `\nBlocked origins: ${[...blocked].slice(0, 64).join(', ')}` : ''), blocked_origins: [...blocked].slice(0, 64) }) + '\n'); }
  finally { clearTimeout(timer); }
}
await browser?.close();
