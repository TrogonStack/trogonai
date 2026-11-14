import tailwindcss from '@tailwindcss/vite';
import { devtools } from '@tanstack/devtools-vite';
import { tanstackRouter } from '@tanstack/router-plugin/vite';
import viteReact from '@vitejs/plugin-react';
import { defineConfig } from 'vitest/config';

// Bazel runs the tests from a runfiles tree of symlinks. Following a symlink
// leaves that tree, and Vite then cannot load the file it resolved, so the
// Bazel test target keeps the symlinked paths instead.
const preserveSymlinks = process.env.VITE_PRESERVE_SYMLINKS === '1';

const config = defineConfig({
  resolve: { tsconfigPaths: true, preserveSymlinks },
  plugins: [devtools(), tailwindcss(), tanstackRouter({ target: 'react', autoCodeSplitting: true }), viteReact()],
  test: {
    environment: 'jsdom',
    setupFiles: ['./src/test/setup.ts'],
    testTimeout: 20_000,
  },
});

export default config;
