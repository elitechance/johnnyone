import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import type { PlannerPromptSettings } from '@johnnyone/ui';
import { SHIPPED_PROMPT_DEFAULTS } from './planner-prompt-defaults';
import {
  ADDITIVE_KEYS,
  applyMutationResult,
  applyResetFailure,
  applySaveFailure,
  differsFromDefault,
  fromSettings,
  isDirty,
  resetKey,
  rowsForLoadState,
  toUpdateInput,
  type PromptDraft,
  type PromptKey,
} from './settings-prompts-logic';

function fullSettings(overrides: Partial<{
  workerNudge: string | null;
  amendPlanner: string | null;
  amendReviewer: string | null;
  smallMode: PlannerPromptSettings['smallMode'] | null;
}> = {}): PlannerPromptSettings {
  return {
    schema: 'johnnyone-planner-prompts/v1',
    development: {
      worker: 'W',
      reviewer: 'R',
      workerNudge: overrides.workerNudge === undefined ? 'WNUDGE' : overrides.workerNudge ?? undefined,
    },
    planning: {
      planner: 'P',
      reviewer: 'V',
      amendPlanner: overrides.amendPlanner === undefined ? 'AMEND-P' : overrides.amendPlanner ?? undefined,
      amendReviewer: overrides.amendReviewer === undefined ? 'AMEND-R' : overrides.amendReviewer ?? undefined,
    },
    smallMode:
      overrides.smallMode === null
        ? undefined
        : overrides.smallMode ?? {
            planner: 'SM-P',
            reviewer: 'SM-R',
            leafWrapper: 'SM-L',
            amendPlanner: 'SM-AMEND-P',
          },
  };
}

