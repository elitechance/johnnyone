/**
 * Expired-token refresh-and-replay (fix/web-session-resume, part 2).
 *
 * `buildHeaders` reads `johnnyone_access_token` straight out of localStorage
 * with no freshness check, and before this change nothing in the client
 * reacted to an auth failure. The JohnnyOne worker's choke point
 * (`worker/lib/auth/require-identity.ts:19`) answers an expired/invalid bearer
 * with an HTTP **200** carrying
 *   `errors: [{ message: 'UNAUTHENTICATED', extensions: { code: 'UNAUTHENTICATED' } }]`
 * so the match cannot key on HTTP 401 alone.
 *
 * The spec lives under `web/**` because that is what `web/vitest.config.ts`
 * includes; the subject is imported relatively since the web vitest config has
 * no tsconfig-paths plugin.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { firstValueFrom } from 'rxjs';
import {
  GraphQLClient,
  GraphQLRequestError,
} from '../../../../ui/src/services/graphql-client';

const API = 'http://example.test/graphql';

type FakeResponse = {
  ok?: boolean;
  status?: number;
  statusText?: string;
  body?: unknown;
};

function res(r: FakeResponse) {
  const status = r.status ?? 200;
  return {
    ok: r.ok ?? (status >= 200 && status < 300),
    status,
    statusText: r.statusText ?? 'OK',
    json: async () => r.body ?? {},
  };
}

/** HTTP 200 + errors[] — what the worker actually returns for a dead bearer. */
const UNAUTHENTICATED_BODY = {
  data: null,
  errors: [{ message: 'UNAUTHENTICATED', extensions: { code: 'UNAUTHENTICATED' } }],
};

function makeClient(authRefresh: (() => Promise<void>) | null) {
  const c = Object.create(GraphQLClient.prototype) as GraphQLClient;
  (c as any).apiUrl = API;
  (c as any).hostApiUrl = '';
  (c as any).wsUrl = 'ws://example.test/graphql';
  (c as any).extraHeaders = {};
  (c as any).localHostHealth = null;
  (c as any).authRefresh = authRefresh;
  (c as any).inflightAuthRefresh = null;
  return c;
}

function queueFetch(responses: FakeResponse[]) {
  const calls: Array<{ url: string; headers: Record<string, string> }> = [];
  let i = 0;
  const fetchMock = vi.fn(async (url: string, init: any) => {
    calls.push({ url, headers: init?.headers ?? {} });
    const next = responses[Math.min(i, responses.length - 1)];
    i++;
    return res(next) as any;
  });
  vi.stubGlobal('fetch', fetchMock);
  return { fetchMock, calls };
}

