#!/usr/bin/env node
// Faithful test: compiles the REAL planner.page.scss and renders the REAL T2
// (reviewer) terminal-card — now hosting the 3-lens review panel — at a phone width
// so the actual @media mobile rules fire. The lens panel should fill the card,
// stack its pills, and scroll inside it.
const fs = require('node:fs');
const path = require('node:path');
const sass = require('sass');
const puppeteer = require('puppeteer');

const ROOT = path.resolve(__dirname, '..');
const SCSS = path.join(ROOT, 'web', 'src', 'app', 'pages', 'planner', 'planner.page.scss');
const OUT = path.join(ROOT, 'e2e-artifacts', 'coordinator-mobile.png');
const CSS = sass.compile(SCSS).css;

const lensPill = (n, c, v) =>
  `<div class="lens-pill ${c}"><span class="lens-dot"></span><span class="lens-name">${n}</span><span class="lens-verdict">${v}</span></div>`;
const actRow = (t, a, txt, v, vc, att, findings) => {
  const fl = (findings || []).length
    ? `<ul class="lens-act-findings">${findings.map((f) => `<li>${f}</li>`).join('')}</ul>`
    : '';
  const label = v === 'NEEDS_CHANGES' ? 'FAILED' : v;
  return `<div class="lens-act-row ${att ? 'attention' : ''}"><span class="lens-act-time">${t}</span><span class="lens-act-actor" data-actor="${a}">${a}</span><span class="lens-act-text">${v ? `<span class="lens-act-verdict ${vc}">${label}</span> ` : ''}${txt}${fl}</span></div>`;
};

const lensPanel = `
<section class="lens-panel">
  <header class="lens-panel-head">
    <span class="lens-tag">T2 · Lens review</span>
    <span class="lens-scope">Plan review — 3 lenses</span>
  </header>
  <div class="lens-pills">
    ${lensPill('Product', 'pass', 'PASS')}
    ${lensPill('QA', 'warn', 'FAILED')}
    ${lensPill('Lead', 'reviewing', 'reviewing…')}
  </div>
  <div class="lens-activity">
    ${actRow('7:07 PM', 'CO', 'Merged verdict — sent back to T1', 'NEEDS_CHANGES', 'warn', true)}
    ${actRow('7:06 PM', 'QA', 'Acceptance criteria are not testable for several phases.', 'NEEDS_CHANGES', 'warn', true, ['Phases 01–04 overviews have no explicit acceptance criteria / definition-of-done.', 'Validation strategy names no runnable commands for the backend env split.'])}
    ${actRow('7:04 PM', 'PR', 'Scope and screens-to-verify inventory are complete.', 'PASS', 'pass', false)}
    ${actRow('7:02 PM', 'CO', 'Review started — spawning Product / QA / Lead')}
    ${actRow('7:01 PM', 'CO', 'T1 planner ready for review')}
    ${Array.from({length: 24}, (_, i) => actRow('6:'+String(59-i).padStart(2,'0')+' PM', ['CO','PR','QA','LE'][i%4], 'Older event '+(i+1)+' — verifying the feed scrolls all the way to the oldest.')).join('\n    ')}
  </div>
  <footer class="lens-legend"><span><i class="dot info"></i> info</span><span><i class="dot pass"></i> pass</span><span><i class="dot warn"></i> needs-changes</span><span><i class="dot bad"></i> attention</span></footer>
</section>`;

const html = `<!doctype html><html><head><meta name="viewport" content="width=device-width">
<style>html,body{margin:0;background:#0d1117;font-family:Inter,system-ui,sans-serif;}
.host{height:720px;display:flex;flex-direction:column;}
${CSS}</style></head>
<body><div class="host">
  <main class="planner-workspace" style="flex:1;min-height:0;display:flex;">
    <section class="agent-grid active-mobile-panel" style="flex:1;min-height:0;">
      <article class="terminal-card active-mobile-panel">
        <header>
          <span class="badge">T2</span><strong>Plan Reviewer</strong>
          <span class="state">NEEDS_CHANGES</span>
        </header>
        ${lensPanel}
      </article>
    </section>
  </main>
</div></body></html>`;

async function main() {
  const browser = await puppeteer.launch({ headless: true, args: ['--no-sandbox', '--disable-setuid-sandbox'] });
  try {
    const page = await browser.newPage();
    await page.setViewport({ width: 390, height: 760, deviceScaleFactor: 2 });
    await page.setContent(html, { waitUntil: 'networkidle0' });
    await new Promise((r) => setTimeout(r, 200));
    fs.mkdirSync(path.dirname(OUT), { recursive: true });
    await page.screenshot({ path: OUT });
    const info = await page.evaluate(() => {
      const p = document.querySelector('.lens-panel');
      const pills = document.querySelector('.lens-pills');
      return p
        ? {
            panelScrolls: p.scrollHeight > p.clientHeight + 2,
            panelOverflowsX: p.scrollWidth > p.clientWidth + 2,
            pillsOverflowX: pills ? pills.scrollWidth > pills.clientWidth + 2 : null,
          }
        : null;
    });
    console.log('lens-panel:', JSON.stringify(info));
    console.log('screenshot:', OUT);
    await page.close();
  } finally { await browser.close(); }
}
main().catch((e) => { console.error(e); process.exit(1); });
