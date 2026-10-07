import '@angular/compiler';
import { describe, it, expect, beforeEach } from 'vitest';
import { TestBed } from '@angular/core/testing';
import { Title } from '@angular/platform-browser';
import { provideRouter, Route, Router, TitleStrategy, UrlSegment } from '@angular/router';
import '../test-setup';
import { appRoutes } from './app.routes';
import { AppTitleStrategy } from './app-title-strategy';
import { AuthService } from './services/auth.service';
import { matchShellRoute } from './pages/shells/shells-route';

/** Stand-in for every lazily loaded page: the router only needs a Type to resolve a route, and
 *  nothing here mounts a `RouterOutlet`, so it is never instantiated or template-compiled. */
class StubPage {}

// Two layers of coverage, deliberately:
//
//  1. Declaration assertions on the table (order, `data`, config identity). Cheap, and they name the
//     invariants a reader cannot see from one entry.
//  2. REAL `router.navigateByUrl` runs over the real `appRoutes`, which is the only thing that can
//     catch a route-ORDERING mistake — `shells-route.spec.ts` exercises the matcher as a pure
//     function against hand-built segments and would pass happily while `/shells/x` was shadowed by
//     something above it. No component is mounted or loaded (see the note on `testRoutes`), so this
//     inspects the resolved route state instead of a rendered component.

const segs = (...paths: string[]) => paths.map((p) => new UrlSegment(p, {}));

describe('appRoutes — shells declarations', () => {
  const shellsListIndex = appRoutes.findIndex((r) => r.path === 'shells');
  const shellMatcherIndex = appRoutes.findIndex((r) => r.matcher === matchShellRoute);

  it('still resolves /shells to the list page', () => {
    expect(shellsListIndex).toBeGreaterThanOrEqual(0);
    const list = appRoutes[shellsListIndex];
    expect(list.title).toBe('Shells');
    expect(list.loadComponent).toBeTypeOf('function');
    // The list is NOT the plain-shell surface — only the single-shell route carries that flag.
    expect(list.data?.['surface']).toBeUndefined();
  });

  it('declares the single-shell surface as a matcher route carrying surface=shell', () => {
    expect(shellMatcherIndex).toBeGreaterThanOrEqual(0);
    const shell = appRoutes[shellMatcherIndex];
    expect(shell.data?.['surface']).toBe('shell');
    expect(shell.title).toBe('Shell');
    expect(shell.canActivate).toHaveLength(1);
    expect(shell.loadComponent).toBeTypeOf('function');
    // A matcher route must not also declare a path, or Angular throws on config validation.
    expect(shell.path).toBeUndefined();
  });

  it('keeps the list route ahead of the matcher so /shells alone still lists', () => {
    expect(shellsListIndex).toBeLessThan(shellMatcherIndex);
  });

  it('serves the bare, /transcript and /raw URLs from that ONE config object', () => {
    // Same config => Angular reuses the component instance across a view toggle instead of
    // rebuilding the page. Three sibling configs would pass every matcher test and still regress it.
    const shell = appRoutes[shellMatcherIndex];
    for (const url of [
      segs('shells', 'abc'),
      segs('shells', 'abc', 'transcript'),
      segs('shells', 'abc', 'raw'),
    ]) {
      expect(shell.matcher!(url, null as never, shell)).not.toBeNull();
    }
  });

  it('leaves no path-based `shells/:sessionId` entry behind to shadow the matcher', () => {
    expect(appRoutes.some((r) => r.path === 'shells/:sessionId')).toBe(false);
  });
});

