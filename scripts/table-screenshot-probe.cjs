#!/usr/bin/env node
// Render the markdown-table markup (as the planner's marked renderer produces it)
// against the BUILT global stylesheet, and screenshot it — so we can verify the
// table borders are actually visible before deploying.
const fs = require('node:fs');
const http = require('node:http');
const path = require('node:path');
const puppeteer = require('puppeteer');

const ROOT = path.resolve(__dirname, '..');
const DIST = path.join(ROOT, 'dist', 'web', 'browser');
const OUT = path.join(ROOT, 'e2e-artifacts', 'table-borders.png');
const PORT = 4331;

function startServer() {
  const mime = { '.css': 'text/css', '.js': 'text/javascript', '.html': 'text/html' };
  const server = http.createServer((req, res) => {
    const p = path.normalize(path.join(DIST, decodeURIComponent((req.url || '/').split('?')[0])));
    if (!p.startsWith(DIST)) { res.writeHead(403).end(); return; }
    fs.readFile(p, (e, d) => {
      if (e) { res.writeHead(404).end(); return; }
      res.writeHead(200, { 'Content-Type': mime[path.extname(p)] || 'application/octet-stream' });
      res.end(d);
    });
  });
  return new Promise((r) => server.listen(PORT, '127.0.0.1', () => r(server)));
}

async function main() {
  const stylesFile = fs.readdirSync(DIST).find((f) => /^styles.*\.css$/.test(f));
  if (!stylesFile) { console.error('no built styles*.css'); process.exit(1); }
  const server = await startServer();
  const browser = await puppeteer.launch({ headless: true, args: ['--no-sandbox', '--disable-setuid-sandbox'] });
  try {
    const page = await browser.newPage();
    await page.setViewport({ width: 900, height: 520, deviceScaleFactor: 2 });
    // The exact structure the marked renderer emits (markdown-table-wrap > table).
    const html = `<!doctype html><html><head>
      <link rel="stylesheet" href="http://127.0.0.1:${PORT}/${stylesFile}">
      <style>html,body{margin:0;background:#0d1117;padding:24px;font-family:Inter,system-ui,sans-serif;}</style>
      </head><body>
      <article class="plan-markdown-preview markdown-content compact">
        <h3>Key design decisions (locked for v1)</h3>
        <div class="markdown-table-wrap"><table>
          <thead><tr><th>Decision</th><th>Choice</th><th>Rationale</th></tr></thead>
          <tbody>
            <tr><td>Control-plane host</td><td>marketplace.lokalbase.app only</td><td>Single dispatch UI / deploy</td></tr>
            <tr><td>Roles</td><td>ADMIN, CLIENT (assigned apps)</td><td>Least privilege per tenant</td></tr>
            <tr><td>App visibility</td><td>public | internal | unlisted</td><td>Catalog scoping via internal</td></tr>
            <tr><td>Deploy DB</td><td>Fresh D1 (no migration)</td><td>Avoid stale data migration</td></tr>
          </tbody>
        </table></div>
      </article>
      </body></html>`;
    await page.setContent(html, { waitUntil: 'networkidle0' });
    await new Promise((r) => setTimeout(r, 300));
    // Report the computed border on a body cell so we have a non-visual assertion too.
    const border = await page.evaluate(() => {
      const td = document.querySelector('.plan-markdown-preview.markdown-content tbody td');
      const cs = td ? getComputedStyle(td) : null;
      return cs ? { borderTop: cs.borderTopWidth + ' ' + cs.borderTopStyle + ' ' + cs.borderTopColor } : null;
    });
    fs.mkdirSync(path.dirname(OUT), { recursive: true });
    const el = await page.$('.plan-markdown-preview');
    await el.screenshot({ path: OUT });
    console.log('computed td border:', JSON.stringify(border));
    console.log('screenshot:', OUT);
    await page.close();
  } finally { await browser.close(); server.close(); }
}
main().catch((e) => { console.error(e); process.exit(1); });
