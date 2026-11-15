import tailwindcss from '@tailwindcss/vite';
import { devtools } from '@tanstack/devtools-vite';
import { tanstackRouter } from '@tanstack/router-plugin/vite';
import viteReact from '@vitejs/plugin-react';
import { defineConfig } from 'vitest/config';

// Bazel runs the tests from a runfiles tree of symlinks. Vite follows them to
// the real files, which sit outside its root, and by default refuses to load
// anything there, so the Bazel test target lifts that restriction.
const allowOutsideRoot = process.env.VITE_ALLOW_OUTSIDE_ROOT === '1';

const config = defineConfig({
  resolve: { tsconfigPaths: true },
  server: { fs: { strict: !allowOutsideRoot } },
  plugins: [devtools(), tailwindcss(), tanstackRouter({ target: 'react', autoCodeSplitting: true }), viteReact()],
  test: {
    environment: 'jsdom',
    setupFiles: ['./src/test/setup.ts'],
    testTimeout: 20_000,
  },
});

export default config;