describe('GraphQLClient expired-token refresh + single replay', () => {
  beforeEach(() => {
    localStorage.clear();
    localStorage.setItem('johnnyone_access_token', 'dead-token');
    localStorage.setItem('johnnyone_tenant_id', 't1');
    localStorage.setItem('johnnyone_user_id', 'u1');
    vi.restoreAllMocks();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    localStorage.clear();
  });

  it('UNAUTHENTICATED (HTTP 200 + errors[]) → refresh once, replay once, resolve', async () => {
    const refresh = vi.fn(async () => {
      localStorage.setItem('johnnyone_access_token', 'fresh-token');
    });
    const { fetchMock, calls } = queueFetch([
      { body: UNAUTHENTICATED_BODY },
      { body: { data: { health: 'ok' } } },
    ]);
    const client = makeClient(refresh);

    const data = await firstValueFrom(client.query<{ health: string }>('{ health }'));

    expect(data).toEqual({ health: 'ok' });
    expect(refresh).toHaveBeenCalledTimes(1);
    expect(fetchMock).toHaveBeenCalledTimes(2);
    // The replay must carry the NEW token, not the dead one.
    expect(calls[0].headers['Authorization']).toBe('Bearer dead-token');
    expect(calls[1].headers['Authorization']).toBe('Bearer fresh-token');
  });

  it('a second UNAUTHENTICATED → error surfaced, exactly 2 fetches, no infinite loop', async () => {
    const refresh = vi.fn(async () => undefined);
    const { fetchMock } = queueFetch([
      { body: UNAUTHENTICATED_BODY },
      { body: UNAUTHENTICATED_BODY },
    ]);
    const client = makeClient(refresh);

    await expect(
      firstValueFrom(client.query('{ health }')),
    ).rejects.toThrow(/UNAUTHENTICATED/);
    expect(refresh).toHaveBeenCalledTimes(1);
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it('a non-auth GraphQL error → no refresh, no replay, error surfaced as today', async () => {
    const refresh = vi.fn(async () => undefined);
    const { fetchMock } = queueFetch([
      { body: { data: null, errors: [{ message: 'Plan not found' }] } },
    ]);
    const client = makeClient(refresh);

    const err = await firstValueFrom(client.query('{ plan }')).catch((e) => e);
    expect(err).toBeInstanceOf(GraphQLRequestError);
    expect(String(err.message)).toContain('Plan not found');
    expect(refresh).toHaveBeenCalledTimes(0);
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it('HTTP 401 → refresh once and replay once', async () => {
    const refresh = vi.fn(async () => {
      localStorage.setItem('johnnyone_access_token', 'fresh-token');
    });
    const { fetchMock } = queueFetch([
      { status: 401, statusText: 'Unauthorized', body: {} },
      { body: { data: { health: 'ok' } } },
    ]);
    const client = makeClient(refresh);

    await expect(firstValueFrom(client.query('{ health }'))).resolves.toEqual({
      health: 'ok',
    });
    expect(refresh).toHaveBeenCalledTimes(1);
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it('a non-401 HTTP failure → no refresh, no replay', async () => {
    const refresh = vi.fn(async () => undefined);
    const { fetchMock } = queueFetch([
      { status: 500, statusText: 'Internal Server Error', body: {} },
    ]);
    const client = makeClient(refresh);

    await expect(firstValueFrom(client.query('{ health }'))).rejects.toThrow(/500/);
    expect(refresh).toHaveBeenCalledTimes(0);
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it('no refresh hook wired → library behaviour unchanged (one fetch, error surfaced)', async () => {
    const { fetchMock } = queueFetch([{ body: UNAUTHENTICATED_BODY }]);
    const client = makeClient(null);

    await expect(firstValueFrom(client.query('{ health }'))).rejects.toThrow(
      /UNAUTHENTICATED/,
    );
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it('a refresh that itself fails → the original auth error is surfaced, no replay', async () => {
    const refresh = vi.fn(async () => {
      throw new Error('Refresh failed: 401 Unauthorized');
    });
    const { fetchMock } = queueFetch([{ body: UNAUTHENTICATED_BODY }]);
    const client = makeClient(refresh);

    await expect(firstValueFrom(client.query('{ health }'))).rejects.toThrow(
      /UNAUTHENTICATED/,
    );
    expect(refresh).toHaveBeenCalledTimes(1);
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it('many queries failing at once → ONE shared refresh, one replay each', async () => {
    let refreshes = 0;
    let releaseRefresh: (() => void) | null = null;
    const refresh = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          refreshes++;
          releaseRefresh = () => {
            localStorage.setItem('johnnyone_access_token', 'fresh-token');
            resolve();
          };
        }),
    );
    let call = 0;
    const fetchMock = vi.fn(async () => {
      call++;
      // first three calls fail auth, the replays succeed
      return res(
        call <= 3 ? { body: UNAUTHENTICATED_BODY } : { body: { data: { n: call } } },
      ) as any;
    });
    vi.stubGlobal('fetch', fetchMock);
    const client = makeClient(refresh);

    const all = Promise.all([
      firstValueFrom(client.query('{ a }')),
      firstValueFrom(client.query('{ b }')),
      firstValueFrom(client.query('{ c }')),
    ]);
    // let the three failures land
    for (let i = 0; i < 12; i++) await Promise.resolve();
    expect(refreshes).toBe(1);
    releaseRefresh!();
    await all;

    expect(refresh).toHaveBeenCalledTimes(1);
    expect(fetchMock).toHaveBeenCalledTimes(6);
  });

  it('a mutation rejected at the auth choke point is replayed once', async () => {
    const refresh = vi.fn(async () => {
      localStorage.setItem('johnnyone_access_token', 'fresh-token');
    });
    const { fetchMock } = queueFetch([
      { body: UNAUTHENTICATED_BODY },
      { body: { data: { createAgentPlan: { id: 'p1' } } } },
    ]);
    const client = makeClient(refresh);

    await expect(
      firstValueFrom(client.mutate('mutation { createAgentPlan { id } }')),
    ).resolves.toEqual({ createAgentPlan: { id: 'p1' } });
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  /**
   * QA F1. `getAiSession`, `getPlanCheck` and `getTaskRun` are the schema's
   * only three NULLABLE root fields (`worker/schema/johnnyone-ai.graphql:544`,
   * `:551`, `:552`); every Mutation field is non-null, so an auth error there
   * bubbles to the root and `data` is null outright. On those three queries it
   * nullifies just that field, so the body arrives as
   * `{data: {getAiSession: null}, errors: [...]}` — `data` is a non-null
   * OBJECT, which the first cut of the partial-data rail read as "partial" and
   * refused to retry. `getAiSession` is on the phone's hot path, i.e. exactly
   * the surface this change exists to fix.
   *
   * Rule: a `data` object whose every own enumerable value is null carries no
   * applied work, so it is not partial. One non-null field and it is.
   */
  describe('all-null data is not partial data (QA F1)', () => {
    it('{data:{getAiSession:null}, errors:[UNAUTHENTICATED]} → one refresh, one replay', async () => {
      const refresh = vi.fn(async () => {
        localStorage.setItem('johnnyone_access_token', 'fresh-token');
      });
      const { fetchMock } = queueFetch([
        {
          body: {
            data: { getAiSession: null },
            errors: [{ message: 'UNAUTHENTICATED', extensions: { code: 'UNAUTHENTICATED' } }],
          },
        },
        { body: { data: { getAiSession: { id: 's1' } } } },
      ]);
      const client = makeClient(refresh);

      await expect(
        firstValueFrom(client.query('query { getAiSession(id: "s1") { id } }')),
      ).resolves.toEqual({ getAiSession: { id: 's1' } });
      expect(refresh).toHaveBeenCalledTimes(1);
      expect(fetchMock).toHaveBeenCalledTimes(2);
    });

    it('a nullable String root field (getPlanCheck) behaves the same', async () => {
      const refresh = vi.fn(async () => {
        localStorage.setItem('johnnyone_access_token', 'fresh-token');
      });
      const { fetchMock } = queueFetch([
        {
          body: {
            data: { getPlanCheck: null },
            errors: [{ message: 'UNAUTHENTICATED', extensions: { code: 'UNAUTHENTICATED' } }],
          },
        },
        { body: { data: { getPlanCheck: 'green' } } },
      ]);
      const client = makeClient(refresh);

      await expect(firstValueFrom(client.query('{ getPlanCheck }'))).resolves.toEqual({
        getPlanCheck: 'green',
      });
      expect(refresh).toHaveBeenCalledTimes(1);
      expect(fetchMock).toHaveBeenCalledTimes(2);
    });

    it('several root fields, ALL null → still retried', async () => {
      const refresh = vi.fn(async () => undefined);
      const { fetchMock } = queueFetch([
        {
          body: {
            data: { getAiSession: null, getTaskRun: null },
            errors: [{ message: 'UNAUTHENTICATED', extensions: { code: 'UNAUTHENTICATED' } }],
          },
        },
        { body: { data: { getAiSession: null, getTaskRun: 'ok' } } },
      ]);
      const client = makeClient(refresh);

      await expect(
        firstValueFrom(client.query('{ getAiSession { id } getTaskRun }')),
      ).resolves.toEqual({ getAiSession: null, getTaskRun: 'ok' });
      expect(refresh).toHaveBeenCalledTimes(1);
      expect(fetchMock).toHaveBeenCalledTimes(2);
    });

    it('MIXED data — one field applied, one nulled → still REFUSED (the rail must not loosen)', async () => {
      const refresh = vi.fn(async () => undefined);
      const { fetchMock } = queueFetch([
        {
          body: {
            data: { a: 1, b: null },
            errors: [{ message: 'UNAUTHENTICATED', extensions: { code: 'UNAUTHENTICATED' } }],
          },
        },
      ]);
      const client = makeClient(refresh);

      await expect(firstValueFrom(client.query('{ a b }'))).rejects.toThrow(
        /UNAUTHENTICATED/,
      );
      expect(refresh).toHaveBeenCalledTimes(0);
      expect(fetchMock).toHaveBeenCalledTimes(1);
    });

    it('a falsy-but-not-null applied value (0, "", false) still counts as applied → REFUSED', async () => {
      const refresh = vi.fn(async () => undefined);
      const { fetchMock } = queueFetch([
        {
          body: {
            data: { count: 0, label: '', flag: false, missing: null },
            errors: [{ message: 'UNAUTHENTICATED', extensions: { code: 'UNAUTHENTICATED' } }],
          },
        },
      ]);
      const client = makeClient(refresh);

      await expect(
        firstValueFrom(client.query('{ count label flag missing }')),
      ).rejects.toThrow(/UNAUTHENTICATED/);
      expect(refresh).toHaveBeenCalledTimes(0);
      expect(fetchMock).toHaveBeenCalledTimes(1);
    });

    it('an empty data object {} → retried (nothing was applied)', async () => {
      const refresh = vi.fn(async () => undefined);
      const { fetchMock } = queueFetch([
        {
          body: {
            data: {},
            errors: [{ message: 'UNAUTHENTICATED', extensions: { code: 'UNAUTHENTICATED' } }],
          },
        },
        { body: { data: { health: 'ok' } } },
      ]);
      const client = makeClient(refresh);

      await expect(firstValueFrom(client.query('{ health }'))).resolves.toEqual({
        health: 'ok',
      });
      expect(fetchMock).toHaveBeenCalledTimes(2);
    });

    it('a non-object data (scalar/array) with an auth error is treated as applied → REFUSED', async () => {
      const refresh = vi.fn(async () => undefined);
      const { fetchMock } = queueFetch([
        {
          body: {
            data: [{ id: 1 }],
            errors: [{ message: 'UNAUTHENTICATED', extensions: { code: 'UNAUTHENTICATED' } }],
          },
        },
      ]);
      const client = makeClient(refresh);

      await expect(firstValueFrom(client.query('{ things }'))).rejects.toThrow(
        /UNAUTHENTICATED/,
      );
      expect(refresh).toHaveBeenCalledTimes(0);
      expect(fetchMock).toHaveBeenCalledTimes(1);
    });

    it('a nested object under an all-null root field is NOT reached (only own top-level values count)', async () => {
      // `{getAiSession: {id: null}}` means the session WAS resolved and its
      // field nulled — the root field holds an object, so this is applied work.
      const refresh = vi.fn(async () => undefined);
      const { fetchMock } = queueFetch([
        {
          body: {
            data: { getAiSession: { id: null } },
            errors: [{ message: 'UNAUTHENTICATED', extensions: { code: 'UNAUTHENTICATED' } }],
          },
        },
      ]);
      const client = makeClient(refresh);

      await expect(
        firstValueFrom(client.query('{ getAiSession { id } }')),
      ).rejects.toThrow(/UNAUTHENTICATED/);
      expect(refresh).toHaveBeenCalledTimes(0);
      expect(fetchMock).toHaveBeenCalledTimes(1);
    });
  });

  it('an auth error alongside PARTIAL data is NOT replayed (the write may have landed)', async () => {
    const refresh = vi.fn(async () => undefined);
    const { fetchMock } = queueFetch([
      {
        body: {
          data: { createAgentPlan: { id: 'p1' }, other: null },
          errors: [
            { message: 'UNAUTHENTICATED', extensions: { code: 'UNAUTHENTICATED' } },
          ],
        },
      },
    ]);
    const client = makeClient(refresh);

    await expect(
      firstValueFrom(client.mutate('mutation { createAgentPlan { id } other }')),
    ).rejects.toThrow(/UNAUTHENTICATED/);
    expect(refresh).toHaveBeenCalledTimes(0);
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it('an authorization (role) failure is not treated as an expired token', async () => {
    const refresh = vi.fn(async () => undefined);
    const { fetchMock } = queueFetch([
      {
        body: {
          data: null,
          errors: [{ message: 'Unauthorized: ADMIN role required' }],
        },
      },
    ]);
    const client = makeClient(refresh);

    await expect(firstValueFrom(client.query('{ secret }'))).rejects.toThrow(
      /ADMIN role required/,
    );
    expect(refresh).toHaveBeenCalledTimes(0);
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("the worker's resolver-level 'Not authenticated' is also retried", async () => {
    const refresh = vi.fn(async () => {
      localStorage.setItem('johnnyone_access_token', 'fresh-token');
    });
    const { fetchMock } = queueFetch([
      { body: { data: null, errors: [{ message: 'Not authenticated' }] } },
      { body: { data: { listApiKeys: [] } } },
    ]);
    const client = makeClient(refresh);

    await expect(firstValueFrom(client.query('{ listApiKeys }'))).resolves.toEqual({
      listApiKeys: [],
    });
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it('a request sent without auth headers is never retried', async () => {
    const refresh = vi.fn(async () => undefined);
    const { fetchMock } = queueFetch([{ body: UNAUTHENTICATED_BODY }]);
    const client = makeClient(refresh);

    await expect(
      firstValueFrom(
        (client as any).requestAt('http://host.test/graphql', '{ health }', undefined, false),
      ),
    ).rejects.toThrow(/UNAUTHENTICATED/);
    expect(refresh).toHaveBeenCalledTimes(0);
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });
});
