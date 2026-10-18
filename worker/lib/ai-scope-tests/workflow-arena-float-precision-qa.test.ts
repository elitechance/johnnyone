import { describe, it, expect } from 'vitest';
import { buildSchema, graphql, GraphQLFloat, GraphQLInt } from 'graphql';

/**
 * QA A round 2 — what the `Int` -> `Float` fix traded away.
 *
 * Round 1 measured that `seed: Int!` under `workflowRuns: [WorkflowRun!]!` nulled the entire
 * GraphQL response ROOT for any value above 2^31-1, reproducing (and exceeding) the shipped
 * `listAiSessions` outage. Dev A retyped every arena counter as `Float`, which fixes that.
 *
 * A separate file from `workflow-arena-int32-qa.test.ts` because that one is now co-authored — Dev
 * A inverted all five of its cases when the fix landed — and because this is a different claim.
 *
 * The claim: `Float` removes the hard failure and introduces a SILENT one. A GraphQL `Float` is an
 * IEEE-754 double, exact only up to 2^53. Dev A bounded `fan_out` (<= 1024), `seed`
 * (<= 1_000_000) and `gate_exit` (32 bits) on the host, so none of those can reach the unsafe
 * range. `gate_collected` is **deliberately unbounded** — their reasoning, which is sound, is that
 * refusing a real score because an advisory count is large would be the wrong trade. That leaves
 * exactly one field that can carry a value a double cannot hold, and it is the field whose `null`
 * already means something specific.
 *
 * Their own test stops at `Number.MAX_SAFE_INTEGER` (2^53 - 1), the last exact value. This starts
 * at 2^53 + 1.
 */

