// Plans tab (initiative 8ad3ae71, phase 01 task 05): pins the shared client's plans documents to the
// worker schema. Every root field must exist under the right root type (Query vs Mutation), every
// argument must be declared on that field, and every selected field — at every depth — must exist
// on its type. A renamed/removed schema field or a typo in `plans-api-documents.ts` fails here
// instead of at runtime on the phone.
import { readFileSync } from 'fs';
import { resolve } from 'path';
import { describe, expect, it } from 'vitest';
import {
  PLANS_DOCUMENTS,
  PLAN_RUN_FIELDS_PLACEHOLDER,
  plansDocument,
  type PlansOperation,
} from '../../../../../ui/src/services/plans-api-documents';

// ── a tiny schema model: type name → field name → { type, args } ──

interface FieldDef {
  type: string;
  args: Set<string>;
}
type Schema = Map<string, Map<string, FieldDef>>;

function stripComments(sdl: string): string {
  return sdl
    .split('\n')
    .map((l) => l.replace(/#.*$/, ''))
    .join('\n');
}

/** Named type of a field type expression: `[BriefCheckItem!]!` → `BriefCheckItem`. */
function namedType(expr: string): string {
  return expr.replace(/[[\]!\s]/g, '');
}

export function parseSchema(sdl: string): Schema {
  const schema: Schema = new Map();
  const src = stripComments(sdl);
  const blockRe = /\b(?:type|extend type|input)\s+(\w+)[^{]*\{([^}]*)\}/g;
  let m: RegExpExecArray | null;
  while ((m = blockRe.exec(src))) {
    const [, name, body] = m;
    const fields = schema.get(name) ?? new Map<string, FieldDef>();
    // Join multi-line argument lists, then read `name(args): Type` / `name: Type`.
    const flat = body.replace(/\(([^)]*)\)/g, (_all, a: string) => `(${a.replace(/\s+/g, ' ')})`);
    for (const line of flat.split('\n')) {
      const f = /^\s*(\w+)\s*(\(([^)]*)\))?\s*:\s*([^\s]+)/.exec(line);
      if (!f) continue;
      const args = new Set<string>();
      for (const a of (f[3] ?? '').split(',')) {
        const an = /^\s*(\w+)\s*:/.exec(a);
        if (an) args.add(an[1]);
      }
      fields.set(f[1], { type: namedType(f[4]), args });
    }
    schema.set(name, fields);
  }
  return schema;
}

// ── a tiny document walker ──

const SCALARS = new Set(['String', 'Int', 'Float', 'Boolean', 'ID']);

function tokenize(doc: string): string[] {
  return doc.match(/\$?\w+|[{}():,!\[\]]/g) ?? [];
}

/** Errors for one operation document (empty = conforms). */
export function checkDocument(schema: Schema, doc: string): string[] {
  const errors: string[] = [];
  const t = tokenize(doc);
  let i = 0;
  const kind = t[i++];
  if (kind !== 'query' && kind !== 'mutation') return [`not an operation: ${kind}`];
  const rootName = kind === 'query' ? 'Query' : 'Mutation';
  // Skip the operation name and its variable definitions up to the first `{`.
  while (i < t.length && t[i] !== '{') i++;

  const walk = (typeName: string, path: string): void => {
    i++; // consume `{`
    const fields = schema.get(typeName);
    // `?` = the parent field was already reported missing; don't report it twice.
    if (!fields && typeName !== '?') {
      errors.push(`${path}: unknown type ${typeName}`);
    }
    while (i < t.length && t[i] !== '}') {
      const name = t[i++];
      const def = fields?.get(name);
      if (fields && !def) errors.push(`${path}.${name}: not a field of ${typeName}`);
      if (t[i] === '(') {
        i++;
        while (i < t.length && t[i] !== ')') {
          const arg = t[i];
          if (/^\w+$/.test(arg) && t[i + 1] === ':') {
            if (def && !def.args.has(arg)) errors.push(`${path}.${name}(${arg}): undeclared argument`);
            i += 3; // arg : $value
          } else {
            i++;
          }
        }
        i++; // `)`
      }
      if (t[i] === '{') {
        if (def && SCALARS.has(def.type)) errors.push(`${path}.${name}: scalar ${def.type} has no selection`);
        walk(def?.type ?? '?', `${path}.${name}`);
      } else if (def && !SCALARS.has(def.type) && schema.has(def.type)) {
        errors.push(`${path}.${name}: object type ${def.type} needs a selection`);
      }
    }
    i++; // `}`
  };
  walk(rootName, rootName);
  return errors;
}

