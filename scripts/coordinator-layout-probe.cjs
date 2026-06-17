#!/usr/bin/env node
// Verify the coordinator-body layout: desktop grid-areas (phases | lens/events |
// settings), the planning `no-phases` variant, and the mobile vertical stack.
// Rules below are copied verbatim from planner.page.scss (the parts that changed).
const fs = require('node:fs');
const path = require('node:path');
const puppeteer = require('puppeteer');
const OUT = path.join(__dirname, '..', 'e2e-artifacts', 'coordinator-layout.png');

const DESKTOP = `
.coordinator-body { min-height:0; display:grid;
  grid-template-columns:300px minmax(0,1fr) 330px;
  grid-template-rows:auto minmax(0,1fr);
  grid-template-areas:"phases lens settings" "phases events settings";
  gap:10px; padding:10px; overflow:hidden; height:340px; }
.coordinator-body.no-phases { grid-template-columns:minmax(0,1fr) 330px;
  grid-template-areas:"lens settings" "events settings"; }
.phase-list{grid-area:phases;} .lens-panel{grid-area:lens;}
.event-log{grid-area:events;} .run-settings{grid-area:settings;}
.coordinator-body > *{border:1px solid #2c3a4d;background:#0c1726;border-radius:6px;
  padding:8px;color:#c4d2e2;font:600 12px Inter;}
`;
const MOBILE = `
.m .coordinator-body, .m .coordinator-body.no-phases { min-height:0; display:flex;
  flex-direction:column; gap:10px; overflow-y:auto; height:420px; width:330px; padding:10px; }
.m .phase-list,.m .lens-panel,.m .event-log,.m .run-settings{flex:0 0 auto;}
.m .phase-list,.m .event-log,.m .run-settings{min-height:120px;}
`;

function body(noPhases, label) {
  const phases = noPhases ? '' : `<section class="phase-list">Phases</section>`;
  return `<div style="color:#8fa0b3;font:600 12px Inter;margin:4px 0">${label}</div>
   <div class="coordinator-body ${noPhases ? 'no-phases' : ''}">
     ${phases}
     <section class="lens-panel">Lens panel</section>
     <section class="event-log">Events</section>
     <section class="run-settings">Run settings</section>
   </div>`;
}

async function main() {
  const browser = await puppeteer.launch({ headless: true, args: ['--no-sandbox', '--disable-setuid-sandbox'] });
  try {
    const page = await browser.newPage();
    await page.setViewport({ width: 1080, height: 1180, deviceScaleFactor: 2 });
    const html = `<!doctype html><html><head><style>
      html,body{margin:0;background:#0d1117;padding:20px;font-family:Inter,system-ui,sans-serif;}
      ${DESKTOP}${MOBILE}</style></head><body>
      <h3 style="color:#7fc7ff;font:700 13px Inter">DESKTOP — development (with phases)</h3>
      ${body(false, 'phase-list left · lens over events (middle) · settings right')}
      <h3 style="color:#7fc7ff;font:700 13px Inter;margin-top:18px">DESKTOP — planning (no-phases)</h3>
      ${body(true, 'lens over events (full width) · settings right')}
      <h3 style="color:#7fc7ff;font:700 13px Inter;margin-top:18px">MOBILE — planning CO tab (vertical scroll stack)</h3>
      <div class="m">${body(true, 'lens · events · settings — all visible, scrolls')}</div>
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