const schemaSources = import.meta.glob('../../schema/johnnyone-ai.graphql', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;
const schemaSource = Object.values(schemaSources)[0] ?? '';

/** 2^53 + 1 — the first integer an IEEE-754 double cannot represent. */
const FIRST_UNSAFE = '9007199254740993';
/** What a double rounds it to. */
const ROUNDED_TO = 9007199254740992;

describe('QA A r2 — the Float fix and 2^53', () => {
  it('the arena counters really are Float now, and Float is a double', () => {
    expect(schemaSource, 'the schema must be inlined').toContain('type WorkflowRun {');
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
    // The round-1 failure mode is genuinely gone …
    expect(() => GraphQLInt.serialize(2_147_483_648)).toThrow();
    expect(GraphQLFloat.serialize(2_147_483_648)).toBe(2_147_483_648);
    // … and Float is exact only to 2^53.
    expect(Number.isSafeInteger(9_007_199_254_740_991)).toBe(true);
    expect(Number.isSafeInteger(ROUNDED_TO + 1)).toBe(false);
  });

  /**
   * **DEFECT. The corruption happens before GraphQL is reached, so nothing reports it.**
   *
   * The host serialises `gate_collected` as an exact JSON integer — measured in
   * `services::workflows_qa_attacks_r2::qa_a2_the_host_bounds_everything_except_gate_collected`,
   * which stores 2^53+1 and asserts it serialises as itself. `desktopRpc` then does `JSON.parse`
   * on that body, and `JSON.parse` has nowhere to put an integer that large: it yields a double.
   * The value is already wrong by the time any resolver or `Float` serialiser sees it, so there is
   * no error, no null, and no log line — just a number that is quietly not the one the host sent.
   */
  it('a gateCollected above 2^53 is silently rounded by JSON.parse, before any resolver runs', () => {
    const fromHost = `{"id":"r1","gateExit":0,"gateCollected":${FIRST_UNSAFE}}`;
    const parsed = JSON.parse(fromHost) as { gateCollected: number };

    expect(
      String(parsed.gateCollected),
      'DEFECT: the worker cannot observe the value the host sent',
    ).not.toBe(FIRST_UNSAFE);
    expect(parsed.gateCollected).toBe(ROUNDED_TO);
    expect(
      Number.isSafeInteger(parsed.gateCollected),
      'and it looks perfectly healthy afterwards — there is nothing for a caller to detect',
    ).toBe(false);

    // Larger values lose more than one. An advisory count is advisory, but a reader comparing two
    // runs' counts is comparing numbers that were never measured.
    const big = '123456789012345678';
    expect(String(JSON.parse(`{"n":${big}}`).n)).not.toBe(big);
  });

  /**
   * And `Float` serialises the already-rounded value without complaint, so the full path is silent
   * end to end. Contrast the round-1 behaviour, which was loud and total: this is quiet and
   * partial, which for a number a human reads off a screen is the worse of the two.
   */
  it('the whole path stays silent: no error, no null, a wrong number', async () => {
    const schema = buildSchema(schemaSource);
    const result = await graphql({
      schema,
      source: `{ workflowRuns(workflowId: "w1") { id seed gateExit gateCollected } }`,
      rootValue: {
        // Exactly what desktopRpc would hand a resolver after JSON.parse.
        workflowRuns: () => [
          JSON.parse(
            `{"id":"r1","workflowId":"w1","seed":0,"gateExit":0,"gateCollected":${FIRST_UNSAFE},"seatsHeld":"{}","fleetBusy":"{}","startedAt":"now"}`,
          ),
        ],
      },
    });

    expect(result.errors, 'DEFECT: no error is raised anywhere on the path').toBeUndefined();
    const row = (result.data as any).workflowRuns[0];
    expect(row.gateCollected, 'DEFECT: and the value is not the one the host recorded').toBe(
      ROUNDED_TO,
    );
    // (An earlier draft asserted `!== Number(FIRST_UNSAFE) + 1` here. That is vacuously true in
    // the wrong direction: adding 1 to an already-rounded double rounds straight back, so the
    // expression IS the rounded value. Caught by the assertion failing; removed rather than
    // weakened, because the meaningful comparison is against the decimal STRING the host sent.)
    expect(String(row.gateCollected)).not.toBe(FIRST_UNSAFE);
    // Non-vacuous: a value inside the safe range survives this identical path exactly.
    const safe = await graphql({
      schema,
      source: `{ workflowRuns(workflowId: "w1") { id gateCollected } }`,
      rootValue: {
        workflowRuns: () => [JSON.parse('{"id":"r1","gateCollected":9007199254740991}')],
      },
    });
    expect((safe.data as any).workflowRuns[0].gateCollected).toBe(9_007_199_254_740_991);
  });

  /**
   * The bounded fields cannot reach the unsafe range, so the exposure really is confined to
   * `gateCollected`. Pinned against the host's own constants so that raising either bound without
   * revisiting the wire type turns this red.
   */
  it('the bounded counters cannot reach 2^53, so the exposure is gateCollected alone', () => {
    const MAX_FAN_OUT = 1_024; // services::workflows::MAX_FAN_OUT
    const MAX_SEED = 1_000_000; // services::workflows::MAX_SEED
    const MAX_GATE_EXIT = 2_147_483_647; // i32::MAX, the gate_exit bound
    for (const [name, bound] of [
      ['fanOut', MAX_FAN_OUT],
      ['seed', MAX_SEED],
      ['gateExit', MAX_GATE_EXIT],
    ] as const) {
      expect(Number.isSafeInteger(bound), `${name}'s bound must be safe`).toBe(true);
      expect(bound, `${name} must stay far below 2^53`).toBeLessThan(2 ** 53);
    }
    // `version` is unbounded in the schema but is assigned by the host as "one past the highest in
    // the family", so reaching 2^53 would need 2^53 versions. Recorded as reasoning, not measured.
    expect(schemaSource).toContain('version: Float!');
  });

  /**
   * The input side of the Float change, which Dev A's rewrite notes but does not push past the safe
   * range: a `Float` argument accepts an unsafe literal at coercion, so the HOST is the only thing
   * refusing it. For `seed` that is fine (`MAX_SEED`). For `gateCollected` there is deliberately no
   * host bound, so a client can also WRITE a value that cannot round-trip.
   */
  it('an unsafe gateCollected can be written in as well as read out', async () => {
    const schema = buildSchema(schemaSource);
    const seen: unknown[] = [];
    const result = await graphql({
      schema,
      source: `mutation { recordWorkflowRunGate(runId: "r1", gateExit: 0, gateCollected: ${FIRST_UNSAFE}) { id gateCollected } }`,
      rootValue: {
        recordWorkflowRunGate: (args: { gateCollected: number }) => {
          seen.push(args.gateCollected);
          return { id: 'r1', gateCollected: args.gateCollected };
        },
      },
    });
    expect(result.errors, 'Float accepts the literal — no coercion error').toBeUndefined();
    expect(
      seen[0],
      'DEFECT: the resolver receives the rounded value, so the host is asked to store a number ' +
        'the client did not send',
    ).toBe(ROUNDED_TO);

    // Non-vacuous: the same mutation with an Int-range value is exact.
    const ok = await graphql({
      schema,
      source: `mutation { recordWorkflowRunGate(runId: "r1", gateExit: 0, gateCollected: 12) { id gateCollected } }`,
      rootValue: {
        recordWorkflowRunGate: (args: { gateCollected: number }) => ({
          id: 'r1',
          gateCollected: args.gateCollected,
        }),
      },
    });
    expect((ok.data as any).recordWorkflowRunGate.gateCollected).toBe(12);
  });

  /**
   * **The `rescore` flag, which contradicts the rationale for the fix it implements.**
   *
   * The host's own words: "The separate name IS the guard: there is no flag to forget and no
   * default to get wrong." The GraphQL surface is
   * `recordWorkflowRunGate(..., rescore: Boolean)` — a flag, with a default. So the guard that was
   * argued for as a naming decision is, on the only surface a client can reach, exactly the
   * boolean argument the argument rejected. The important half still holds (the host refuses to
   * re-score the run a sitting champion was promoted on, whatever the flag says, measured in
   * `qa_a2_the_rescore_lock_holds_and_releases_correctly`), but any holder of `plans:write` can
   * re-score any other run by passing one extra argument.
   */
  it('a rescore is its own mutation, not a flag — matching the host rationale', () => {
    const line = schemaSource
      .split('\n')
      .map((l) => l.trim())
      .filter((l) => !l.startsWith('#'))
      .find((l) => l.startsWith('recordWorkflowRunGate('));
    expect(line, 'recordWorkflowRunGate must exist').toBeTruthy();
    // FIXED. The flag is gone from the wire: a second result for a run goes through its own
    // mutation, so the separate NAME is the guard — there is no flag to forget and no default to
    // get wrong, which is exactly what the host's rationale asked for and what the wire API had not
    // yet honoured.
    expect(line, 'the rescore flag must be gone').not.toContain('rescore');
    // Both doors exist, one per intent, on both the recorder and the executor.
    expect(schemaSource).toContain('recordWorkflowRunRescore(');
    expect(schemaSource).toContain('runWorkflowRunRescore(');
    const execute = schemaSource
      .split('\n')
      .map((l) => l.trim())
      .filter((l) => !l.startsWith('#'))
      .find((l) => l.startsWith('runWorkflowRunGate('));
    expect(execute, 'and not on the executor either').not.toContain('rescore');
  });
});