describe('settings-prompts-logic', () => {
  it('has no Angular / Ionic import', () => {
    const src = readFileSync(
      resolve(dirname(fileURLToPath(import.meta.url)), 'settings-prompts-logic.ts'),
      'utf8',
    );
    const imports = src.split('\n').filter((line) => /^\s*import\b/.test(line)).join('\n');
    expect(imports).not.toMatch(/@angular\/|@ionic\//);
  });

  it('differsFromDefault is true only when the string is not identical to the constant', () => {
    const key: PromptKey = 'smallMode.planner';
    const def = SHIPPED_PROMPT_DEFAULTS[key];
    expect(differsFromDefault(key, def)).toBe(false);
    expect(differsFromDefault(key, def + 'x')).toBe(true);
    expect(differsFromDefault(key, null)).toBe(false);
    expect(differsFromDefault('development.workerNudge', 'anything')).toBe(false);
  });

  it("resetKey('smallMode.planner') persist has the default planner and the other ten strings equal to lastSaved", () => {
    const lastSaved: PromptDraft = {
      'planning.planner': 'P',
      'planning.reviewer': 'V',
      'planning.amendPlanner': 'CUSTOM-AMEND-P',
      'planning.amendReviewer': 'AR',
      'development.worker': 'W',
      'development.reviewer': 'R',
      'development.workerNudge': 'CUSTOM-NUDGE',
      'smallMode.planner': 'CUSTOM-SM-P',
      'smallMode.reviewer': 'SM-R',
      'smallMode.leafWrapper': 'SM-L',
      'smallMode.amendPlanner': 'SM-AMEND-P',
    };
    const draft: PromptDraft = {
      ...lastSaved,
      'smallMode.planner': 'DIRTY-SM-P',
      'planning.planner': 'UNSAVED-P',
    };
    const result = resetKey(draft, lastSaved, 'smallMode.planner');
    expect(result).not.toBeNull();
    const persist = result!.persist;
    expect(persist['smallMode.planner']).toBe(SHIPPED_PROMPT_DEFAULTS['smallMode.planner']);
    expect(persist['planning.planner']).toBe('P');
    expect(persist['development.workerNudge']).toBe('CUSTOM-NUDGE');
    expect(persist['planning.amendPlanner']).toBe('CUSTOM-AMEND-P');
    expect(result!.draft['smallMode.planner']).toBe(SHIPPED_PROMPT_DEFAULTS['smallMode.planner']);
    expect(result!.draft['planning.planner']).toBe('UNSAVED-P');
  });

  it('toUpdateInput includes workerNudge / amend pair only when those draft values are strings', () => {
    const { draft } = fromSettings(fullSettings());
    const all = toUpdateInput(draft, true);
    expect(all.development.workerNudge).toBe('WNUDGE');
    expect(all.planning.amendPlanner).toBe('AMEND-P');
    expect(all.planning.amendReviewer).toBe('AMEND-R');

    const nulled: PromptDraft = {
      ...draft,
      'development.workerNudge': null,
      'planning.amendPlanner': null,
      'planning.amendReviewer': null,
    };
    const omitted = toUpdateInput(nulled, true);
    expect(omitted.development.workerNudge).toBeUndefined();
    expect(omitted.planning.amendPlanner).toBeUndefined();
    expect(omitted.planning.amendReviewer).toBeUndefined();
  });

  it.each(ADDITIVE_KEYS)(
    'GET null %s is unsupported, draft null, no Default, omitted from update',
    (key) => {
      const overrides =
        key === 'development.workerNudge'
          ? { workerNudge: null }
          : key === 'planning.amendPlanner'
            ? { amendPlanner: null }
            : { amendReviewer: null };
      const { draft, unsupportedKeys } = fromSettings(fullSettings(overrides));
      expect(unsupportedKeys).toContain(key);
      expect(draft[key]).toBeNull();
      expect(draft[key]).not.toBe(SHIPPED_PROMPT_DEFAULTS[key]);
      expect(differsFromDefault(key, draft[key])).toBe(false);
      const input = toUpdateInput(draft, true);
      if (key === 'development.workerNudge') {
        expect(input.development.workerNudge).toBeUndefined();
      } else if (key === 'planning.amendPlanner') {
        expect(input.planning.amendPlanner).toBeUndefined();
      } else {
        expect(input.planning.amendReviewer).toBeUndefined();
      }
    },
  );

  it('fromSettings({ smallMode: null }) → smallModeSupported === false and does not throw', () => {
    const { draft, smallModeSupported, unsupportedKeys } = fromSettings(
      fullSettings({ smallMode: null }),
    );
    expect(smallModeSupported).toBe(false);
    expect(unsupportedKeys).toEqual(expect.arrayContaining([
      'smallMode.planner',
      'smallMode.reviewer',
      'smallMode.leafWrapper',
      'smallMode.amendPlanner',
    ]));
    expect(draft['smallMode.planner']).toBeNull();
  });

  it('toUpdateInput(..., false) has no smallMode key', () => {
    const { draft } = fromSettings(fullSettings());
    const input = toUpdateInput(draft, false);
    expect(input.smallMode).toBeUndefined();
  });

  it('toUpdateInput(..., true) with all smallMode strings present sends all four', () => {
    const { draft } = fromSettings(fullSettings());
    const input = toUpdateInput(draft, true);
    expect(input.smallMode).toEqual({
      planner: 'SM-P',
      reviewer: 'SM-R',
      leafWrapper: 'SM-L',
      amendPlanner: 'SM-AMEND-P',
    });
  });

  it('applyMutationResult missing development.worker does not return a saved draft', () => {
    const returned = fullSettings();
    delete (returned.development as { worker?: string }).worker;
    expect(() => applyMutationResult(returned)).toThrow(/development\.worker/);
  });

  it('applyMutationResult with workerNudge: null does not treat the save as failed', () => {
    const returned = fullSettings({ workerNudge: null });
    const draft = applyMutationResult(returned);
    expect(draft['development.worker']).toBe('W');
    expect(draft['development.workerNudge']).toBeNull();
  });

  it('applySaveFailure leaves draft and lastSaved unchanged; a Custom draft still differsFromDefault', () => {
    const lastSaved = fromSettings(fullSettings()).draft;
    const draft: PromptDraft = {
      ...lastSaved,
      'smallMode.planner': (lastSaved['smallMode.planner'] as string) + 'x',
    };
    const result = applySaveFailure(draft, lastSaved);
    expect(result.draft).toBe(draft);
    expect(result.lastSaved).toBe(lastSaved);
    expect(differsFromDefault('smallMode.planner', result.draft['smallMode.planner'])).toBe(true);
  });

  it('applyResetFailure restores lastSaved[key], not the shipped default', () => {
    const lastSaved = fromSettings(fullSettings()).draft;
    const draft: PromptDraft = {
      ...lastSaved,
      'smallMode.planner': SHIPPED_PROMPT_DEFAULTS['smallMode.planner'],
    };
    const restored = applyResetFailure(draft, lastSaved, 'smallMode.planner');
    expect(restored['smallMode.planner']).toBe(lastSaved['smallMode.planner']);
    expect(restored['smallMode.planner']).not.toBe(SHIPPED_PROMPT_DEFAULTS['smallMode.planner']);
  });

  it('isDirty is false when every field equals lastSaved, even if they differ from default', () => {
    const lastSaved = fromSettings(fullSettings()).draft;
    expect(isDirty(lastSaved, lastSaved)).toBe(false);
    expect(differsFromDefault('planning.planner', lastSaved['planning.planner'])).toBe(true);
  });

  it("load state 'loading' yields no rows", () => {
    expect(rowsForLoadState('loading')).toBe(false);
    expect(rowsForLoadState('load-error')).toBe(false);
    expect(rowsForLoadState('ready')).toBe(true);
  });
});
