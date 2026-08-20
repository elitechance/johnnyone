import { stageProviderPayload } from './stage-providers';

describe('stageProviderPayload', () => {
  it('omits both keys when neither stage agent is chosen ("same as planning")', () => {
    // An omitted key is how the host is told to inherit the planning agent. An empty-string value
    // would instead persist a provider named '' — the bug this guards.
    expect(stageProviderPayload('', '')).toEqual({});
    expect(stageProviderPayload(null, undefined)).toEqual({});
    expect(stageProviderPayload('   ', '  ')).toEqual({});
  });

  it('binds grok to build and claude to validate — the requested shape', () => {
    expect(stageProviderPayload('grok', 'claude_code')).toEqual({
      devWorkerProvider: 'grok',
      devReviewerProvider: 'claude_code',
    });
  });

  it('binds each side independently', () => {
    expect(stageProviderPayload('grok', '')).toEqual({ devWorkerProvider: 'grok' });
    expect(stageProviderPayload('', 'claude_code')).toEqual({ devReviewerProvider: 'claude_code' });
  });

  it('trims surrounding whitespace', () => {
    expect(stageProviderPayload(' grok ', ' claude_code ')).toEqual({
      devWorkerProvider: 'grok',
      devReviewerProvider: 'claude_code',
    });
  });
});
