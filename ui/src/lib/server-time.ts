/**
 * Pure (DOM-free) rules for timestamps that come from the engine.
 *
 * The engine stores times with SQLite's `datetime('now')`, which is UTC written WITHOUT a zone
 * (`2026-11-24 13:14:38`). `new Date()` reads a zone-less date-time as LOCAL time, so a message
 * sent at 08:14 in New York showed 08:14 while live (the optimistic bubble uses an ISO `Z` time)
 * and 01:14 PM after a reload (the stored row). Every engine time goes through `toIsoUtc` first.
 */

const ZONELESS = /^(\d{4}-\d{2}-\d{2})[ T](\d{2}:\d{2}(?::\d{2}(?:\.\d+)?)?)$/;

/** An engine time as ISO-8601 UTC. A zone-less date-time is UTC; anything else passes through. */
export function toIsoUtc(ts: string | null | undefined): string {
  const s = (ts ?? '').trim();
  const m = ZONELESS.exec(s);
  return m ? `${m[1]}T${m[2]}Z` : s;
}

/** An engine time as a `Date` (zone-less = UTC), or `null` when it is empty or unparseable. */
export function parseServerTime(ts: string | null | undefined): Date | null {
  const iso = toIsoUtc(ts);
  if (!iso) return null;
  const ms = Date.parse(iso);
  return Number.isNaN(ms) ? null : new Date(ms);
}

/** The chat bubble's clock time (`08:14 AM`) in the viewer's zone; `''` for no/invalid time. */
export function formatMessageTime(ts: string | null | undefined, locales?: string | string[]): string {
  const d = parseServerTime(ts);
  return d ? d.toLocaleTimeString(locales ?? [], { hour: '2-digit', minute: '2-digit' }) : '';
}
