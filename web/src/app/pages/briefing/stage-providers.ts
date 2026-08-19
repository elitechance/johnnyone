/**
 * Per-stage agent binding for the SDLC loop.
 *
 * An initiative runs as two stage-runs sharing an initiative id: planning, then development. Each
 * carries its own worker (builds) and reviewer (validates) provider. Historically the development
 * stage-run cloned the planning row's pair, so one agent necessarily did both stages. These fields
 * let the caller bind development independently — e.g. claude plans, grok builds, claude validates.
 *
 * Kept as a pure function (no Angular, no injector) so the mapping is unit-testable without
 * standing up the page.
 */
export interface StageProviderPayload {
  devWorkerProvider?: string;
  devReviewerProvider?: string;
}

/**
 * Map the two form selections to the mutation payload.
 *
 * The form's "same as planning" choice is the empty string. An omitted key is what the host reads as
 * "inherit the planning agent", so a blank selection must produce **no key at all** — sending
 * `devWorkerProvider: ''` would persist an empty provider name that resolves to nothing.
 */
export function stageProviderPayload(
  devWorker: string | null | undefined,
  devReviewer: string | null | undefined,
): StageProviderPayload {
  const payload: StageProviderPayload = {};
  const worker = (devWorker ?? '').trim();
  const reviewer = (devReviewer ?? '').trim();
  if (worker) payload.devWorkerProvider = worker;
  if (reviewer) payload.devReviewerProvider = reviewer;
  return payload;
}
