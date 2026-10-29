// Render the REAL markup against the REAL compiled stylesheet, so the look can be judged rather
// than asserted. The DOM is derived from terminal.page.html by stripping Angular control flow and
// bindings — not hand-drawn, because a hand-drawn DOM would flatter the CSS.
//
// What this proves: the component's own CSS, applied to the component's own element/class
// structure, produces (or does not produce) the mock. What it does NOT prove: that the Angular
// bindings resolve — `nx build web` and the specs cover that.
import { readFileSync, writeFileSync } from 'node:fs';
import * as sass from 'sass';

const html = readFileSync('web/src/app/pages/terminal/terminal.page.html', 'utf8');
const css = sass.compile('web/src/app/pages/terminal/terminal.page.scss', {loadPaths:['web/src','.']}).css;

const lines = html.split('\n');
function slice(startRe, endMarker) {
  const i = lines.findIndex(l => startRe.test(l));
  if (i < 0) throw new Error('not found: ' + startRe);
  let depth = 0, out = [];
  for (let j = i; j < lines.length; j++) {
    out.push(lines[j]);
    if (lines[j].includes(endMarker) && j > i) break;
  }
  return out.join('\n');
}
const settings = slice(/<section class="terminal-surface watchdog-settings"/, '</section>');
const chip = slice(/<!-- Watchdog chip/, '</div>');

// Strip Angular: control-flow lines, @let, property/event bindings, and interpolations.
const SAMPLE = {
  'wdChip?.paneSecsAgo': '41', 'wdChip?.quietSecs': '18m 22s', 'wdChip.paneSecsAgo': '41',
  'wdChip.quietSecs': '18m 22s', 'wdChip.nudgeSecsAgo': '2',
};
function deAngular(src, keepBranch) {
  return src.split('\n')
    .filter(l => !/^\s*@(let|if|else|for|switch|case|default|empty)\b/.test(l.trim()))
    .filter(l => l.trim() !== '}' && l.trim() !== '} {')
    .map(l => l.replace(/\s\[[a-zA-Z@.\-\[\]]+\]="[^"]*"/g, '')
                .replace(/\s\([a-zA-Z.]+\)="[^"]*"/g, '')
                .replace(/\s\*[a-zA-Z]+="[^"]*"/g, '')
                .replace(/\{\{([^}]*)\}\}/g, (_, e) => {
                   const k = e.trim();
                   for (const [p, v] of Object.entries(SAMPLE)) if (k.includes(p)) return v;
                   if (/formatCountdown/.test(k)) return '0:19';
                   if (/wdNudgeCount/.test(k)) return '7';
                   if (/wdArmedAgo/.test(k)) return '4h 12m';
                   if (/wdPrompt\(\)\.length/.test(k)) return '198';
                   if (/wdEnabled\(\)/.test(k)) return 'On';
                   if (/wdSaving\(\)/.test(k)) return 'Save';
                   if (/opt/.test(k)) return '20';
                   return '';
                 }))
    .join('\n');
}
const page = `<!doctype html><meta charset="utf-8"><style>
:root{--jo-accent:#e8a33d;--jo-bg:#07090d;--jo-bg-raised:#101722;--jo-line:#263241;
--jo-fg:#dbe7f3;--jo-fg-muted:#8fa0b3;--jo-fg-faint:#6b7a8d;--jo-good:#5fd78f;--jo-warn:#ffb454;--jo-bad:#ff6868;}
body{background:#07090d;color:#dbe7f3;font:13px/1.5 system-ui,sans-serif;margin:0;padding:18px;max-width:900px}
h3{color:#8fa0b3;font-size:11px;letter-spacing:.08em;text-transform:uppercase;margin:22px 0 8px}
${css}
</style>
<h3>Watchdog settings view — real markup, real stylesheet</h3>
${deAngular(settings)}
<h3>The composer chip</h3>
<div class="st-composer-wrap">${deAngular(chip)}</div>
<h3>The transcript Watchdog row</h3>
<div class="shell-transcript"><div class="st-log">
<article class="st-row you"><span class="st-ic">›</span><div class="st-bd">
<div class="st-meta"><span class="st-who">You</span><span class="st-when">13:31</span></div>
<div class="st-tx">can you check what will it need to add watchdog in J1 ?</div></div></article>
<article class="st-row wd"><span class="st-ic">⏱</span><div class="st-bd">
<div class="st-meta"><span class="st-who">Watchdog</span><span class="st-when">13:58</span></div>
<div class="st-tx">Nudged after 20m 04s of silence — "You have been quiet for 20m. Report what you are working on."</div></div></article>
<article class="st-row"><span class="st-ic">¶</span><div class="st-bd">
<div class="st-meta"><span class="st-who">Agent</span><span class="st-when">13:58</span></div>
<div class="st-tx">Still on the kart deploy verification.</div></div></article>
</div></div>`;
writeFileSync('/home/creepy/Documents/Workspace/.johnnyone/watchdog/rendered-check.html', page);
console.log('harness written; css bytes:', css.length);
