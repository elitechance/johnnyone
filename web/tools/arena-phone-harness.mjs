// Drives the BUILT web app in real Chrome at a phone viewport and PROVES, per route, that the page
// rendered and that every control a human needs is reachable. It exits NON-ZERO when it is not.
//
// STANDING RISK, written here because the next person should read it before trusting the output:
// this file is the only thing proving this feature's central claim — that a human can reach the
// promote button on a phone — it is one unversioned-by-tests script of ~450 lines, and in the round
// that reviewed it, TWO OF THREE findings were defects in this harness rather than in the product
// (a metric that read 0 in a clipped layout, and a canned state the host could never produce). Treat
// a green run as evidence only after checking that the falsifications still fail: collapse the
// scroller, error every operation, point it at the wrong directory. They are listed at the bottom.
//
// CI, recommended and deliberately NOT wired: blocking for changes under `web/src/app/pages/arena/`,
// the four `*.shell.ts`, and `terminal.page.*`; advisory elsewhere. A browser job that blocks
// everything gets disabled within a month, and then this evidence evaporates exactly the way it
// nearly did in a reports folder. Wiring it is the user's call, not a developer's.
//
// Round-3 rewrite. The previous version could not fail:
//   * QA ran it verbatim against the wrong serve root and got seven screenshots and a complete
//     metrics file with no error — `bodyText: 'no'` (a 404 body) and the exact metric signature of
//     the "before" run it was being used as evidence against. There is now a RENDER ASSERTION per
//     route (a marker string that must be in the rendered text) and the process exits 1.
//   * it measured `getBoundingClientRect()` on `.aw`, which inside `contain: size layout style`
//     reports the COMPRESSED box: `arena-setup-prompt` reported 860px of content in an 860px
//     viewport and looked healthy while 198px were unreachable. It now measures `scrollHeight` and
//     `clientHeight` on the REAL scroller and reports what is unreachable after scrolling.
//   * it checked one control per page through a global `||` chain, so two routes checked nothing
//     and the arena page never examined the reject form or the halt banner. Each route now names
//     EVERY control that must be reachable.
//   * its canned data never produced a tall state: no halt, no errors, nothing clicked, and every
//     string short ASCII, so horizontal overflow was structurally always 0. It now serves a halted
//     search, three rejection reasons including a 170-character unbroken URL, errors on the
//     mutations, and it clicks the forms open before measuring.
import http from 'node:http';
import { readFile, mkdir, writeFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import path, { join } from 'node:path';
import { tmpdir } from 'node:os';
import puppeteer from 'puppeteer';

// Run it:  npx nx run web:arena-phone
//   or:    node web/tools/arena-phone-harness.mjs [serveRoot] [outDir] [label] [port] [errors]
//
// SERVE ROOT defaults to the production build output. The harness drives the BUILT app, so
// `nx build web` has to have run; pointing it at anything else is how a previous version of this
// file produced seven screenshots of a 404 body.
//
// OUT DIR defaults OUTSIDE the repository, and that is deliberate: the screenshots are read as
// local files on a phone and must never land in a diff. The reviewer's own folder is passed
// explicitly — the convention on this machine is
// `.johnnyone/reports/arena-screens`, set through ARENA_HARNESS_OUT.
const ROOT = process.argv[2] || 'dist/web/browser';
const OUT =
  process.argv[3] || process.env.ARENA_HARNESS_OUT || join(tmpdir(), 'arena-phone-harness');
const LABEL = process.argv[4] || 'after';
const PORT = Number(process.argv[5] || 8099);
/**
 * `errors` makes EVERY GraphQL operation fail. It exists to prove the marker check is not satisfied
 * by static template copy: with it, a route whose marker is a hard-coded string in the template
 * still "renders" and passes, which is how three of seven routes — the entire setup wizard — were
 * data-blind. Every marker below is now a value the route had to FETCH.
 */
const ALL_ERRORS = process.argv[6] === 'errors';
/** CSS injected into every page at runtime, to reproduce a layout defect without rebuilding. */
const INJECT_CSS = process.env.ARENA_HARNESS_INJECT_CSS || '';
const TYPES = { '.js': 'text/javascript', '.css': 'text/css', '.html': 'text/html', '.ico': 'image/x-icon', '.svg': 'image/svg+xml', '.json': 'application/json', '.woff2': 'font/woff2', '.png': 'image/png' };

// A 170-character unbroken token: the wrapping class of defect is invisible to short ASCII.
const LONG_URL =
  'https://example.internal/ci/runs/2026-10-14T07-00-00Z/artifacts/coverage/lcov-report/src/app/pages/arena/workflow-arena.page.ts.html#L1-L240?token=abcdefghijklmnop';
const REASONS = [
  `the gate never touches the cashier path — see ${LONG_URL}`,
  'it passed but the screenshot clause is still unsatisfiable on a worker with no browser',
  'coverage went down and the gate does not look at coverage',
];

const WF = { id: 'w2', repo: 'johnnyone', phaseType: 'dev-phase', family: 'glm-5.3-flash', version: 2, parentId: 'w1', role: 'You are the development worker for one phase of an approved plan.', instruction: 'Implement the phase. Follow the repo conventions. Run the gate before reporting ready.', contract: `capture with .johnnyone/capture.sh, or write in decisions.md why the screen cannot be captured — reference ${LONG_URL}`, hop: '{"steps":["read conventions first","implement"],"returns":"ready"}', fanOut: 2, isChampion: false, createdAt: '2026-10-14T07:00:00Z' };
const CHAMP = { ...WF, id: 'w1', version: 1, parentId: null, contract: 'a screenshot of each screen you changed', hop: '{"steps":["implement"]}', fanOut: 1, isChampion: true };
const RUN_PASS = { id: 'r2', workflowId: 'w2', planId: 'p1', seed: 0, gateExit: 0, gateCollected: 23, seatsHeld: '{"glm-5.3-flash":2}', fleetBusy: '{}', startedAt: '2026-10-14T07:00:00Z', endedAt: '2026-10-14T07:06:40Z' };
// SAME SEAT BUDGET as the challenger, and that is not cosmetic. `comparable` is computed on the
// host by `budgets_are_comparable`, an equality on the parsed seat maps — so a champion holding 1
// seat against a challenger holding 2 would be written `comparable: 0` and the promotion refused.
// The old fixture declared 1 against 2 AND `comparable: true`, which is a state J1 cannot reach:
// the screenshot said "the seat budgets matched" over two runs the same file said did not match.
// The layout proof was real either way; the state it was proved in was not.
const RUN_FAIL = { ...RUN_PASS, id: 'r1', workflowId: 'w1', gateExit: 101, gateCollected: null, seatsHeld: '{"glm-5.3-flash":2}', endedAt: '2026-10-14T07:08:12Z' };
// HALTED, with the long reason first: this is the tallest state the banner can be in.
const STATE_HALTED = { repo: 'johnnyone', phaseType: 'dev-phase', family: 'glm-5.3-flash', consecutiveRejections: 3, halted: true, strikeLimit: 3, recentRejections: REASONS, championWorkflowId: 'w1' };
const STATE_OPEN = { ...STATE_HALTED, consecutiveRejections: 1, halted: false, recentRejections: [REASONS[0]] };
const PREF_OK = { id: 'pf1', winnerRunId: 'r2', loserRunId: 'r1', comparable: true, reason: null, createdAt: '2026-10-14T07:10:00Z' };
const RUN_UNSCORED = { ...RUN_PASS, gateExit: null, gateCollected: null, endedAt: null };

/**
 * The canned state the server answers from. THREE scenarios, because one dataset cannot contain
 * every decisive control at once: a halted search hides nothing but blocks everything, a scored run
 * has no `score-run` control, and an unscored one can never open the promotion confirm. The
 * decisive control of the whole feature — `promote-do`, the one place the design requires a human —
 * had never been measured on a phone because the only dataset in use made its confirm unopenable.
 */
let SCENARIO = {
  name: 'halted',
  state: STATE_HALTED,
  prefs: [],
  challengerRun: RUN_PASS,
};
const PLAN = {
  plan: { id: 'p1', runType: 'development', title: 'The workflow arena', workspacePath: '/home/creepy/Documents/Workspace/personal/apps/johnnyone', planPath: '/x/plan.md', status: 'running', initiativeId: 'init-1', initiativeStatus: 'development', health: 'in-progress', currentPhaseId: '04-leases', currentPhaseIndex: 3, phaseRunMode: 'continue', workerProvider: 'claude_code', reviewerProvider: 'claude_code', testCommands: ['npm run check'], createdAt: '2026-10-14T06:00:00Z', updatedAt: '2026-10-14T07:00:00Z' },
  phases: [
    { id: 'ph1', planId: 'p1', phaseId: '03-tables', phaseTitle: 'Migration 026 — the three tables', phaseIndex: 2, status: 'done', gateVerdict: 'PASS', clarificationAttempts: 0, findingsJson: '[]', testCommands: ['cargo test --lib db::'], effectiveTestCommands: ['cargo test --lib db::'], createdAt: 'x', updatedAt: 'x' },
    { id: 'ph2', planId: 'p1', phaseId: '04-leases', phaseTitle: 'The lease pool', phaseIndex: 3, status: 'running', gateVerdict: 'PENDING', clarificationAttempts: 0, findingsJson: '[]', testCommands: [`cargo test -p johnnyone-desktop --lib leases:: -- --nocapture --test-threads=1 # ${LONG_URL}`], effectiveTestCommands: [`cargo test -p johnnyone-desktop --lib leases:: -- --nocapture --test-threads=1 # ${LONG_URL}`], createdAt: 'x', updatedAt: 'x' },
    { id: 'ph3', planId: 'p1', phaseId: '08-polish', phaseTitle: 'Plan-tab polish', phaseIndex: 7, status: 'queued', gateVerdict: 'PENDING', clarificationAttempts: 0, findingsJson: '[]', testCommands: null, effectiveTestCommands: [], createdAt: 'x', updatedAt: 'x' },
  ],
  tasks: [], events: [],
};

// Mutations ERROR, so the pages render their error blocks (raw `String(e)`, one unbroken line) —
// another thing the old canned data could never produce.
const MUTATION_ERROR = { errors: [{ message: `the host refused: workflow run start failed at ${LONG_URL}` }] };

function answer(body) {
  const name = /(?:query|mutation)\s+(\w+)/.exec(body || '')?.[1] ?? '';
  if (ALL_ERRORS) return JSON.stringify({ errors: [{ message: 'harness: every operation fails' }] });
  const isMutation = /^\s*mutation/.test(body || '');
  const d = {
    GetAgentPlan: { getAgentPlan: PLAN },
    ListAgentPlans: { listAgentPlans: [PLAN.plan] },
    Workflows: { workflows: [CHAMP, WF] },
    Workflow: { workflow: WF },
    WorkflowRuns: { workflowRuns: body.includes('w1') ? [RUN_FAIL] : [SCENARIO.challengerRun] },
    WorkflowPrefs: { workflowPrefs: SCENARIO.prefs },
    // The host derives the repo identity; the client asks. Missing from this map, the setup screen
    // showed "…" — which the data-dependent marker caught immediately, and the old static marker
    // would not have.
    RepoIdentity: { repoIdentity: 'github.com/lokalcamp/johnnyone' },
    WorkflowSearchState: { workflowSearchState: SCENARIO.state },
    RunAgentPlanTestGate: { runAgentPlanTestGate: { gateDeclared: true, exitCode: 101, collected: null, command: `cargo test -p johnnyone-desktop --lib leases:: # ${LONG_URL}`, commandsRun: 1, output: `error[E0432]: unresolved import\n  --> ${LONG_URL}\n`, truncated: true } },
  }[name];
  if (d) return JSON.stringify({ data: d });
  if (isMutation) return JSON.stringify(MUTATION_ERROR);
  return JSON.stringify({ data: {} });
}

const server = http.createServer(async (req, res) => {
  if (req.method === 'POST' || req.url.includes('/graphql')) {
    let body = '';
    req.on('data', (c) => (body += c));
    req.on('end', () => {
      res.writeHead(200, { 'content-type': 'application/json', 'access-control-allow-origin': '*' });
      res.end(answer(body));
    });
    return;
  }
  let rel = decodeURIComponent(req.url.split('?')[0]);
  let file = path.join(ROOT, rel);
  if (!existsSync(file) || rel === '/' || !path.extname(rel)) file = path.join(ROOT, 'index.html');
  try {
    const data = await readFile(file);
    res.writeHead(200, { 'content-type': TYPES[path.extname(file)] ?? 'application/octet-stream' });
    res.end(data);
  } catch {
    res.writeHead(404); res.end('no');
  }
});
await new Promise((r) => server.listen(PORT, '127.0.0.1', r));

await mkdir(OUT, { recursive: true });
const browser = await puppeteer.launch({
  executablePath: '/usr/bin/google-chrome',
  headless: 'shell',
  args: ['--no-sandbox', '--disable-dev-shm-usage', '--hide-scrollbars'],
});
const page = await browser.newPage();
await page.setViewport({ width: 400, height: 860, deviceScaleFactor: 2, isMobile: true, hasTouch: true });
await page.evaluateOnNewDocument((port) => {
  localStorage.setItem('johnnyone_worker_url', `http://127.0.0.1:${port}`);
  localStorage.setItem('johnnyone_access_token', 'devC-harness-opaque-token');
  localStorage.setItem('johnnyone_refresh_token', 'devC-harness-refresh');
  localStorage.setItem('johnnyone_tenant_id', '00000000-0000-0000-0000-000000000001');
  localStorage.setItem('johnnyone_user_id', '00000000-0000-0000-0000-000000000002');
  localStorage.setItem('johnnyone_auth_user', JSON.stringify({ id: 'u1', email: 'ethan@elitechance.com' }));
  localStorage.setItem('johnnyone_token_expires_at', String(Date.now() + 86400000));
}, PORT);

/**
 * Each route declares:
 *   marker   — text that MUST be in the rendered page. Missing ⇒ the route did not render.
 *   open     — data-test ids to click first, to reach the page's TALLEST state.
 *   controls — every control that must be reachable by scrolling. Not one; all of them.
 *   ionContent — whether this route is expected to carry Ionic's scroller.
 */
const ROUTES = [
  { name: 'arena-setup-gate', url: '/initiatives/p1/arena?phaseId=04-leases', marker: 'cargo test -p johnnyone-desktop', ionContent: true,
    open: ['run-gate'], controls: ['run-gate', 'next-gate'] },
  { name: 'arena-setup-seats', url: '/initiatives/p1/arena?phaseId=04-leases&step=seats', marker: 'The lease pool', ionContent: true,
    open: [], controls: ['seats-unmeasured', 'fan-out', 'next-seats', 'family-glm-5.3-flash', 'family-deepseek-v4.1-flash', 'family-qwen3.8-flash-next'] },
  { name: 'arena-setup-prompt', url: '/initiatives/p1/arena?phaseId=04-leases&step=prompt', marker: 'github.com/lokalcamp/johnnyone', ionContent: true,
    open: ['start-search'], controls: ['slot-role', 'slot-instruction', 'slot-contract', 'slot-hop', 'halt-block', 'start-search'] },
  // 'glm-5.3-flash' was NOT a fetched value here: the fleet strip lists the three families from a
  // constant, so the marker passed with every operation erroring. 'width 2' only exists if a
  // workflow row rendered.
  { name: 'workflows-list', url: '/workflows', marker: 'width 2', ionContent: true,
    open: [], controls: ['fleet-strip', 'wf-row-w1', 'wf-row-w2', 'wf-link-w1', 'wf-arena-w1', 'wf-link-w2', 'wf-arena-w2'] },
  { name: 'workflow-detail', url: '/workflows/w2', marker: 'capture with .johnnyone/capture.sh', ionContent: true,
    open: [], controls: ['slot-role', 'slot-instruction', 'slot-contract', 'slot-hop'] },
  { name: 'workflow-arena', url: '/workflows/w2/arena', marker: 'exit 101', ionContent: true,
    open: ['resume', 'reject', 'replay', 'rescore-run'], controls: ['halt-block', 'resume-reason', 'resume-send', 'score-commands', 'score-phase', 'rescore-confirm', 'rescore-do', 'replay', 'compare', 'promote', 'reject', 'reject-reason', 'reject-send', 'read-the-work'] },
  { name: 'console-arena-tab', url: '/initiatives?initiativeId=p1&tab=arena', marker: '04-leases', ionContent: false,
    open: [], controls: ['all-workflows', 'enter-arena', 'enter-arena-disabled', 'set-gate', 'arena-row-reason'] },
];

const ARENA = ROUTES.find((r) => r.name === 'workflow-arena');

/**
 * Each scenario names the data the server answers from and the routes to drive under it. The
 * `halted` scenario is the tall one and covers all seven routes; the other two exist to put the
 * two decisive controls on screen at all.
 */
const SCENARIOS = [
  { ...SCENARIO, routes: ROUTES },
  {
    name: 'decisive',
    state: STATE_OPEN,
    prefs: [PREF_OK],
    challengerRun: RUN_PASS,
    routes: [
      {
        ...ARENA,
        name: 'workflow-arena-decisive',
        url: ARENA.url,
        marker: 'the seat budgets matched',
        open: ['promote', 'reject'],
        // `promote-do` is the button that performs the promotion. It can only exist when the search
        // is not halted AND a comparable preference exists AND the confirm has been opened.
        controls: ['promote', 'promote-confirm', 'promote-do', 'rescore-run', 'score-phase', 'score-commands', 'replay', 'reject', 'reject-reason', 'reject-send', 'read-the-work'],
      },
    ],
  },
  {
    name: 'unscored',
    state: STATE_OPEN,
    prefs: [],
    challengerRun: RUN_UNSCORED,
    routes: [
      {
        ...ARENA,
        name: 'workflow-arena-unscored',
        url: ARENA.url,
        marker: 'never ran',
        open: [],
        // `score-run` is the executor's primary control and only exists on a run with no score.
        controls: ['score-run', 'score-phase', 'score-commands', 'compare', 'promote', 'reject', 'replay', 'read-the-work'],
      },
    ],
  },
];

const failures = [];
const report = [];

for (const scenario of SCENARIOS) {
 SCENARIO = scenario;
 for (const route of scenario.routes) {
  await page.goto(`http://127.0.0.1:${PORT}${route.url}`, { waitUntil: 'networkidle2' });
  await new Promise((r) => setTimeout(r, 1000));
  if (INJECT_CSS) {
    await page.evaluate((css) => {
      const style = document.createElement('style');
      style.textContent = css;
      document.head.appendChild(style);
    }, INJECT_CSS);
    await new Promise((r) => setTimeout(r, 300));
  }

  // 1. DID IT RENDER? Everything below is meaningless if not, and a 404 body or a blank app shell
  //    used to sail through with a full metrics file.
  const rendered = await page.evaluate(() => ({
    text: (document.body.innerText || '').trim(),
    html: document.body.innerHTML.length,
  }));
  const markerFound = rendered.text.toLowerCase().includes(route.marker.toLowerCase());
  if (rendered.text.length < 40 || !markerFound) {
    const why = rendered.text.length < 40
      ? `!! DID NOT RENDER (innerText ${rendered.text.length} chars) !!`
      : `!! WRONG PAGE (marker ${JSON.stringify(route.marker)} not in the rendered text) !!`;
    failures.push(`${route.name}: ${why} text=${JSON.stringify(rendered.text.slice(0, 120))}`);
    await page.screenshot({ path: path.join(OUT, `${LABEL}-${route.name}.png`) });
    report.push({ name: route.name, scenario: scenario.name, url: route.url, rendered: false, why, text: rendered.text.slice(0, 200) });
    continue;
  }

  // 2. Open the page's tallest state before measuring: forms that are not in the DOM cannot be
  //    found unreachable.
  for (const id of route.open) {
    await page.evaluate((sel) => {
      const el = document.querySelector(`[data-test="${sel}"]`);
      if (el) el.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    }, id);
    await new Promise((r) => setTimeout(r, 250));
  }
  await new Promise((r) => setTimeout(r, 400));

  const metrics = await page.evaluate((route) => {
    const de = document.documentElement;
    const ionContent = document.querySelector('ion-content');
    const panel = document.querySelector('app-arena-panel');
    const inner = ionContent ? ionContent.shadowRoot?.querySelector('.inner-scroll') : null;
    // THE REAL SCROLLER, in order: ion-content's inner scroll, the console panel, the document.
    const scroller = inner ?? panel ?? de;
    const scrollerName = inner ? 'ion-content .inner-scroll' : panel ? 'app-arena-panel' : 'documentElement';
    // scrollHeight/clientHeight on the scroller, NOT a bounding rect on a child inside
    // `contain: size` — that reports the compressed box and is how a clipped route looked healthy.
    // THE SCROLLER ITSELF MUST BE USER-SCROLLABLE. `scrollTop = 1e6` moves an `overflow: hidden`
    // box perfectly well, so `maxScrollTop` cannot tell a scroller from a clip — which is exactly
    // the console-tab defect this branch shipped once (`.terminal-surface { overflow: hidden }`).
    const scrollerStyle = scroller.nodeType === 1 ? getComputedStyle(scroller) : null;
    const scrollerClips = scrollerStyle
      ? /hidden|clip/.test(scrollerStyle.overflowY) && !/auto|scroll/.test(scrollerStyle.overflowY)
      : false;
    const scrollHeight = scroller.scrollHeight;
    // NO `|| innerHeight` FALLBACK. It masked a zero-height scroller: with `ion-content { height: 0 }`
    // injected, `clientHeight` was 0, the fallback made it 860, and a 3,152px page in a zero-height
    // box reported `unreachable: 0`. The degenerate cases are now measured and failed on their own:
    //   * clientHeight 0 or absurdly small — nothing is visible however much it "scrolls";
    //   * clientHeight === scrollHeight on a box that is plainly shorter than its content, which is
    //     what a collapsed container looks like from the inside;
    //   * a scroller whose painted box does not match its clientHeight (the small-but-non-zero
    //     version of the same mask).
    const clientHeight = scroller.clientHeight;
    const scrollerRect = scroller.getBoundingClientRect
      ? scroller.getBoundingClientRect()
      : { height: clientHeight, width: innerWidth };
    const paintedHeight = Math.round(scrollerRect.height);

    const reach = (id) => {
      const el = document.querySelector(`[data-test="${id}"]`);
      if (!el) return { id, present: false };
      el.scrollIntoView({ block: 'center' });
      const r = el.getBoundingClientRect();
      // GEOMETRY IS NOT ENOUGH. `opacity: 0`, `visibility: hidden` and `clip-path` all leave the
      // rect intact, so a pure-geometry check passed three of the four shapes the static guard
      // declares out of scope. `checkVisibility` is paint-level and closes all three.
      const painted =
        typeof el.checkVisibility === 'function'
          ? el.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true, contentVisibilityAuto: true })
          : true;
      const inViewport = r.top >= -1 && r.bottom <= innerHeight + 1 && r.height > 0;
      // THE FIFTH DEGENERATE SHAPE. A control can be painted, hittable and inside the viewport and
      // still be unreachable, because `scrollIntoView` scrolls an `overflow: hidden` ancestor that
      // a FINGER CANNOT. That is this codebase's own round-2 console-tab defect
      // (`.terminal-surface { height: 100%; overflow: hidden }`), and with a nested clip at 820px
      // every other check here was satisfied on its own terms: the clipped box no longer overflowed
      // the scroller, so nothing was "unreachable" by arithmetic.
      //
      // So: walk from the control up to the chosen scroller, and fail on any ancestor that CLIPS
      // (overflow hidden/clip) without being scrollable itself.
      let clippedBy = null;
      for (let node = el.parentElement; node && node !== scroller; node = node.parentElement) {
        const cs = getComputedStyle(node);
        // A box whose overflow is hidden/clip is NOT user-scrollable, by definition — that is the
        // whole distinction. (An earlier version of this loop called "has overflow" `scrolls` and
        // then skipped every box that overflowed, which is every box worth flagging.)
        const clips = /hidden|clip/.test(cs.overflowY) || /hidden|clip/.test(cs.overflowX);
        if (!clips) continue;
        const overflows =
          node.scrollHeight > node.clientHeight + 1 || node.scrollWidth > node.clientWidth + 1;
        if (overflows) {
          clippedBy = `${node.tagName.toLowerCase()}${node.className ? '.' + String(node.className).split(' ')[0] : ''}`;
          break;
        }
      }

      // CAN A FINGER ACTUALLY HIT IT? Hit-testing respects `clip-path`, which `checkVisibility`
      // does not, and it also catches anything painted on top.
      let hittable = false;
      let hitBy = null;
      if (inViewport) {
        // CENTRE ONLY, and an ANCESTOR does not count. elementFromPoint returns the deepest element
        // at the point, so getting the control or something inside it means a finger lands on it;
        // getting an ancestor means it did not. Sampling several points and accepting any of them
        // let a control clipped to 30% of its box pass, and accepting an ancestor let a transparent
        // full-screen overlay pass — both measured.
        const el2 = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
        hittable = !!el2 && (el2 === el || el.contains(el2));
        hitBy = el2 && !hittable ? `${el2.tagName.toLowerCase()}${el2.className ? '.' + String(el2.className).split(' ')[0] : ''}` : null;
      }
      return {
        id,
        present: true,
        top: Math.round(r.top),
        bottom: Math.round(r.bottom),
        painted,
        inViewport,
        hittable,
        hitBy,
        clippedBy,
        reachable: inViewport && painted && hittable && !clippedBy,
      };
    };
    const controls = route.controls.map(reach);
    // EVERY interactive element the route actually renders, by data-test id. The route's list is
    // diffed against this, so a control that exists and is not named FAILS — "13 of 13" used to
    // mean 13 of 13 NAMED, while `promote-do` (the one place the design requires a human) and
    // `score-run` (the executor's primary control) were in no list at all.
    const interactive = [...document.querySelectorAll('[data-test]')]
      .filter((el) => {
        if (el.closest('[hidden]')) return false;
        const tag = el.tagName.toLowerCase();
        if (['button', 'select', 'textarea', 'input'].includes(tag)) return true;
        if (tag === 'a' && el.hasAttribute('href')) return true;
        return el.getAttribute('role') === 'button';
      })
      .map((el) => el.getAttribute('data-test'))
      .filter(Boolean);
    scroller.scrollTop = 0;
    scroller.scrollTop = 1e6;
    const maxScrollTop = scroller.scrollTop;

    return {
      viewport: { w: innerWidth, h: innerHeight },
      scrollerName,
      scrollHeight,
      clientHeight,
      paintedHeight,
      maxScrollTop,
      // What a human can never see: content taller than the box, minus how far it scrolls.
      unreachablePx: Math.max(0, scrollHeight - clientHeight - maxScrollTop),
      hasIonContent: !!ionContent,
      scrollerClips,
      horizontalOverflow: Math.max(de.scrollWidth - innerWidth, scroller.scrollWidth - scroller.clientWidth, 0),
      controls,
      interactive: [...new Set(interactive)],
      textLength: (document.body.innerText || '').length,
    };
  }, route);

  await page.screenshot({ path: path.join(OUT, `${LABEL}-${route.name}.png`) });

  // 3. Assertions, each one a reason to exit non-zero.
  // The scroller has to BE a scroller before any number taken from it means anything.
  const MIN_SCROLLER = 200;
  if (!(metrics.clientHeight > 0)) {
    failures.push(`${route.name}: the scroller (${metrics.scrollerName}) has clientHeight ${metrics.clientHeight} — nothing on this route is visible at all, however the other numbers read`);
  } else if (metrics.clientHeight < MIN_SCROLLER) {
    failures.push(`${route.name}: the scroller (${metrics.scrollerName}) is only ${metrics.clientHeight}px tall in an ${metrics.viewport.h}px viewport — a collapsed container, not a short page`);
  } else if (metrics.clientHeight > metrics.viewport.h + 24) {
    // QA's exact mask: `clientHeight === scrollHeight` with both enormous. Nothing is "unreachable"
    // by arithmetic because the box claims to be as tall as its content — but the box is taller
    // than the SCREEN, so everything past the fold is off it with nothing to scroll.
    failures.push(`${route.name}: the scroller (${metrics.scrollerName}) is ${metrics.clientHeight}px tall in an ${metrics.viewport.h}px viewport — the box is bigger than the screen, so anything past the fold is unreachable no matter what the scroll arithmetic says`);
  } else if (Math.abs(metrics.paintedHeight - metrics.clientHeight) > 24) {
    failures.push(`${route.name}: the scroller paints ${metrics.paintedHeight}px but reports clientHeight ${metrics.clientHeight} — the box and the measurement disagree, which is how a clipped layout reports 0px unreachable`);
  } else if (metrics.scrollerClips && metrics.scrollHeight > metrics.clientHeight + 1) {
    failures.push(`${route.name}: the scroller (${metrics.scrollerName}) has ${metrics.scrollHeight}px of content in a ${metrics.clientHeight}px box and CLIPS its overflow — a script can scroll it, a finger cannot`);
  } else if (metrics.unreachablePx > 0) {
    failures.push(`${route.name}: ${metrics.unreachablePx}px of content is unreachable (scrollHeight ${metrics.scrollHeight}, clientHeight ${metrics.clientHeight}, maxScrollTop ${metrics.maxScrollTop}, scroller ${metrics.scrollerName})`);
  }
  if (metrics.horizontalOverflow > 0) {
    failures.push(`${route.name}: ${metrics.horizontalOverflow}px of HORIZONTAL overflow — something does not wrap`);
  }
  if (route.ionContent && !metrics.hasIonContent) {
    failures.push(`${route.name}: no <ion-content> — the routed shell is missing its scroller`);
  }
  for (const c of metrics.controls) {
    if (!c.present) failures.push(`${route.name}: control [data-test="${c.id}"] is not on the page`);
    else if (!c.inViewport) failures.push(`${route.name}: control [data-test="${c.id}"] cannot be scrolled into view (top ${c.top}, bottom ${c.bottom}, viewport ${metrics.viewport.h})`);
    else if (!c.painted) failures.push(`${route.name}: control [data-test="${c.id}"] is in the viewport but NOT PAINTED (opacity, visibility or content-visibility)`);
    else if (!c.hittable) failures.push(`${route.name}: control [data-test="${c.id}"] is painted but NOT HITTABLE at its centre (hit ${c.hitBy ?? 'nothing'}) — something is clipping it or sitting on top of it`);
    else if (c.clippedBy) failures.push(`${route.name}: control [data-test="${c.id}"] sits inside <${c.clippedBy}>, which CLIPS its overflow and cannot be scrolled — scrollIntoView reached it, a finger cannot`);
  }
  // A control that exists and is not named is a control nobody has proven reachable.
  const named = new Set(route.controls);
  const unchecked = metrics.interactive.filter((id) => !named.has(id));
  if (unchecked.length) {
    failures.push(`${route.name}: ${unchecked.length} interactive control(s) present but UNCHECKED — ${unchecked.join(', ')}. Name them in the route's control list or this run proves nothing about them.`);
  }

  report.push({ name: route.name, scenario: scenario.name, url: route.url, rendered: true, ...metrics });
  console.log(
    `${LABEL} ${route.name}: scroller=${metrics.scrollerName} scrollH=${metrics.scrollHeight} clientH=${metrics.clientHeight} maxScroll=${metrics.maxScrollTop} unreachable=${metrics.unreachablePx} hoverflow=${metrics.horizontalOverflow} controls=${metrics.controls.filter((c) => c.present && c.reachable).length}/${metrics.controls.length} enumerated=${metrics.interactive.length}`,
  );
 }
}

await writeFile(path.join(OUT, `${LABEL}-metrics.json`), JSON.stringify({ failures, routes: report }, null, 2));
await browser.close();
server.close();

if (failures.length) {
  console.error(`\n!! ${failures.length} FAILURE(S) !!`);
  for (const f of failures) console.error(`  - ${f}`);
  process.exit(1);
}
console.log(`\nall ${SCENARIOS.reduce((n, sc) => n + sc.routes.length, 0)} route/scenario pairs rendered, scrolled and reachable`);