describe('appRoutes — real navigation over the shipped table', () => {
  let router: Router;

  // The REAL table, with only the lazy component factories swapped for a stub. Order, the shell
  // matcher, `data`, the guards, the wildcard and the title strategy are all the shipped ones, so a
  // route-ORDERING mistake fails here; the pure matcher spec cannot see ordering at all. The swap is
  // forced: resolving the real `loadComponent` imports pages that pull in `@johnnyone/ui`, Ionic and
  // xterm, which this vitest config (no Angular plugin, no tsconfig path aliases) cannot load —
  // `terminal-reconcile-integration.spec.ts` documents the same wall. `nx build web` covers that the
  // chunks import.
  const testRoutes: Route[] = appRoutes.map((r) => {
    if (!r.loadComponent) return r;
    const { loadComponent: _drop, ...rest } = r;
    return { ...rest, component: StubPage };
  });

  beforeEach(async () => {
    await TestBed.configureTestingModule({
      providers: [
        provideRouter(testRoutes),
        { provide: TitleStrategy, useClass: AppTitleStrategy },
        // Every shell route sits behind `authGuard`, which only needs this one method. A real
        // AuthService would reach for storage + the relay.
        { provide: AuthService, useValue: { syncAuthState: () => true } },
      ],
    }).compileComponents();
    router = TestBed.inject(Router);
  });

  /** The leaf route state after a navigation: params, data and which config matched. */
  function leaf() {
    let route = router.routerState.snapshot.root;
    while (route.firstChild) route = route.firstChild;
    return route;
  }

  it('/shells resolves the list route, not the shell matcher', async () => {
    expect(await router.navigateByUrl('/shells')).toBe(true);
    expect(router.url).toBe('/shells');
    // The router CLONES each declared config, so identity against `testRoutes[i]` is not available
    // — assert the distinguishing fields instead: this is the path route, not the matcher.
    expect(leaf().routeConfig?.path).toBe('shells');
    expect(leaf().routeConfig?.matcher).toBeUndefined();
    expect(leaf().data['surface']).toBeUndefined();
  });

  it('/shells/x resolves the shell surface with no view param', async () => {
    expect(await router.navigateByUrl('/shells/x')).toBe(true);
    expect(leaf().routeConfig?.matcher).toBe(matchShellRoute);
    expect(leaf().data['surface']).toBe('shell');
    expect(leaf().paramMap.get('sessionId')).toBe('x');
    expect(leaf().paramMap.get('view')).toBeNull();
  });

  it('/shells/x/raw carries the view param on the SAME config object', async () => {
    await router.navigateByUrl('/shells/x');
    const bareConfig = leaf().routeConfig;
    expect(await router.navigateByUrl('/shells/x/raw')).toBe(true);
    expect(leaf().paramMap.get('view')).toBe('raw');
    expect(leaf().paramMap.get('sessionId')).toBe('x');
    // IDENTITY across the two navigations — this is the assertion the whole matcher exists for. A
    // different config object per view is what makes Angular destroy and rebuild TerminalPage.
    expect(leaf().routeConfig).toBe(bareConfig);
  });

  it('/shells/x/transcript stays valid if someone types it', async () => {
    expect(await router.navigateByUrl('/shells/x/transcript')).toBe(true);
    expect(leaf().paramMap.get('view')).toBe('transcript');
    expect(leaf().data['surface']).toBe('shell');
  });

  it('/shells/x/Raw still reaches the shell rather than the wildcard', async () => {
    expect(await router.navigateByUrl('/shells/x/Raw')).toBe(true);
    expect(router.url).toBe('/shells/x/Raw');
    expect(leaf().data['surface']).toBe('shell');
    // Verbatim — the page case-folds it and then replaces the URL with the canonical spelling.
    expect(leaf().paramMap.get('view')).toBe('Raw');
  });

  it('/shells/x/bogus still reaches the shell (the page then canonicalises the URL)', async () => {
    expect(await router.navigateByUrl('/shells/x/bogus')).toBe(true);
    expect(leaf().data['surface']).toBe('shell');
    expect(leaf().paramMap.get('sessionId')).toBe('x');
    expect(leaf().paramMap.get('view')).toBe('bogus');
  });

  it('/shells/x/raw/extra is NOT a shell — four segments fall through to the wildcard', async () => {
    await router.navigateByUrl('/shells/x/raw/extra');
    expect(router.url).toBe('/initiatives');
  });

  it('a genuine non-route still falls through to the wildcard', async () => {
    await router.navigateByUrl('/nope');
    expect(router.url).toBe('/initiatives');
  });

  it('re-applies the static "Shell" title on EVERY shell navigation', async () => {
    // The mechanism behind the tab-title effect in TerminalPage: the title strategy runs on each
    // navigation, so a view toggle (which is now a navigation) resets the tab to "Shell" unless that
    // effect depends on `shellView()` and re-applies "Shell · <session>" in the following flush.
    const title = TestBed.inject(Title);
    await router.navigateByUrl('/shells/x');
    expect(title.getTitle()).toBe('Shell');
    title.setTitle('Shell · kord');
    await router.navigateByUrl('/shells/x/raw');
    expect(title.getTitle()).toBe('Shell');
  });
});
