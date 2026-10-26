/// <reference types="vitest/config" />
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';

// The shared editor code in ../editors/codemirror uses this app's copies of the editor libraries.
const modules = (name: string) => fileURLToPath(new URL(`./node_modules/${name}`, import.meta.url));
const alias = { '@codemirror/language': modules('@codemirror/language'), '@lezer/highlight': modules('@lezer/highlight') };

export default defineConfig({
  resolve: { alias },
  server: { fs: { allow: ['..'] } },
  test: { environment: 'happy-dom', testTimeout: 30000 },
});
