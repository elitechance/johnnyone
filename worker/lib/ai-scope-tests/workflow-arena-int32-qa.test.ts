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
 * The arena re-creates the preconditions. `services::workflows` stores and serves `i64` for
 * `seed`, `fanOut`, `version`, `gateExit` and `gateCollected`; the host's only bounds are
 * `seed >= 0` and `fanOut >= 1`; and `johnnyone-ai.graphql` types all five as 32-bit `Int`. The
 * host half is measured in `services::workflows_qa_attacks::qa_a_the_host_accepts_and_serves_integers_above_i32_max`.
 * This is the GraphQL half: what the client actually receives.
 *
 * The test is deliberately built on the REAL schema file, so it tracks any retyping of these
 * fields (to `Float!`, which is what the token-count fix used) without being edited.
 */

const schemaSources = import.meta.glob('../../schema/johnnyone-ai.graphql', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;
const schemaSource = Object.values(schemaSources)[0] ?? '';

const OVER_I32 = 2_147_483_648; // i32::MAX + 1
const MAX_SAFE = 9_007_199_254_740_991; // Number.MAX_SAFE_INTEGER

describe('QA A — the arena Int fields and 32-bit overflow', () => {
  it('the schema really types the arena counters as 32-bit Int', () => {
    expect(schemaSource, 'the schema must be inlined').toContain('type WorkflowRun {');
    // Measured, not assumed: these are the exact declarations under test.
    for (const decl of [
      'seed: Int!',
      'gateExit: Int',
      'gateCollected: Int',
      'fanOut: Int!',
      'version: Int!',
      'consecutiveRejections: Int!',
      'strikeLimit: Int!',
    ]) {
      expect(schemaSource, `${decl} must be present for this probe to mean anything`).toContain(
        decl,
      );
    }
    // And GraphQL `Int` is 32-bit, which is the whole premise.
    expect(() => GraphQLInt.serialize(OVER_I32)).toThrow();
    expect(() => GraphQLInt.serialize(MAX_SAFE)).toThrow();
    expect(GraphQLInt.serialize(2_147_483_647)).toBe(2_147_483_647);
  });

  /**
   * The outage, reproduced. `workflowRuns: [WorkflowRun!]!` with `seed: Int!`: one run whose seed
   * exceeds i32 and the client gets `workflowRuns: null` for the WHOLE list, not a partial result
   * and not a bad value — the same non-null propagation that emptied the tab bar.
   *
   * Probed with the SAME fields a client would request, because asking for a narrower selection
   * hides the bug.
   */
  it('one over-i32 seed nulls the entire workflowRuns list', async () => {
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

    // One run with an over-i32 seed, exactly as the host would serve it.
    const overflowed = await graphql({
      schema,
      source: query,
      rootValue: { workflowRuns: () => [run(MAX_SAFE)] },
    });
    expect(overflowed.errors, 'an over-i32 seed must produce an error').toBeDefined();
    expect(overflowed.errors?.[0].message).toMatch(/Int cannot represent non 32-bit/i);
    // Worse than the listAiSessions outage: `workflowRuns: [WorkflowRun!]!` is non-null at the
    // Query level too, so the null propagates past the list all the way to the ROOT. The client
    // receives `data: null` — every other field in the same document is lost with it.
    expect(
      overflowed.data,
      'DEFECT: the ENTIRE response data is null, not just the list',
    ).toBeNull();

    // And one bad row poisons the good ones alongside it.
    const mixed = await graphql({
      schema,
      source: query,
      rootValue: { workflowRuns: () => [run(0), run(OVER_I32), run(1)] },
    });
    expect(
      mixed.data,
      'DEFECT: two healthy runs are lost because a third overflowed',
    ).toBeNull();

    // And it is not confined to the arena: a document that also asks for something unrelated
    // loses that too, because the root is what gets nulled.
    const collateral = await graphql({
      schema,
      source: `{ listTmuxSessions { name } workflowRuns(workflowId: "w1") { id seed } }`,
      rootValue: {
        listTmuxSessions: () => [{ name: 'johnnyone_main' }],
        workflowRuns: () => [run(OVER_I32)],
      },
    });
    expect(
      collateral.data,
      'DEFECT: an unrelated field in the same query is lost as collateral',
    ).toBeNull();
  });

  /**
   * `gateCollected: Int` is NULLABLE, so an overflowing collected count degrades to `null` on that
   * field alone rather than nulling the row — but `null` is the arena's "the gate printed nothing
   * parseable" signal, so a huge-but-real count arrives as the one value that means the opposite.
   * That is quieter than the list nulling and arguably worse: nothing reports it.
   */
  it('an over-i32 gateCollected arrives as null, which already means something else', async () => {
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
    expect(result.errors).toBeDefined();
    const row = (result.data as any).workflowRuns[0];
    expect(row.id, 'the row survives because gateCollected is nullable').toBe('r1');
    expect(
      row.gateCollected,
      'DEFECT: a real count of 9007199254740991 is delivered as null, which the arena defines as ' +
        '"nothing was parseable" — the inverse claim',
    ).toBeNull();
  });

  /**
   * `fanOut: Int!` and `version: Int!` on `Workflow`, and `Workflow` is served by
   * `workflows: [Workflow!]!` and `workflow(id): Workflow!`. The host refuses only `fanOut < 1`,
   * so an over-i32 fanOut nulls the whole workflows list the same way.
   */
  it('an over-i32 fanOut nulls the whole workflows list', async () => {
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
    expect(bad.errors).toBeDefined();
    expect(bad.data, 'DEFECT: the champion list disappears entirely, and so does the root').toBeNull();
  });

  /**
   * The input side. `startWorkflowRun(seed: Int)` rejects an over-i32 literal at VALIDATION, so
   * this one direction is safe — a client cannot push an unrepresentable seed in through GraphQL.
   * Which means the only way a row gets one is a host-side or local writer, and
   * `rpc_start_workflow_run` reads `seed` with `as_i64()` and bounds only the negative side.
   */
  it('an over-i32 seed is refused on the way IN, so only a non-GraphQL writer can create one', async () => {
    const schema = buildSchema(schemaSource);
    const result = await graphql({
      schema,
      source: `mutation { startWorkflowRun(workflowId: "w1", seed: ${MAX_SAFE}, seatsHeld: "{}", fleetBusy: "{}") { id seed } }`,
      rootValue: { startWorkflowRun: () => ({ id: 'r1', seed: 0 }) },
    });
    expect(result.errors, 'the input coercion must refuse it').toBeDefined();
    expect(result.errors?.[0].message).toMatch(/Int cannot represent non 32-bit|expected type "Int"/i);
  });
});
