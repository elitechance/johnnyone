/**
 * DOM-free seam for the Settings → Planner prompts card.
 * No Angular, Ionic, or network — the page is the container.
 */
import type { PlannerPromptSettings } from '@johnnyone/ui';
import { SHIPPED_PROMPT_DEFAULTS } from './planner-prompt-defaults';

export type PromptKey =
  | 'planning.planner'
  | 'planning.reviewer'
  | 'planning.amendPlanner'
  | 'planning.amendReviewer'
  | 'development.worker'
  | 'development.reviewer'
  | 'development.workerNudge'
  | 'smallMode.planner'
  | 'smallMode.reviewer'
  | 'smallMode.leafWrapper'
  | 'smallMode.amendPlanner';

export type PromptValue = string | null;
export type PromptDraft = Record<PromptKey, PromptValue>;
export type LoadState = 'idle' | 'loading' | 'ready' | 'load-error';

export const PROMPT_KEYS: PromptKey[] = [
  'planning.planner',
  'planning.reviewer',
  'planning.amendPlanner',
  'planning.amendReviewer',
  'development.worker',
  'development.reviewer',
  'development.workerNudge',
  'smallMode.planner',
  'smallMode.reviewer',
  'smallMode.leafWrapper',
  'smallMode.amendPlanner',
];

export const REQUIRED_KEYS: PromptKey[] = [
  'planning.planner',
  'planning.reviewer',
  'development.worker',
  'development.reviewer',
];

export const ADDITIVE_KEYS: PromptKey[] = [
  'development.workerNudge',
  'planning.amendPlanner',
  'planning.amendReviewer',
];

export const SMALL_MODE_KEYS: PromptKey[] = [
  'smallMode.planner',
  'smallMode.reviewer',
  'smallMode.leafWrapper',
  'smallMode.amendPlanner',
];

export interface PromptRow {
  key: PromptKey;
  name: string;
  persistOnly?: boolean;
}

export const COMMERCIAL_ROWS: PromptRow[] = [
  { key: 'planning.planner', name: 'Planning planner' },
  { key: 'planning.reviewer', name: 'Planning reviewer' },
  { key: 'planning.amendPlanner', name: 'Planning amend planner' },
  { key: 'planning.amendReviewer', name: 'Planning amend reviewer' },
  { key: 'development.worker', name: 'Development worker' },
  { key: 'development.reviewer', name: 'Development reviewer' },
  { key: 'development.workerNudge', name: 'Development worker nudge', persistOnly: true },
];

export const SMALL_MODE_ROWS: PromptRow[] = [
  { key: 'smallMode.planner', name: 'Small-mode planner' },
  { key: 'smallMode.reviewer', name: 'Small-mode reviewer' },
  { key: 'smallMode.leafWrapper', name: 'Leaf wrapper' },
  { key: 'smallMode.amendPlanner', name: 'Amend planner' },
];

export function emptyDraft(): PromptDraft {
  return {
    'planning.planner': null,
    'planning.reviewer': null,
    'planning.amendPlanner': null,
    'planning.amendReviewer': null,
    'development.worker': null,
    'development.reviewer': null,
    'development.workerNudge': null,
    'smallMode.planner': null,
    'smallMode.reviewer': null,
    'smallMode.leafWrapper': null,
    'smallMode.amendPlanner': null,
  };
}

function asString(value: unknown): PromptValue {
  return typeof value === 'string' ? value : null;
}

