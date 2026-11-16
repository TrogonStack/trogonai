import { realpathSync } from 'node:fs';
import { dirname } from 'node:path';

import tailwindcss from '@tailwindcss/vite';
import { devtools } from '@tanstack/devtools-vite';
import { tanstackRouter } from '@tanstack/router-plugin/vite';
import viteReact from '@vitejs/plugin-react';
import { configDefaults, defineConfig } from 'vitest/config';

// Vite and Node must load dependencies through the same canonical tree.
const bazelRoot = process.env.VITE_BAZEL_RUNFILES === '1' ? dirname(realpathSync('vite.config.ts')) : undefined;

// Bazel's declared dependencies can resolve outside the Vite root through runfiles symlinks.
const allowOutsideRoot = process.env.VITE_ALLOW_OUTSIDE_ROOT === '1';

// Split route chunks load on first navigation, so under Vitest that load lands inside a test's clock.
const autoCodeSplitting = process.env.VITEST === undefined;

const config = defineConfig({
  root: bazelRoot,
  resolve: { tsconfigPaths: true },
  server: { fs: { strict: !allowOutsideRoot } },
  plugins: [devtools(), tailwindcss(), tanstackRouter({ target: 'react', autoCodeSplitting }), viteReact()],
  test: {
    environment: 'jsdom',
    setupFiles: ['./src/test/setup.ts'],
    testTimeout: 20_000,
    // The output tree also contains Bazel's generated mirror of this test's inputs.
    exclude: [...configDefaults.exclude, ...(bazelRoot ? ['**/*.runfiles/**'] : [])],
  },
});

export default config;
