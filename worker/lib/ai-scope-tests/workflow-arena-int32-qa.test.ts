import { describe, it, expect } from 'vitest';
import { buildSchema, graphql, GraphQLInt } from 'graphql';

/**
 * QA A — the arena's `Int` fields against 32-bit overflow.
 *
 * This project has already shipped a production outage of exactly this shape: one AI session's
 * token count exceeded 2^31-1, GraphQL `Int` refused to serialise it, and because the field sat
 * under a non-null list the WHOLE `listAiSessions` query returned null — an empty terminal tab bar
 * on every device, with nothing in the UI saying why.
 *
 * The arena re-created the preconditions: `services::workflows` stores and serves `i64` for
 * `seed`, `fanOut`, `version`, `gateExit` and `gateCollected`; the host's only bounds were
 * `seed >= 0` and `fanOut >= 1`; and `johnnyone-ai.graphql` typed all five as 32-bit `Int`. QA
 * measured it worse than the original outage: `workflowRuns: [WorkflowRun!]!` is non-null at the
 * Query level too, so the null propagated past the list all the way to the RESPONSE ROOT and took
 * every unrelated field in the same document with it.
 *
 * **ROUND 2: FIXED, and these tests are inverted from the pins that documented it.** The schema now
 * types every arena numeric as `Float` (the same fix the token-count outage used), and the host
 * additionally BOUNDS what is genuinely bounded — `fanOut` by `MAX_FAN_OUT`, `seed` by `MAX_SEED`,
 * `gateExit` to 32 bits. `gateCollected` stays unbounded on the host on purpose, because refusing a
 * real gate result over an advisory count would be the wrong trade, which is precisely why it must
 * not sit on a 32-bit field: an over-range count would have arrived as `null`, and this field
 * defines `null` as "nothing was parseable" — the inverse claim, delivered silently.
 *
 * The host half is measured in
 * `services::workflows_qa_attacks::qa_a_the_host_refuses_the_integers_it_can_bound`.
 *
 * The tests are deliberately built on the REAL schema file, so they track any RE-retyping of these
 * fields back to `Int` without being edited.
 */

