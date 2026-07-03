import { describe, it, expect } from 'vitest';

// The upload chunk math (overhaul P5 Phase 03, D5/D6) is a pure module so every correctness-critical
// step — chunk boundaries, the payload-free call sequence, the byte-wise base64 encode, the progress
// fraction, and the per-file cap pre-check — is pinned without any IO. The component only performs the
// `File.slice`/`arrayBuffer` read and zips the bytes through `bytesToBase64` at send time.
import {
  planChunks,
  uploadCallSequence,
  bytesToBase64,
  uploadProgress,
  exceedsTotalCap,
} from './file-chunker';

const MIB = 1024 * 1024;

describe('file chunker — pure logic', () => {
  describe('planChunks', () => {
    it('yields exactly one empty chunk for a zero-byte file (so empty files still upload)', () => {
      expect(planChunks(0)).toEqual([{ index: 0, start: 0, end: 0 }]);
    });

    it('splits 1.5 MiB into two chunks with correct start/end and last end === size', () => {
      const chunks = planChunks(1.5 * MIB);
      expect(chunks).toEqual([
        { index: 0, start: 0, end: MIB },
        { index: 1, start: MIB, end: 1.5 * MIB },
      ]);
      expect(chunks[chunks.length - 1].end).toBe(1.5 * MIB);
    });

    it('splits 2 MiB into two full chunks', () => {
      expect(planChunks(2 * MIB)).toEqual([
        { index: 0, start: 0, end: MIB },
        { index: 1, start: MIB, end: 2 * MIB },
      ]);
    });
  });

  describe('uploadCallSequence', () => {
    it('one chunk → single done element', () => {
      expect(uploadCallSequence(1)).toEqual([{ chunkIndex: 0, totalChunks: 1, done: true }]);
    });

    it('three chunks → monotonic index, constant total, done only on the last', () => {
      expect(uploadCallSequence(3)).toEqual([
        { chunkIndex: 0, totalChunks: 3, done: false },
        { chunkIndex: 1, totalChunks: 3, done: false },
        { chunkIndex: 2, totalChunks: 3, done: true },
      ]);
    });
  });

  describe('bytesToBase64', () => {
    it('encodes a binary vector INCLUDING bytes > 127 without corruption', () => {
      // 0x80 (128) and 0xff (255) are exactly where an ArrayBuffer→btoa shortcut corrupts.
      expect(bytesToBase64(new Uint8Array([0x00, 0x7f, 0x80, 0xff]))).toBe('AH+A/w==');
    });

    it('encodes an ASCII sanity vector', () => {
      expect(bytesToBase64(new Uint8Array([0x48, 0x69]))).toBe('SGk=');
    });
  });

  describe('uploadProgress', () => {
    it('is received/total, guards a zero total as complete', () => {
      expect(uploadProgress(1, 4)).toBe(0.25);
      expect(uploadProgress(4, 4)).toBe(1);
      expect(uploadProgress(0, 0)).toBe(1);
    });
  });

  describe('exceedsTotalCap (per file)', () => {
    it('flags only sizes strictly over the 50 MiB cap', () => {
      expect(exceedsTotalCap(60 * MIB)).toBe(true);
      expect(exceedsTotalCap(10 * MIB)).toBe(false);
      expect(exceedsTotalCap(50 * MIB)).toBe(false);
    });
  });
});
