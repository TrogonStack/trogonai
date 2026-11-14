import { realpathSync } from 'node:fs';
import { dirname } from 'node:path';

import tailwindcss from '@tailwindcss/vite';
import { devtools } from '@tanstack/devtools-vite';
import { tanstackRouter } from '@tanstack/router-plugin/vite';
import viteReact from '@vitejs/plugin-react';
import { configDefaults, defineConfig } from 'vitest/config';

// Vite and Node must load dependencies through the same canonical tree.
const bazelRoot = process.env.VITE_BAZEL_RUNFILES === '1' ? dirname(realpathSync('vite.config.ts')) : undefined;

const config = defineConfig({
  root: bazelRoot,
  resolve: { tsconfigPaths: true },
  plugins: [devtools(), tailwindcss(), tanstackRouter({ target: 'react', autoCodeSplitting: true }), viteReact()],
  test: {
    environment: 'jsdom',
    setupFiles: ['./src/test/setup.ts'],
    testTimeout: 20_000,
    // The output tree also contains Bazel's generated mirror of this test's inputs.
    exclude: [...configDefaults.exclude, ...(bazelRoot ? ['**/*.runfiles/**'] : [])],
  },
});

export default config;
