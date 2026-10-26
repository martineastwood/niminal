/// <reference types="vitest/config" />
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';

// The shared editor code in ../editors/codemirror uses this app's copies of the editor libraries.
const modules = (name: string) => fileURLToPath(new URL(`./node_modules/${name}`, import.meta.url));
const alias = { '@codemirror/language': modules('@codemirror/language'), '@lezer/highlight': modules('@lezer/highlight') };

// The ready-made instruments and effects are read from the language's own library, one level up.
// The pattern grid tests build and click through a whole grid in a simulated browser, which is slow on a busy machine.
export default defineConfig({ resolve: { alias }, server: { fs: { allow: ['..'] } }, test: { testTimeout: 30000, execArgv: ['--max-old-space-size=8192'] } });
