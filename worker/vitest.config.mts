import { cloudflareTest } from '@cloudflare/vitest-pool-workers';
import { defineConfig } from 'vitest/config';
import path from 'path';

const authMiddlewarePath = path.resolve(__dirname, '../../../../lokal/apps/lokal-infra/packages/cli/cloudflare/modules/auth/auth-middleware.ts');

const modulesAuthRewrite = {
  name: 'rewrite-modules-auth-mw',
  transform(code: string, id: string) {
    if (id.includes('chat-relay-do') || id.includes('upgrade.test')) {
      // Keep the source on disk using the canonical 'modules/auth/auth-middleware' import
      // but rewrite at transform time to a resolvable absolute path so the vitest-pool-workers
      // / workerd context can load the builtin verifier (signJwt, verify*) without bare-specifier failure.
      const resolvedImport = authMiddlewarePath.replace(/\\/g, '/');
      return code.replace(
        /from ['"]modules\/auth\/auth-middleware['"]/g,
        `from '${resolvedImport}'`
      );
    }
    return undefined;
  },
};

// @cloudflare/vitest-pool-workers 0.16.x: cloudflareTest(options) takes { main, miniflare, wrangler }
// DIRECTLY (NOT nested under poolOptions.workers — that was the old defineWorkersConfig shape, and
// nesting it meant the bindings never reached the pool, so env.DB / env.CHAT_RELAY_DO were undefined
// and the real DO-runtime tests threw their guard errors).
export default defineConfig({
  resolve: {
    alias: {
      'modules/auth/auth-middleware': authMiddlewarePath,
    },
  },
  plugins: [
    modulesAuthRewrite,
    cloudflareTest({
      main: './worker/test-worker.ts',
      miniflare: {
        compatibilityDate: '2026-03-02',
        compatibilityFlags: ['nodejs_compat'],
        d1Databases: ['DB'],
        bindings: {
          ENVIRONMENT: 'test',
          JWT_SECRET: 'test-secret-for-phase-00',
        },
        durableObjects: {
          CHAT_RELAY_DO: 'ChatRelayDO',
        },
      },
    }),
  ],
  test: {
    include: ['worker/**/*.test.ts'],
  },
});
