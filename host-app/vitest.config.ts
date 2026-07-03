import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    globals: true,
    environment: 'jsdom',
    // Plugin-less (no Angular plugin) — host-app pure-logic specs only, mirroring
    // web/vitest.config.ts. The web glob (`web/**`) does not match host-app, so
    // host-app specs need their own config (Overhaul P9 / D6).
    include: ['host-app/**/*.spec.ts'],
  },
});
