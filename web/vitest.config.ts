import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    globals: true,
    environment: 'jsdom',
    // Angular plugin skipped for scaffold smoke; full component tests can add
    // `import angular from '@analogjs/vitest-angular'` + plugin when Angular
    // specs are written (see task 00 decisions).
    include: ['web/**/*.spec.ts'],
  },
});
