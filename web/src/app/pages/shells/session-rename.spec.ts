// Shell rename (initiative 6672dfab, task 02): the shared Ionic prompt, the wire contract of the
// EXISTING `updateAiSessionTitle` mutation, and how the two compose through `renameIntent`.
//
// Neither page can be TestBed-compiled here (`web/vitest.config.ts` has no `@analogjs/vitest-angular`,
// so a `templateUrl` component cannot be instantiated). So the prompt is driven with a fake
// `AlertController`, and the mutation through the REAL `JohnnyApiService` with a stub `GraphQLClient`
// — the idiom of `web/src/app/pages/terminal/watchdog-triggers.spec.ts`.
import '@angular/compiler';
import { describe, it, expect, beforeEach } from 'vitest';
import { TestBed } from '@angular/core/testing';
import { firstValueFrom, of, throwError, type Observable } from 'rxjs';
import '../../../test-setup';
import { JohnnyApiService } from '../../../../../ui/src/services/johnny-api.service';
import { GraphQLClient } from '../../../../../ui/src/services/graphql-client';
import {
  promptSessionRename,
  type RenameAlertController,
  type RenameAlertOptions,
} from './session-rename-prompt';
import { renameIntent } from './shells-page-logic';

/** Records what the prompt asked for and dismisses with a chosen result. */
class FakeAlertController implements RenameAlertController {
  created: RenameAlertOptions[] = [];
  presented = 0;
  constructor(private readonly result: { role?: string; data?: any }) {}
  async create(opts: RenameAlertOptions) {
    this.created.push(opts);
    return {
      present: async () => {
        this.presented++;
      },
      onDidDismiss: async () => this.result,
    };
  }
}

class StubClient {
  sent: { op: string; variables: Record<string, unknown> | undefined }[] = [];
  payload: unknown = {};
  error: Error | null = null;
  private reply<T>(op: string, variables?: Record<string, unknown>): Observable<T> {
    this.sent.push({ op, variables });
    if (this.error) {
      const e = this.error;
      return throwError(() => e);
    }
    return of(this.payload as T);
  }
  query<T>(op: string, variables?: Record<string, unknown>) {
    return this.reply<T>(op, variables);
  }
  queryPreferLocalHost<T>(workerOp: string, _localOp: string, variables?: Record<string, unknown>) {
    return this.reply<T>(workerOp, variables);
  }
  mutate<T>(op: string, variables?: Record<string, unknown>) {
    return this.reply<T>(op, variables);
  }
}

describe('promptSessionRename', () => {
  it('opens one Ionic alert: header, one prefilled title input capped at 120, Cancel/Save', async () => {
    const fake = new FakeAlertController({ role: 'cancel' });
    await promptSessionRename(fake, '  build  ');
    expect(fake.created.length).toBe(1);
    const opts = fake.created[0];
    expect(opts.header).toBe('Rename shell');
    expect(opts.inputs.length).toBe(1);
    expect(opts.inputs[0].name).toBe('title');
    expect(opts.inputs[0].type).toBe('text');
    expect(opts.inputs[0].value).toBe('build');
    expect(opts.inputs[0].placeholder).toBe('Shell');
    expect(opts.inputs[0].attributes.maxlength).toBe(120);
    expect(opts.buttons.map((b) => b.role)).toEqual(['cancel', 'confirm']);
    expect(opts.buttons.map((b) => b.text)).toEqual(['Cancel', 'Save']);
    expect('message' in opts, 'user text never goes through the HTML message').toBe(false);
    expect(fake.presented).toBe(1);
  });

  it('Save returns the RAW input (no trimming here)', async () => {
    const fake = new FakeAlertController({ role: 'confirm', data: { values: { title: '  new ' } } });
    expect(await promptSessionRename(fake, 'old')).toBe('  new ');
  });

  it('Cancel returns null', async () => {
    expect(await promptSessionRename(new FakeAlertController({ role: 'cancel' }), 'old')).toBeNull();
  });

  it('backdrop dismissal returns null', async () => {
    expect(await promptSessionRename(new FakeAlertController({ role: 'backdrop' }), 'old')).toBeNull();
  });

  it('Save with no data returns an empty string', async () => {
    expect(await promptSessionRename(new FakeAlertController({ role: 'confirm' }), 'old')).toBe('');
  });

  it('a null current title prefills an empty input (not the "Shell" fallback)', async () => {
    const fake = new FakeAlertController({ role: 'cancel' });
    await promptSessionRename(fake, null);
    expect(fake.created[0].inputs[0].value).toBe('');
  });
});

describe('updateSessionTitle contract', () => {
  let api: JohnnyApiService;
  let gql: StubClient;

  beforeEach(() => {
    gql = new StubClient();
    TestBed.resetTestingModule();
    TestBed.configureTestingModule({
      providers: [JohnnyApiService, { provide: GraphQLClient, useValue: gql }],
    });
    api = TestBed.inject(JohnnyApiService);
  });

  it('sends one updateAiSessionTitle mutation with exactly { id, title }', async () => {
    const row = { id: 's1', title: 'new', attachedTmux: true, terminalStatus: 'attached' };
    gql.payload = { updateAiSessionTitle: row };

    const out = await firstValueFrom(api.updateSessionTitle('s1', 'new'));

    expect(gql.sent.length, 'one rename sends exactly one operation').toBe(1);
    const { op, variables } = gql.sent[0];
    expect(op).toContain('updateAiSessionTitle');
    expect(op).toContain('$id: ID!');
    expect(op).toContain('$title: String!');
    expect(op, 'the selection keeps the row shape').toContain('attachedTmux');
    expect(variables).toEqual({ id: 's1', title: 'new' });
    expect(out).toEqual(row);
  });

  it('rejects with the transport error so the pages can surface it', async () => {
    gql.error = new Error('No online backend app found');
    await expect(firstValueFrom(api.updateSessionTitle('s1', 'new'))).rejects.toThrow(
      'No online backend app found',
    );
  });
});

describe('rename composition', () => {
  let api: JohnnyApiService;
  let gql: StubClient;

  beforeEach(() => {
    gql = new StubClient();
    TestBed.resetTestingModule();
    TestBed.configureTestingModule({
      providers: [JohnnyApiService, { provide: GraphQLClient, useValue: gql }],
    });
    api = TestBed.inject(JohnnyApiService);
  });

  // Exactly the call-site rule both pages follow: prompt → renameIntent → request only on non-null.
  async function rename(current: string, alert: FakeAlertController) {
    const input = await promptSessionRename(alert, current);
    const intent = renameIntent(current, input);
    if (!intent) return null;
    return firstValueFrom(api.updateSessionTitle('s1', intent.title));
  }

  it('confirming the unchanged title sends no operation', async () => {
    const alert = new FakeAlertController({ role: 'confirm', data: { values: { title: ' build ' } } });
    expect(await rename('build', alert)).toBeNull();
    expect(gql.sent.length).toBe(0);
  });

  it('cancelling sends no operation', async () => {
    expect(await rename('build', new FakeAlertController({ role: 'cancel' }))).toBeNull();
    expect(gql.sent.length).toBe(0);
  });

  it('a new title sends the trimmed value', async () => {
    gql.payload = { updateAiSessionTitle: { id: 's1', title: 'deploy' } };
    const alert = new FakeAlertController({ role: 'confirm', data: { values: { title: '  deploy ' } } });
    await rename('build', alert);
    expect(gql.sent.length).toBe(1);
    expect(gql.sent[0].variables).toEqual({ id: 's1', title: 'deploy' });
  });
});
