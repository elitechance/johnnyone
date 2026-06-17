#!/usr/bin/env node
// Faithful test: compiles the REAL planner.page.scss and renders the new run-stage
// stepper + header stage badges + T2 placeholder across the planning lifecycle, so
// we can verify the observability reads clearly before deploying.
const fs = require('node:fs');
const path = require('node:path');
const sass = require('sass');
const puppeteer = require('puppeteer');

const ROOT = path.resolve(__dirname, '..');
const SCSS = path.join(ROOT, 'web', 'src', 'app', 'pages', 'planner', 'planner.page.scss');
const OUT = path.join(ROOT, 'e2e-artifacts', 'stage-observability.png');
const CSS = sass.compile(SCSS).css;

const STEPS = ['Draft', 'Review', 'Approved'];
function stage(tone, activeIndex, title, detail) {
  const steps = STEPS.map((label, i) =>
    `<span class="run-stage-step ${i === activeIndex ? 'active' : ''} ${i < activeIndex ? 'done' : ''}">${label}</span>${i < STEPS.length - 1 ? '<span class="run-stage-sep">›</span>' : ''}`,
  ).join('');
  return `<section class="run-stage" data-tone="${tone}">
    <div class="run-stage-steps">${steps}</div>
    <div class="run-stage-now"><strong>${title}</strong><span>${detail}</span></div>
  </section>`;
}
function t2empty(tone, title, detail) {
  return `<div class="t2-empty" data-tone="${tone}"><span class="t2-empty-tag">T2 · Lens review</span><strong>${title}</strong><p>${detail}</p></div>`;
}
function badge(tone, text) {
  return `<span class="state stage" data-tone="${tone}">${text}</span>`;
}

const html = `<!doctype html><html><head><style>
html,body{margin:0;background:#0d1117;padding:24px;font-family:Inter,system-ui,sans-serif;}
.lbl{color:#7fc7ff;font:700 12px Inter;margin:16px 0 6px;}
.box{border:1px solid #263241;background:#070f1a;border-radius:8px;max-width:560px;overflow:hidden;margin-bottom:6px;}
.hdr{display:flex;align-items:center;gap:10px;padding:8px 12px;border-bottom:1px solid #1c2838;}
.hdr .badge{font:700 11px Inter;color:#7fc7ff;border:1px solid #2a4a6b;background:#0c1d2e;border-radius:4px;padding:2px 7px;}
.hdr strong{color:#d4e0ee;font:600 13px Inter;margin-right:auto;}
${CSS}</style></head><body>
  <div class="lbl">Stage badges (shown in every panel header — T1 / T2 / CO)</div>
  <div style="display:flex;gap:8px;flex-wrap:wrap;">
    ${badge('active', 'Drafting')} ${badge('active', 'Reviewing')} ${badge('done', 'Approved')} ${badge('attention', 'Attention')} ${badge('attention', 'Blocked')}
  </div>

  <div class="lbl">CO stage stepper — drafting</div>
  <div class="box">${stage('active', 0, 'T1 drafting the plan', 'The planner is writing the plan files. The 3-lens T2 review starts when the draft is ready.')}</div>

  <div class="lbl">CO stage stepper — reviewing (lens summary)</div>
  <div class="box">${stage('active', 1, 'T2 reviewing — 3 lenses', 'Product PASS · QA reviewing… · Lead reviewing…')}</div>

  <div class="lbl">CO stage stepper — approved</div>
  <div class="box">${stage('done', 2, 'Plan approved', 'The plan passed T2 review.')}</div>

  <div class="lbl">CO stage stepper — needs attention</div>
  <div class="box">${stage('attention', 1, 'Needs your attention', 'T2 did not return a parseable verdict after 5 clarification attempts')}</div>

  <div class="lbl">T2 panel placeholder — before review starts</div>
  <div class="box" style="height:150px;display:grid;grid-template-rows:38px 1fr;">
    <div class="hdr"><span class="badge">T2</span><strong>Plan Reviewer</strong>${badge('active', 'Drafting')}</div>
    ${t2empty('active', 'T1 drafting the plan', 'Review has not started — T1 is still working. The 3-lens review (Product / QA / Lead) will appear here once T2 begins.')}
  </div>
</body></html>`;

async function main() {
  const browser = await puppeteer.launch({ headless: true, args: ['--no-sandbox', '--disable-setuid-sandbox'] });
  try {
    const page = await browser.newPage();
    await page.setViewport({ width: 620, height: 1000, deviceScaleFactor: 2 });
    await page.setContent(html, { waitUntil: 'networkidle0' });
    await new Promise((r) => setTimeout(r, 200));
    fs.mkdirSync(path.dirname(OUT), { recursive: true });
    await page.screenshot({ path: OUT, fullPage: true });
    console.log('screenshot:', OUT);
    await page.close();
  } finally { await browser.close(); }
}
main().catch((e) => { console.error(e); process.exit(1); });
