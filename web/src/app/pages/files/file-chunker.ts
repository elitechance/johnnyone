// Pure, Angular/Ionic-free upload chunk math (overhaul P5 Phase 03, D5/D6). Everything correctness-
// critical about the upload path lives here so it is unit-testable without any IO: the chunk plan, the
// payload-free call sequence (the "monotonic index, `done` only on the last chunk" invariant), the
// byte-wise base64 encode (the highest-risk step), the progress fraction, and the friendly per-file
// cap pre-check. The component (`files.page.ts`) only performs the `File.slice`/`arrayBuffer` read and
// zips the bytes through `bytesToBase64` at send time. Defaults mirror the host caps (1 MiB/chunk,
// 50 MiB total — `host_files.rs`); the host remains the authority (D2/D5).

/** Host per-chunk cap: 1 MiB (`MAX_UPLOAD_CHUNK_BYTES`). */
export const CHUNK_BYTES = 1024 * 1024;
/** Host per-destination total cap: 50 MiB (`MAX_UPLOAD_TOTAL_BYTES`). */
export const TOTAL_CAP_BYTES = 50 * 1024 * 1024;

export interface ChunkRange {
  index: number;
  start: number;
  end: number;
}

export interface UploadCall {
  chunkIndex: number;
  totalChunks: number;
  done: boolean;
}

/**
 * Contiguous byte ranges covering `[0, size)`, each at most `chunkBytes`; the last range's `end`
 * equals `size`. A zero-byte file yields exactly one empty chunk `{ index:0, start:0, end:0 }`, so an
 * empty file still uploads and the `done` chunk fires.
 */
export function planChunks(size: number, chunkBytes: number = CHUNK_BYTES): ChunkRange[] {
  if (size <= 0) return [{ index: 0, start: 0, end: 0 }];
  const chunks: ChunkRange[] = [];
  let start = 0;
  let index = 0;
  while (start < size) {
    const end = Math.min(start + chunkBytes, size);
    chunks.push({ index, start, end });
    start = end;
    index += 1;
  }
  return chunks;
}

/**
 * Ordered, payload-free per-chunk call descriptor for a file's `totalChunks` chunks: `chunkIndex`
 * counts `0..totalChunks-1`, `totalChunks` is constant, and `done` is true only on the last chunk.
 * A 1-chunk (or empty) file yields `[{ chunkIndex:0, totalChunks:1, done:true }]`. This is the pure
 * seam that pins the upload invariant without any IO (D6).
 */
export function uploadCallSequence(totalChunks: number): UploadCall[] {
  const total = Math.max(0, Math.floor(totalChunks));
  const sequence: UploadCall[] = [];
  for (let chunkIndex = 0; chunkIndex < total; chunkIndex += 1) {
    sequence.push({ chunkIndex, totalChunks: total, done: chunkIndex === total - 1 });
  }
  return sequence;
}

/**
 * Base64-encode a byte array — the correctness-critical step. Walks the bytes into a latin1 string in
 * fixed-size blocks (to avoid call-stack limits on large inputs) before `btoa`. Passing `btoa` a raw
 * `ArrayBuffer` or a multibyte JS string corrupts any byte > 127, which is exactly why this is a pure,
 * spec-pinned helper (its test includes a bytes-> 127 vector — D5/D6). `btoa` is a browser/jsdom
 * global, so this runs under the plugin-less web vitest.
 */
export function bytesToBase64(bytes: Uint8Array): string {
  const BLOCK = 0x8000; // 32 KiB worth of chars per fromCharCode call
  let binary = '';
  for (let i = 0; i < bytes.length; i += BLOCK) {
    const block = bytes.subarray(i, i + BLOCK);
    binary += String.fromCharCode.apply(null, block as unknown as number[]);
  }
  return btoa(binary);
}

/** Upload progress as a `0..1` fraction; `totalChunks <= 0` is treated as complete (`1`). */
export function uploadProgress(receivedChunks: number, totalChunks: number): number {
  if (totalChunks <= 0) return 1;
  return Math.min(1, Math.max(0, receivedChunks / totalChunks));
}

/**
 * Friendly client-side per-file pre-check against the host's 50 MiB total cap. The host enforces
 * `MAX_UPLOAD_TOTAL_BYTES` per destination path (it accumulates against that file's own `.part`
 * sidecar), so each file in a multi-file drop is an independent 50 MiB upload — call this once per
 * file, never on the summed drop. The host remains the authority (D2/D5).
 */
export function exceedsTotalCap(size: number, capBytes: number = TOTAL_CAP_BYTES): boolean {
  return size > capBytes;
}
