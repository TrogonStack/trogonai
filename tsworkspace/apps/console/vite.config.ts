import { realpathSync } from 'node:fs';
import { dirname, join } from 'node:path';

import tailwindcss from '@tailwindcss/vite';
import { devtools } from '@tanstack/devtools-vite';
import { tanstackRouter } from '@tanstack/router-plugin/vite';
import viteReact from '@vitejs/plugin-react';
import type { Plugin } from 'vite';
import { defineConfig } from 'vitest/config';

// Bazel runs the tests from a runfiles tree of symlinks. Following a symlink
// leaves that tree, and Vite then cannot load the file it resolved, so the
// Bazel test target keeps the symlinked paths instead.
const preserveSymlinks = process.env.VITE_PRESERVE_SYMLINKS === '1';

// Vitest resolves setupFiles through a resolver that always follows symlinks,
// so under Bazel the setup file arrives as its real output tree path, outside
// the runfiles root. Vite refuses to load files outside the root, and loading
// it from there would also give its imports different module ids than the
// tests get. Mapping the real package directory back onto the root keeps
// every project file in the one tree the tests run from.
function runfilesRoot(): Plugin {
  let root = '';
  let realRoot = '';
  return {
    name: 'runfiles-root',
    enforce: 'pre',
    configResolved(config) {
      root = config.root;
      realRoot = dirname(realpathSync(join(root, 'package.json')));
    },
    resolveId(id) {
      if (realRoot === root) return null;
      const path = id.startsWith('/@fs/') ? id.slice('/@fs'.length) : id;
      if (!path.startsWith(`${realRoot}/`)) return null;
      return root + path.slice(realRoot.length);
    },
  };
}

const config = defineConfig({
  resolve: { tsconfigPaths: true, preserveSymlinks },
  plugins: [
    ...(preserveSymlinks ? [runfilesRoot()] : []),
    devtools(),
    tailwindcss(),
    tanstackRouter({ target: 'react', autoCodeSplitting: true }),
    viteReact(),
  ],
  test: {
    environment: 'jsdom',
    setupFiles: ['./src/test/setup.ts'],
    testTimeout: 20_000,
  },
});

export default config;
