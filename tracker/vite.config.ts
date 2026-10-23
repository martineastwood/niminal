import { defineConfig } from 'vite';

// The ready-made instruments and effects are read from the language's own library, one level up.
export default defineConfig({ server: { fs: { allow: ['..'] } } });