function rootFieldOf(doc: string): { kind: string; field: string } {
  const t = tokenize(doc);
  let i = 0;
  while (i < t.length && t[i] !== '{') i++;
  return { kind: t[0], field: t[i + 1] };
}

const schemaPath = resolve(__dirname, '../../../../../worker/schema/johnnyone-ai.graphql');
const schema = parseSchema(readFileSync(schemaPath, 'utf8'));
// The real AgentPlanRun selection lives in the Angular service; a representative one is enough to
// walk the AgentPlanRun-returning documents (the plan fields it adds are pinned below).
const RUN = 'plan { id initiativeId initiativeStatus title workspacePath briefingSessionId acceptedBriefVersion planningQueuedAfter updatedAt }';

const EXPECTED_ROOTS: Record<PlansOperation, 'query' | 'mutation'> = {
  listDraftPlans: 'query',
  getBriefVersion: 'query',
  listBriefVersions: 'query',
  getBriefDiff: 'query',
  getBriefCheck: 'query',
  createDraftPlan: 'mutation',
  createDraftPlanDuplicate: 'mutation',
  updateBriefField: 'mutation',
  resetBriefToVersion: 'mutation',
  addOutlinePhase: 'mutation',
  addOutlineTask: 'mutation',
  updateOutlineItem: 'mutation',
  updateOutlineItemPosition: 'mutation',
  deleteOutlineItem: 'mutation',
  updatePlanningQueue: 'mutation',
};

describe('plans API client ↔ worker schema contract', () => {
  it('covers exactly the 15 plans operations', () => {
    expect(Object.keys(PLANS_DOCUMENTS).sort()).toEqual(Object.keys(EXPECTED_ROOTS).sort());
  });

  for (const op of Object.keys(PLANS_DOCUMENTS) as PlansOperation[]) {
    it(`${op}: root field under ${EXPECTED_ROOTS[op] === 'query' ? 'Query' : 'Mutation'}, every argument and selected field exists`, () => {
      const doc = plansDocument(op, RUN);
      const { kind, field } = rootFieldOf(doc);
      expect(kind).toBe(EXPECTED_ROOTS[op]);
      expect(field).toBe(op);
      const root = schema.get(kind === 'query' ? 'Query' : 'Mutation')!;
      expect(root.has(field), `${field} missing from ${kind} root`).toBe(true);
      expect(checkDocument(schema, doc)).toEqual([]);
      expect(doc).not.toContain(PLAN_RUN_FIELDS_PLACEHOLDER);
    });
  }

  it('selects every BriefVersion field the type declares', () => {
    const declared = [...schema.get('BriefVersion')!.keys()].sort();
    const doc = PLANS_DOCUMENTS.getBriefVersion;
    for (const f of declared) expect(doc, `BriefVersion.${f} not selected`).toMatch(new RegExp(`\\b${f}\\b`));
    expect([...schema.get('BriefFields')!.keys()].sort()).toEqual(
      ['decided', 'doneWhen', 'outOfScope', 'outcome', 'where', 'why'],
    );
  });

  it('AgentPlan exposes the migration 031 columns as nullable fields', () => {
    const plan = schema.get('AgentPlan')!;
    expect(plan.get('acceptedBriefVersion')?.type).toBe('Int');
    expect(plan.get('planningQueuedAfter')?.type).toBe('String');
  });

  it('the walker is not vacuous: a misspelled field, argument or root fails', () => {
    expect(checkDocument(schema, 'query Q { listDraftPlans { id titel } }')).toEqual([
      'Query.listDraftPlans.titel: not a field of DraftPlan',
    ]);
    expect(checkDocument(schema, 'query Q($x: ID!) { getBriefCheck(planId: $x) { rule } }')).toEqual([
      'Query.getBriefCheck(planId): undeclared argument',
    ]);
    expect(checkDocument(schema, 'mutation M { listDraftPlans { id } }')).toEqual([
      'Mutation.listDraftPlans: not a field of Mutation',
    ]);
    expect(checkDocument(schema, 'query Q { getBriefVersion(initiativeId: "x") { fields } }')).toEqual([
      'Query.getBriefVersion.fields: object type BriefFields needs a selection',
    ]);
  });
});
