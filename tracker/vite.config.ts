/// <reference types="vitest/config" />
import { defineConfig } from 'vite';

// The ready-made instruments and effects are read from the language's own library, one level up.
// The pattern grid tests build and click through a whole grid in a simulated browser, which is slow on a busy machine.
export default defineConfig({ server: { fs: { allow: ['..'] } }, test: { testTimeout: 30000 } });
