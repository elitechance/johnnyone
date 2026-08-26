import { describe, it, expect, beforeEach } from 'vitest';
import { TestBed } from '@angular/core/testing';
import { provideRouter, Router } from '@angular/router';
import '../../../test-setup';

describe('prompt-library editor URL construction', () => {
  beforeEach(async () => {
    await TestBed.configureTestingModule({
      providers: [provideRouter([])],
    }).compileComponents();
  });

  it('queryParams on /settings keep prompt; a ? inside the command does not', () => {
    const router = TestBed.inject(Router);
    const stuffed = router.createUrlTree(['/settings?prompt=planning.planner']);
    expect(router.serializeUrl(stuffed)).toBe('/settings%3Fprompt%3Dplanning.planner');
    expect(stuffed.queryParams['prompt']).toBeUndefined();

    const tree = router.createUrlTree(['/settings'], {
      queryParams: { prompt: 'planning.planner' },
    });
    expect(tree.queryParams['prompt']).toBe('planning.planner');
    expect(router.serializeUrl(tree)).toBe('/settings?prompt=planning.planner');
  });
});