const schemaSources = import.meta.glob('../../schema/johnnyone-ai.graphql', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;
const schemaSource = Object.values(schemaSources)[0] ?? '';

const OVER_I32 = 2_147_483_648; // i32::MAX + 1
const MAX_SAFE = 9_007_199_254_740_991; // Number.MAX_SAFE_INTEGER

describe('QA A — the arena numeric fields and 32-bit overflow', () => {
  it('the schema types every arena counter as Float, not 32-bit Int', () => {
    expect(schemaSource, 'the schema must be inlined').toContain('type WorkflowRun {');
    // Measured, not assumed: these are the exact declarations under test.
    for (const decl of [
      'seed: Float!',
      'gateExit: Float',
      'gateCollected: Float',
      'fanOut: Float!',
      'version: Float!',
      'consecutiveRejections: Float!',
      'strikeLimit: Float!',
    ]) {
      expect(schemaSource, `${decl} must be present for this probe to mean anything`).toContain(
        decl,
      );
    }
    // And the 32-bit forms must be GONE, so a revert cannot pass this file.
    for (const gone of [
      'seed: Int!',
      'gateExit: Int\n',
      'gateCollected: Int\n',
      'fanOut: Int!',
      'version: Int!',
      'consecutiveRejections: Int!',
      'strikeLimit: Int!',
    ]) {
      expect(schemaSource, `${gone.trim()} must no longer appear`).not.toContain(gone);
    }
    // The mutation arguments too — an `Int!` argument refuses the value at input coercion.
    expect(schemaSource).toContain('gateExit: Float!');
    expect(schemaSource).toContain('fanOut: Float!');
    expect(schemaSource).toContain('seed: Float');
    // And GraphQL `Int` really is 32-bit, which is why none of these may be one.
    expect(() => GraphQLInt.serialize(OVER_I32)).toThrow();
    expect(() => GraphQLInt.serialize(MAX_SAFE)).toThrow();
    expect(GraphQLInt.serialize(2_147_483_647)).toBe(2_147_483_647);
  });

  /**
   * The outage, no longer reproducible. With `seed: Float!` the over-i32 value serialises, so the
   * list comes back whole, the response root survives, and the unrelated field in the same document
   * survives with it.
   *
   * Probed with the SAME fields a client would request, because asking for a narrower selection
   * would hide a regression here.
   */
  it('an over-i32 seed no longer nulls the workflowRuns list or the response root', async () => {
    const schema = buildSchema(schemaSource);
    const query = `{
      workflowRuns(workflowId: "w1") {
        id
        workflowId
        planId
        seed
        gateExit
        gateCollected
        seatsHeld
        fleetBusy
        startedAt
        endedAt
      }
    }`;
    const run = (seed: number) => ({
      id: 'r1',
      workflowId: 'w1',
      planId: null,
      seed,
      gateExit: 0,
      gateCollected: 7,
      seatsHeld: '{"glm":1}',
      fleetBusy: '{}',
      startedAt: '2026-10-14T00:00:00Z',
      endedAt: '2026-10-14T00:01:00Z',
    });

    // A healthy seed: the list comes back.
    const healthy = await graphql({
      schema,
      source: query,
      rootValue: { workflowRuns: () => [run(0)] },
    });
    expect(healthy.errors, 'control: a normal seed must not error').toBeUndefined();
    expect((healthy.data as any).workflowRuns).toHaveLength(1);

    // One run with an over-i32 seed, exactly as a pre-bounds host could have served it. The host
    // refuses to CREATE one now, so this is the belt to that braces: even if a row like this exists
    // (a local writer, an older row, a hand-edited database), the wire carries it.
    const overflowed = await graphql({
      schema,
      source: query,
      rootValue: { workflowRuns: () => [run(MAX_SAFE)] },
    });
    expect(overflowed.errors, 'an over-i32 seed must no longer error').toBeUndefined();
    expect(
      (overflowed.data as any)?.workflowRuns?.[0]?.seed,
      'and the value arrives intact rather than nulling anything',
    ).toBe(MAX_SAFE);

    // One large row no longer poisons the good ones alongside it.
    const mixed = await graphql({
      schema,
      source: query,
      rootValue: { workflowRuns: () => [run(0), run(OVER_I32), run(1)] },
    });
    expect(mixed.errors).toBeUndefined();
    expect(
      (mixed.data as any).workflowRuns,
      'all three runs survive, including the two healthy ones',
    ).toHaveLength(3);

    // And the collateral damage is gone: an unrelated field in the same document survives, which
    // is the half that made this worse than the listAiSessions outage.
    const collateral = await graphql({
      schema,
      source: `{ listTmuxSessions { name } workflowRuns(workflowId: "w1") { id seed } }`,
      rootValue: {
        listTmuxSessions: () => [{ name: 'johnnyone_main' }],
        workflowRuns: () => [run(OVER_I32)],
      },
    });
    expect(collateral.errors).toBeUndefined();
    expect(
      (collateral.data as any).listTmuxSessions,
      'the unrelated field is no longer lost',
    ).toHaveLength(1);
  });

  /**
   * The quietest form of the bug, and the one the ruling called out. `gateCollected` is NULLABLE, so
   * an overflowing count used to degrade to `null` on that field alone rather than nulling the row —
   * and `null` is the arena's "the gate printed nothing parseable" signal, so a huge-but-real count
   * arrived as the one value that means the OPPOSITE, with nothing reporting it.
   *
   * `Float` keeps the nullability meaningful: `null` now means only what the arena says it means.
   * This is also why `gate_collected` is the one numeric the host does NOT bound — an advisory
   * number must never cost a real score.
   */
  it('an over-i32 gateCollected survives instead of arriving as null', async () => {
    const schema = buildSchema(schemaSource);
    const result = await graphql({
      schema,
      source: `{ workflowRuns(workflowId: "w1") { id seed gateExit gateCollected } }`,
      rootValue: {
        workflowRuns: () => [
          {
            id: 'r1',
            workflowId: 'w1',
            seed: 0,
            gateExit: 0,
            gateCollected: MAX_SAFE,
            seatsHeld: '{}',
            fleetBusy: '{}',
            startedAt: 'now',
          },
        ],
      },
    });
    expect(result.errors).toBeUndefined();
    const row = (result.data as any).workflowRuns[0];
    expect(row.id).toBe('r1');
    expect(
      row.gateCollected,
      'a real count of 9007199254740991 must arrive as itself, not as the null that means ' +
        '"nothing was parseable"',
    ).toBe(MAX_SAFE);

    // And a genuine null still arrives as null: the signal keeps its meaning.
    const unparseable = await graphql({
      schema,
      source: `{ workflowRuns(workflowId: "w1") { id gateCollected } }`,
      rootValue: {
        workflowRuns: () => [{ id: 'r1', gateCollected: null }],
      },
    });
    expect(unparseable.errors).toBeUndefined();
    expect((unparseable.data as any).workflowRuns[0].gateCollected).toBeNull();
  });

  /**
   * `fanOut` and `version` on `Workflow`, served by `workflows: [Workflow!]!` and
   * `workflow(id): Workflow!`. The host now refuses a `fanOut` above `MAX_FAN_OUT`, and the field is
   * `Float!`, so the champion list cannot disappear this way from either end.
   */
  it('an over-i32 fanOut no longer nulls the workflows list', async () => {
    const schema = buildSchema(schemaSource);
    const workflow = (fanOut: number) => ({
      id: 'w1',
      repo: 'johnnyone',
      phaseType: 'dev-phase',
      family: 'glm',
      version: 1,
      parentId: null,
      role: 'r',
      instruction: 'i',
      contract: 'c',
      hop: '{}',
      fanOut,
      isChampion: true,
      createdAt: 'now',
    });
    const source = `{ workflows { id repo phaseType family version parentId role instruction contract hop fanOut isChampion createdAt } }`;

    const healthy = await graphql({
      schema,
      source,
      rootValue: { workflows: () => [workflow(3)] },
    });
    expect(healthy.errors, 'control').toBeUndefined();

    const bad = await graphql({
      schema,
      source,
      rootValue: { workflows: () => [workflow(MAX_SAFE)] },
    });
    expect(bad.errors, 'a large fanOut must no longer error on the wire').toBeUndefined();
    expect(
      (bad.data as any)?.workflows?.[0]?.fanOut,
      'the champion list survives and the value arrives intact',
    ).toBe(MAX_SAFE);
  });

  /**
   * The input side, and the deliberate trade. `startWorkflowRun(seed: Float)` ACCEPTS an over-i32
   * literal now — `Int` used to refuse it at validation, which was accidental protection on one
   * direction only. The bound has moved to where it belongs: the HOST refuses it, with a message
   * that says why, for every writer rather than only for GraphQL clients.
   *
   * That is the right place for it. `rpc_start_workflow_run` reads `seed` with `as_i64()`, so a
   * local Tauri command or a relay caller never went through the GraphQL coercion at all; relying on
   * the wire type to enforce a domain rule left every other writer unguarded. The host bound is
   * pinned in `services::workflows_qa_attacks::qa_a_the_host_refuses_the_integers_it_can_bound`.
   */
  it('a large seed now passes input coercion, and the host is what refuses it', async () => {
    const schema = buildSchema(schemaSource);
    const result = await graphql({
      schema,
      source: `mutation { startWorkflowRun(workflowId: "w1", seed: ${MAX_SAFE}) { id seed } }`,
      rootValue: { startWorkflowRun: () => ({ id: 'r1', seed: 0 }) },
    });
    expect(result.errors, 'Float coercion accepts it').toBeUndefined();
    // Which is why the schema must NOT be the only guard: the host owns the bound.
    expect(schemaSource).toContain('Bounded by the host');

    // And the round-2 API change, measured here as a side effect: `seatsHeld` / `fleetBusy` are no
    // longer arguments at all, so a browser cannot declare its own contention even if it tries.
    const declared = await graphql({
      schema,
      source: `mutation { startWorkflowRun(workflowId: "w1", seatsHeld: "{}", fleetBusy: "{}") { id seed } }`,
      rootValue: { startWorkflowRun: () => ({ id: 'r1', seed: 0 }) },
    });
    expect(declared.errors, 'the declared-budget arguments must be gone').toBeDefined();
    expect(declared.errors?.map((e) => e.message).join(' ')).toMatch(
      /Unknown argument "seatsHeld"/,
    );
    expect(declared.errors?.map((e) => e.message).join(' ')).toMatch(
      /Unknown argument "fleetBusy"/,
    );
  });
});