export function fromSettings(settings: PlannerPromptSettings): {
  draft: PromptDraft;
  smallModeSupported: boolean;
  unsupportedKeys: PromptKey[];
} {
  const draft = emptyDraft();
  const unsupportedKeys: PromptKey[] = [];

  draft['planning.planner'] = asString(settings.planning?.planner);
  draft['planning.reviewer'] = asString(settings.planning?.reviewer);
  draft['planning.amendPlanner'] = asString(settings.planning?.amendPlanner);
  draft['planning.amendReviewer'] = asString(settings.planning?.amendReviewer);
  draft['development.worker'] = asString(settings.development?.worker);
  draft['development.reviewer'] = asString(settings.development?.reviewer);
  draft['development.workerNudge'] = asString(settings.development?.workerNudge);

  const smallModeSupported = settings.smallMode != null;
  if (smallModeSupported && settings.smallMode) {
    draft['smallMode.planner'] = asString(settings.smallMode.planner);
    draft['smallMode.reviewer'] = asString(settings.smallMode.reviewer);
    draft['smallMode.leafWrapper'] = asString(settings.smallMode.leafWrapper);
    draft['smallMode.amendPlanner'] = asString(settings.smallMode.amendPlanner);
  } else {
    for (const key of SMALL_MODE_KEYS) {
      draft[key] = null;
      unsupportedKeys.push(key);
    }
  }

  for (const key of ADDITIVE_KEYS) {
    if (draft[key] === null) unsupportedKeys.push(key);
  }

  return { draft, smallModeSupported, unsupportedKeys };
}

export function differsFromDefault(key: PromptKey, value: PromptValue): boolean {
  if (value === null) return false;
  const shipped = SHIPPED_PROMPT_DEFAULTS[key];
  if (shipped === undefined) return false;
  return value !== shipped;
}

export function isDirty(draft: PromptDraft, lastSaved: PromptDraft): boolean {
  return PROMPT_KEYS.some((key) => draft[key] !== lastSaved[key]);
}

export function resetKey(
  draft: PromptDraft,
  lastSaved: PromptDraft,
  key: PromptKey,
): { draft: PromptDraft; persist: PromptDraft } | null {
  if (draft[key] === null || lastSaved[key] === null) return null;
  const shipped = SHIPPED_PROMPT_DEFAULTS[key];
  if (shipped === undefined) return null;
  return {
    draft: { ...draft, [key]: shipped },
    persist: { ...lastSaved, [key]: shipped },
  };
}

export function applyMutationResult(returned: PlannerPromptSettings): PromptDraft {
  const { draft } = fromSettings(returned);
  for (const key of REQUIRED_KEYS) {
    if (typeof draft[key] !== 'string') {
      throw new Error(`Mutation return missing ${key}`);
    }
  }
  return draft;
}

export function applySaveFailure(
  draft: PromptDraft,
  lastSaved: PromptDraft,
): { draft: PromptDraft; lastSaved: PromptDraft } {
  return { draft, lastSaved };
}

export function applyResetFailure(
  draft: PromptDraft,
  lastSaved: PromptDraft,
  key: PromptKey,
): PromptDraft {
  return { ...draft, [key]: lastSaved[key] };
}

export function toUpdateInput(
  draft: PromptDraft,
  smallModeSupported: boolean,
): PlannerPromptSettings {
  const development: PlannerPromptSettings['development'] = {
    worker: draft['development.worker'] ?? '',
    reviewer: draft['development.reviewer'] ?? '',
  };
  if (typeof draft['development.workerNudge'] === 'string') {
    development.workerNudge = draft['development.workerNudge'];
  }
  const planning: PlannerPromptSettings['planning'] = {
    planner: draft['planning.planner'] ?? '',
    reviewer: draft['planning.reviewer'] ?? '',
  };
  if (typeof draft['planning.amendPlanner'] === 'string') {
    planning.amendPlanner = draft['planning.amendPlanner'];
  }
  if (typeof draft['planning.amendReviewer'] === 'string') {
    planning.amendReviewer = draft['planning.amendReviewer'];
  }
  const input: PlannerPromptSettings = { development, planning };
  if (smallModeSupported) {
    input.smallMode = {
      planner: draft['smallMode.planner'] ?? '',
      reviewer: draft['smallMode.reviewer'] ?? '',
      leafWrapper: draft['smallMode.leafWrapper'] ?? '',
      amendPlanner: draft['smallMode.amendPlanner'] ?? '',
    };
  }
  return input;
}

export function rowsForLoadState(state: LoadState): boolean {
  return state === 'ready';
}
