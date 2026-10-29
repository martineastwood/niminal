/// <reference types="vitest/config" />
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';

// The shared editor code in ../editors/codemirror uses this app's copies of the editor libraries.
const modules = (name: string) => fileURLToPath(new URL(`./node_modules/${name}`, import.meta.url));
const alias = Object.fromEntries(['@codemirror/language', '@codemirror/state', '@codemirror/view', '@codemirror/lsp-client', '@lezer/highlight'].map(name => [name, modules(name)]));

export default defineConfig({
  resolve: { alias },
  // src-tauri/target holds gigabytes of build output that the dev server must not watch
  server: { fs: { allow: ['..'] }, watch: { ignored: ['**/src-tauri/**'] } },
  test: { environment: 'happy-dom', testTimeout: 30000 },
});
