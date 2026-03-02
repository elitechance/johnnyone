/** Generate a new UUID v4 */
export function generateId(): string {
  return crypto.randomUUID();
}

/** Generate a prefixed ID (e.g., "ses_abc123") */
export function generatePrefixedId(prefix: string): string {
  return `${prefix}_${crypto.randomUUID().replace(/-/g, '').substring(0, 12)}`;
}
