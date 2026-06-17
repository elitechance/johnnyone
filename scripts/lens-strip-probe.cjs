#!/usr/bin/env node
// Render the full lens-review panel (header + pills + activity feed + legend) with
// the component-scoped lens CSS (inlined here, copied verbatim from planner.page.scss)
// and screenshot it — to verify it follows the lens-activity mock before deploying.
const fs = require('node:fs');
const path = require('node:path');
const puppeteer = require('puppeteer');

const OUT = path.join(__dirname, '..', 'e2e-artifacts', 'lens-strip.png');
const SCSS = fs.readFileSync(
  path.join(__dirname, '..', 'web', 'src', 'app', 'pages', 'planner', 'planner.page.scss'),
  'utf8',
);

// Extract just the lens-* rules from the SCSS, then compile with sass.
function extractLens(scss) {
  const lines = scss.split('\n');
  const out = [];
  let depth = 0;
  let capturing = false;
  for (const line of lines) {
    if (!capturing && depth === 0 && /^\.lens-(panel|pills|pill|activity|act-|legend)/.test(line.trim())) {
      capturing = true;
    }
    if (capturing) {
      out.push(line);
      depth += (line.match(/{/g) || []).length - (line.match(/}/g) || []).length;
      if (depth <= 0) {
        capturing = false;
        depth = 0;
      }
    }
  }
  return out.join('\n');
}

let CSS;
try {
  const sass = require('sass');
  CSS = sass.compileString(extractLens(SCSS)).css;
} catch (e) {
  console.error('sass compile failed, falling back to raw (may look off):', e.message);
  CSS = extractLens(SCSS);
}

function pill(name, cls, verdict) {
  return `<div class="lens-pill ${cls}"><span class="lens-dot"></span><span class="lens-name">${name}</span><span class="lens-verdict">${verdict}</span></div>`;
}
function row(time, actor, text, { verdict, vcls, attention } = {}) {
  const v = verdict ? `<span class="lens-act-verdict ${vcls}">${verdict}</span> ` : '';
  return `<div class="lens-act-row ${attention ? 'attention' : ''}"><span class="lens-act-time">${time}</span><span class="lens-act-actor" data-actor="${actor}">${actor}</span><span class="lens-act-text">${v}${text}</span></div>`;
}

async function main() {
  const browser = await puppeteer.launch({ headless: true, args: ['--no-sandbox', '--disable-setuid-sandbox'] });
  try {
    const page = await browser.newPage();
    await page.setViewport({ width: 560, height: 560, deviceScaleFactor: 2 });
    const html = `<!doctype html><html><head><style>
      html,body{margin:0;background:#0d1117;padding:24px;font-family:Inter,system-ui,sans-serif;}
      ${CSS}</style></head><body>
      <section class="lens-panel">
        <header class="lens-panel-head">
          <span class="lens-tag">T2 · Lens review</span>
          <span class="lens-scope">Plan review — 3 lenses</span>
        </header>
        <div class="lens-pills">
          ${pill('Product', 'pass', 'PASS')}
          ${pill('QA', 'warn', 'NEEDS_CHANGES')}
          ${pill('Lead', 'bad', 'BLOCKED')}
        </div>
        <div class="lens-activity">
          ${row('7:02 PM', 'CO', 'Review started — spawning Product / QA / Lead')}
          ${row('7:02 PM', 'PR', 'Review started')}
          ${row('7:02 PM', 'QA', 'Review started')}
          ${row('7:02 PM', 'LE', 'Review started')}
          ${row('7:04 PM', 'PR', 'reported its verdict', { verdict: 'PASS', vcls: 'pass' })}
          ${row('7:05 PM', 'LE', 'reported its verdict', { verdict: 'BLOCKED', vcls: 'bad', attention: true })}
          ${row('7:06 PM', 'QA', 'reported its verdict', { verdict: 'NEEDS_CHANGES', vcls: 'warn', attention: true })}
          ${row('7:06 PM', 'CO', 'Merged verdict — sent back to T1', { verdict: 'NEEDS_CHANGES', vcls: 'warn', attention: true })}
        </div>
        <footer class="lens-legend">
          <span><i class="dot info"></i> info</span>
          <span><i class="dot pass"></i> pass</span>
          <span><i class="dot warn"></i> needs-changes</span>
          <span><i class="dot bad"></i> attention / blocked</span>
        </footer>
      </section>
      </body></html>`;
    await page.setContent(html, { waitUntil: 'networkidle0' });
    await new Promise((r) => setTimeout(r, 200));
    fs.mkdirSync(path.dirname(OUT), { recursive: true });
    await page.screenshot({ path: OUT, fullPage: true });
    console.log('screenshot:', OUT);
    await page.close();
  } finally { await browser.close(); }
}
main().catch((e) => { console.error(e); process.exit(1); });
