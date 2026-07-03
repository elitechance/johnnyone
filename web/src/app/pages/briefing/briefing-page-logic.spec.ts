import { describe, it, expect } from 'vitest';

// The briefing DECISION logic is extracted into an Angular/Ionic-free module (decision D8) so it can
// be unit-tested directly under the plugin-less web vitest — the real BriefingPage pulls in Ionic +
// the chat-window. The page/composer delegate to exactly these functions.
import {
  composeBriefingSeed,
  foldMessages,
  canAccept,
  shouldSeed,
  pendingAttachmentsReducer,
  referencePathsReducer,
  BRIEFING_PREAMBLE,
  type RawMessage,
} from './briefing-page-logic';

describe('briefing page — pure logic', () => {
  describe('composeBriefingSeed', () => {
    it('includes the preamble, the raw ask, and both context sections when provided', () => {
      const seed = composeBriefingSeed(
        'add a file browser',
        ['/store/INIT/attachments/spec.pdf'],
        ['/home/u/worker'],
      );
      expect(seed).toContain(BRIEFING_PREAMBLE);
      expect(seed).toContain('add a file browser');
      expect(seed).toContain('Attached files:');
      expect(seed).toContain('- /store/INIT/attachments/spec.pdf');
      expect(seed).toContain('Referenced host paths:');
      expect(seed).toContain('- /home/u/worker');
    });

    it('omits each section when its list is empty (no dangling headers)', () => {
      const seed = composeBriefingSeed('just the ask', [], []);
      expect(seed).toContain('just the ask');
      expect(seed).not.toContain('Attached files:');
      expect(seed).not.toContain('Referenced host paths:');
    });

    it('omits blank/whitespace-only entries', () => {
      const seed = composeBriefingSeed('ask', ['   ', '/real/path'], ['']);
      expect(seed).toContain('- /real/path');
      expect(seed).not.toContain('Referenced host paths:');
    });
  });

  describe('foldMessages', () => {
    it('maps raw rows to AiMessage in order, preserving role/content/id', () => {
      const raw: RawMessage[] = [
        { id: 'm1', role: 'user', content: 'hi' },
        { id: 'm2', role: 'assistant', content: 'hello' },
        { id: 'm3', role: 'user', content: 'more' },
      ];
      const folded = foldMessages(raw);
      expect(folded.map((m) => m.id)).toEqual(['m1', 'm2', 'm3']);
      expect(folded.map((m) => m.role)).toEqual(['user', 'assistant', 'user']);
      expect(folded[1].content).toBe('hello');
    });

    it('normalizes an unknown role to assistant', () => {
      const folded = foldMessages([{ id: 'x', role: 'weird', content: 'c' }]);
      expect(folded[0].role).toBe('assistant');
    });
  });

  describe('canAccept', () => {
    it('is true only for a briefing-status initiative', () => {
      expect(canAccept('briefing')).toBe(true);
      expect(canAccept('planning')).toBe(false);
      expect(canAccept(undefined)).toBe(false);
      expect(canAccept(null)).toBe(false);
    });
  });

  describe('shouldSeed', () => {
    it('seeds only when the conversation is empty', () => {
      expect(shouldSeed([])).toBe(true);
      expect(shouldSeed([{ length: 1 } as any])).toBe(false);
      expect(shouldSeed(null)).toBe(false);
    });
  });

  describe('chip reducers', () => {
    it('adds and dedupes attachment chips, ignoring blanks', () => {
      let s: string[] = [];
      s = pendingAttachmentsReducer(s, { type: 'add', value: '/a.pdf' });
      s = pendingAttachmentsReducer(s, { type: 'add', value: '/a.pdf' }); // dupe → no change
      s = pendingAttachmentsReducer(s, { type: 'add', value: '   ' }); // blank → no change
      s = pendingAttachmentsReducer(s, { type: 'add', value: '/b.png' });
      expect(s).toEqual(['/a.pdf', '/b.png']);
      s = pendingAttachmentsReducer(s, { type: 'remove', value: '/a.pdf' });
      expect(s).toEqual(['/b.png']);
      s = pendingAttachmentsReducer(s, { type: 'reset' });
      expect(s).toEqual([]);
    });

    it('reference-paths reducer add/dedupe/remove behaves the same', () => {
      let s: string[] = [];
      s = referencePathsReducer(s, { type: 'add', value: '/w/docs' });
      s = referencePathsReducer(s, { type: 'add', value: '/w/docs' });
      expect(s).toEqual(['/w/docs']);
      s = referencePathsReducer(s, { type: 'remove', value: '/w/docs' });
      expect(s).toEqual([]);
    });
  });
});
