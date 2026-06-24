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
    include: ['__tests__/**/*.test.{ts,tsx,mjs}', 'lib/**/*.test.{ts,tsx}'],
    exclude: ['node_modules/**', '.next/**', 'out/**', 'dist/**'],
  },
});
