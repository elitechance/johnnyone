// The four new routes, asserted on the REAL `appRoutes` table and with real `navigateByUrl` runs,
// which is the only thing that catches an ordering mistake (the `app.routes.spec.ts` precedent).
// No component is mounted: a stub Type is enough for the router to resolve a config.
import '@angular/compiler';
import { describe, it, expect, beforeEach } from 'vitest';
import { TestBed } from '@angular/core/testing';
import { provideRouter, Route, Router } from '@angular/router';
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
