import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import { readFileSync } from 'node:fs';

// One grammar for the editor extension and the docs.
const niminal = JSON.parse(
  readFileSync(new URL('../editors/vscode/syntaxes/niminal.tmLanguage.json', import.meta.url), 'utf8'),
);

export default defineConfig({
  site: 'https://niminal.dev',
  integrations: [
    starlight({
      title: 'niminal',
      description: 'A live-coding audio language with its own DSP engine.',
      social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/martineastwood/niminal' }],
      // Ink is the default ground. With nothing stored, Starlight would follow the OS.
      head: [
        { tag: 'link', attrs: { rel: 'preconnect', href: 'https://fonts.googleapis.com' } },
        { tag: 'link', attrs: { rel: 'preconnect', href: 'https://fonts.gstatic.com', crossorigin: '' } },
        { tag: 'link', attrs: { rel: 'stylesheet', href: 'https://fonts.googleapis.com/css2?family=Archivo:wght@600;800;900&family=Inter:wght@400;500;600&family=JetBrains+Mono:wght@400;500;700&display=swap' } },
        { tag: 'link', attrs: { rel: 'icon', href: '/favicon.svg', type: 'image/svg+xml' } },
        {
        tag: 'script',
        content: "try{if(!localStorage.getItem('starlight-theme')){localStorage.setItem('starlight-theme','dark');document.documentElement.dataset.theme='dark'}}catch(e){}",
        },
      ],
      customCss: ['./src/styles/theme.css'],
      expressiveCode: {
        shiki: { langs: [{ ...niminal, name: 'niminal', aliases: ['nml'] }] },
        themes: ['night-owl', 'github-light'],
      },
      sidebar: [
        { label: 'Guide', items: [
          { slug: 'guide/getting-started' },
          { slug: 'guide/the-pieces' },
          { slug: 'guide/live-coding' },
          { slug: 'guide/offline-rendering' },
        ] },
        { label: 'Language', items: [
          { slug: 'reference/language' },
          { slug: 'reference/language/syntax' },
          { slug: 'reference/language/units-and-rates' },
          { slug: 'reference/language/envelopes' },
          { slug: 'reference/language/instruments' },
          { slug: 'reference/language/samples' },
          { slug: 'reference/language/functions' },
          { slug: 'reference/language/tables' },
          { slug: 'reference/language/routing' },
          { slug: 'reference/language/patterns' },
          { slug: 'reference/language/performance' },
          { slug: 'reference/language/modules' },
          { slug: 'reference/language/standard-library' },
          { label: 'Scores (.nms)', slug: 'reference/score-model' },
          { slug: 'reference/reserved-words' },
          { slug: 'reference/opcodes' },
        ] },
        { label: 'niminal (server)', items: [
          { label: 'Command line', slug: 'reference/server/cli' },
          { label: 'Engine API', slug: 'reference/engine-api' },
        ] },
        { label: 'niminal-live (sequencer)', items: [
          { label: 'Command line', slug: 'reference/niminal-live/cli' },
          { label: 'Sequencer protocol', slug: 'reference/protocol' },
          { label: 'Editor and LSP', slug: 'reference/editor' },
        ] },
        { label: 'Tracker', items: [
          { label: 'Overview', slug: 'reference/tracker' },
          { label: 'The grid', slug: 'reference/tracker/grid' },
          { label: 'Patterns, sequence and timing', slug: 'reference/tracker/sequence' },
          { label: 'Instruments and effects', slug: 'reference/tracker/instruments' },
          { label: 'Track commands, automation and envelopes', slug: 'reference/tracker/automation' },
          { label: 'Mixer, returns and sends', slug: 'reference/tracker/mixer' },
          { label: 'Selecting and editing', slug: 'reference/tracker/editing' },
          { label: 'Interface, size and keyboard', slug: 'reference/tracker/interface' },
        ] },
      ],
    }),
  ],
});
