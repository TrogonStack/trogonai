import { readdirSync, realpathSync } from 'node:fs';
import { dirname, join, sep } from 'node:path';

import { defineConfig } from 'vitest/config';

const bazelRoot = process.env.VITE_BAZEL_RUNFILES === '1' ? dirname(realpathSync('vitest.config.ts')) : undefined;
const literalPattern = (file: string) =>
  file
    .split(sep)
    .join('/')
    .replace(/[*?[\]{}()!+@|\\#]/g, '\\$&');
const declaredTests = bazelRoot
  ? readdirSync('tests', { recursive: true, encoding: 'utf8' })
      .filter((file) => file.endsWith('.test.ts'))
      .map((file) => literalPattern(`tests/${file}`))
  : undefined;

export default defineConfig({
  root: bazelRoot,
  server: { fs: { strict: process.env.VITE_ALLOW_OUTSIDE_ROOT !== '1' } },
  test: {
    include: declaredTests ?? ['tests/**/*.test.ts'],
    exclude: ['**/node_modules/**', '**/*.runfiles/**'],
    coverage: {
      provider: 'v8',
      include: ['src/**/*.ts'],
      reporter: ['text', 'json-summary', 'lcov'],
      reportsDirectory: process.env.TEST_UNDECLARED_OUTPUTS_DIR
        ? join(process.env.TEST_UNDECLARED_OUTPUTS_DIR, 'coverage')
        : 'coverage',
    },
  },
});
