import { defineConfig } from 'vitest/config';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const __dirname = dirname(fileURLToPath(import.meta.url));
const r = (p) => resolve(__dirname, p);

export default defineConfig({
  resolve: {
    alias: {
      '@root': r('.'),
      '@common': r('common'),
      '@components': r('components'),
      '@modules': r('modules'),
      '@app': r('app'),
      '@lib': r('lib'),
    },
  },
  test: {
    globals: true,
    environment: 'jsdom',
    setupFiles: ['./__tests__/setup.ts'],
    // Cap fork workers so the suite does not oversubscribe CPU on an 18-core box
    // (and especially when other builds run alongside). Oversubscription starved
    // async effects and caused rare false-timeout flakes in data-loading screens.
    // Cap fork workers so the suite does not oversubscribe CPU on an 18-core box;
    // oversubscription starved async effects and aggravated render-timing flakes.
    pool: 'forks',
    poolOptions: { forks: { maxForks: 6, minForks: 1 } },
    include: ['__tests__/**/*.test.{ts,tsx,mjs}', 'lib/**/*.test.{ts,tsx}'],
    exclude: ['node_modules/**', '.next/**', 'out/**', 'dist/**'],
  },
});
