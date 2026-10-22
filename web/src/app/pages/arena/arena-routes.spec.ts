// The four new routes, asserted on the REAL `appRoutes` table and with real `navigateByUrl` runs,
// which is the only thing that catches an ordering mistake (the `app.routes.spec.ts` precedent).
// No component is mounted: a stub Type is enough for the router to resolve a config.
import '@angular/compiler';
import { describe, it, expect, beforeEach } from 'vitest';
import { TestBed } from '@angular/core/testing';
import { provideRouter, Route, Router } from '@angular/router';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import '../../../test-setup';
import { appRoutes } from '../../app.routes';
import { AuthService } from '../../services/auth.service';

class StubPage {}

/** The real table with every lazy `loadComponent` swapped for a stub, exactly as the shipped
 *  route spec does, so ORDER and guards are the real ones. */
const testRoutes: Route[] = appRoutes.map((r) =>
  r.loadComponent ? { ...r, loadComponent: undefined, component: StubPage } : r,
);

describe('appRoutes — the arena routes', () => {
  beforeEach(() => {
    TestBed.resetTestingModule();
    TestBed.configureTestingModule({
      providers: [
        provideRouter(testRoutes),
        { provide: AuthService, useValue: { syncAuthState: () => true, isAuthenticated: () => true } },
      ],
    });
  });

  const find = (path: string) => appRoutes.find((r) => r.path === path);

  // The route table is EAGER. Anything it imports by value — a `providers: [X]` entry, say — ships
  // in the initial bundle, which is already ~1 MB over its budget, so the arena's service is
  // provided by the (lazy) shell components instead. This asserts the table stays free of it.
  it('imports no arena code by value — the table is in the initial bundle', () => {
    const src = readFileSync(join(process.cwd(), 'web/src/app/app.routes.ts'), 'utf8');
    const imports = src.slice(0, src.indexOf('export const appRoutes'));
    expect(imports).not.toMatch(/^import .*pages\/arena/m);
    for (const r of appRoutes) {
      expect(r.providers, `${r.path} declares route providers`).toBeUndefined();
    }
  });

  // REWRITTEN in round 3. The first version had three holes QA found: `toContain('m.ArenaSetupShell')`
  // is a substring over the WHOLE file, so a comment satisfied it; the negative regex required a
  // trailing single quote, so `import("…/arena-setup.page")` with double quotes evaded it; and
  // nothing asserted a shell actually contains `ion-content` or still declares
  // `providers: [ArenaApiService]` — a shell that lost either passed every test, and a dropped
  // provider shows up only as a runtime injector error on a page nothing can mount.
  it('loads the ion-content SHELL for each arena route, not the bare page body', () => {
    const src = readFileSync(join(process.cwd(), 'web/src/app/app.routes.ts'), 'utf8');
    // Comments cannot satisfy this: the match is the route's own loadComponent expression.
    const loaders = [...src.matchAll(/path:\s*['"]([^'"]+)['"][\s\S]*?import\(\s*['"]([^'"]+)['"]\s*\)\.then\(\s*\(m\)\s*=>\s*m\.(\w+)/g)]
      .map((m) => ({ path: m[1], specifier: m[2], symbol: m[3] }));
    for (const [path, specifier, symbol] of [
      ['initiatives/:id/arena', './pages/arena/arena-setup.shell', 'ArenaSetupShell'],
      ['workflows', './pages/arena/workflows.shell', 'WorkflowsShell'],
      ['workflows/:id/arena', './pages/arena/workflow-arena.shell', 'WorkflowArenaShell'],
      ['workflows/:id', './pages/arena/workflow-detail.shell', 'WorkflowDetailShell'],
    ] as const) {
      const loader = loaders.find((l) => l.path === path);
      expect(loader, `no loadComponent parsed for ${path}`).toBeTruthy();
      expect(loader!.specifier).toBe(specifier);
      expect(loader!.symbol).toBe(symbol);
    }
    // A routed page WITHOUT ion-content is clipped and unscrollable — measured in Chrome at
    // 400x860, with the promote control 464px past the clip. Quote-agnostic.
    for (const l of loaders) {
      expect(l.specifier, `${l.path} loads a page body directly`).not.toMatch(
        /arena\/(?:arena-setup|workflows|workflow-detail|workflow-arena)\.page$/,
      );
    }
  });

  it('every shell really supplies ion-content and really provides the arena wire', () => {
    // These four files are the layer no unit test can mount (importing Ionic collects zero tests
    // under this vitest config), and they are the layer the whole scroll fix lives in. Asserted as
    // source here, and exercised for real by the browser harness, which now fails the build if a
    // route does not render or does not scroll.
    for (const file of [
      'arena-setup.shell.ts',
      'workflows.shell.ts',
      'workflow-detail.shell.ts',
      'workflow-arena.shell.ts',
    ]) {
      const src = readFileSync(join(process.cwd(), 'web/src/app/pages/arena', file), 'utf8');
      const template = /template:\s*`([^`]*)`/.exec(src)?.[1] ?? '';
      expect(template, `${file} has no <ion-content> in its template`).toContain('<ion-content');
      expect(template, `${file} does not close its <ion-content>`).toContain('</ion-content>');
      // The body must be INSIDE the scroller, not beside it.
      expect(
        /<ion-content[^>]*>\s*<app-[a-z-]+\s*\/>\s*<\/ion-content>/.test(template),
        `${file} does not place its page body inside the ion-content`,
      ).toBe(true);
      expect(src, `${file} imports IonContent`).toMatch(/import\s*\{[^}]*\bIonContent\b[^}]*\}\s*from\s*'@ionic\/angular\/standalone'/);
      expect(src, `${file} no longer provides ArenaApiService`).toMatch(
        /providers:\s*\[\s*ArenaApiService\s*\]/,
      );
      expect(src, `${file} does not import ArenaApiService`).toMatch(
        /import\s*\{\s*ArenaApiService\s*\}\s*from\s*'\.\/arena-api\.service'/,
      );
    }
  });

  it('declares all four, lazily and behind the auth guard', () => {
    for (const path of ['initiatives/:id/arena', 'workflows', 'workflows/:id', 'workflows/:id/arena']) {
      const r = find(path);
      expect(r, `missing route ${path}`).toBeTruthy();
      expect(r!.loadComponent, `${path} should be lazy`).toBeTypeOf('function');
      expect(r!.canActivate, `${path} should be guarded`).toHaveLength(1);
      expect(r!.title).toBeTruthy();
    }
  });

  it('keeps the arena setup from shadowing the validation page under /initiatives/:id', () => {
    expect(find('initiatives/:id/validation')).toBeTruthy();
    expect(find('initiatives/:id/arena')).toBeTruthy();
  });

  it('resolves each URL to its own config, with the wildcard not swallowing them', async () => {
    const router = TestBed.inject(Router);
    for (const [url, path] of [
      ['/initiatives/abc/arena', 'initiatives/:id/arena'],
      ['/workflows', 'workflows'],
      ['/workflows/w2', 'workflows/:id'],
      ['/workflows/w2/arena', 'workflows/:id/arena'],
    ] as const) {
      expect(await router.navigateByUrl(url), `navigating ${url}`).toBe(true);
      expect(router.url).toBe(url);
      const matched = router.routerState.snapshot.root.firstChild;
      expect(matched?.routeConfig?.path, `${url} resolved to the wrong config`).toBe(path);
    }
  });

  it('carries the phase through as a query param, which survives the navigation', async () => {
    const router = TestBed.inject(Router);
    expect(await router.navigateByUrl('/initiatives/abc/arena?phaseId=04-leases&step=prompt')).toBe(true);
    const snap = router.routerState.snapshot.root.firstChild;
    expect(snap?.paramMap.get('id')).toBe('abc');
    expect(snap?.queryParamMap.get('phaseId')).toBe('04-leases');
    expect(snap?.queryParamMap.get('step')).toBe('prompt');
  });

  it('leaves every route that existed before it untouched', async () => {
    const router = TestBed.inject(Router);
    for (const url of ['/initiatives', '/files', '/shells', '/initiatives/abc/validation', '/settings']) {
      expect(await router.navigateByUrl(url), `navigating ${url}`).toBe(true);
      expect(router.url).toBe(url);
    }
  });
});
