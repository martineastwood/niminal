// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import starlightThemeBlack from 'starlight-theme-black';

export default defineConfig({
	site: 'https://niminal.dev',
	integrations: [
		starlight({
			title: 'Niminal',
			description: 'A Nim stack for building AI agents, terminal interfaces, and MCP servers.',
			customCss: ['./src/styles/sidebar.css'],
			social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/martineastwood' }],
			sidebar: [
				{ label: 'Niminal', slug: 'index' },
				{
					label: 'nimgent',
					items: [
						{ label: 'Introduction', slug: 'nimgent' },
						{ label: 'Quickstart', slug: 'nimgent/guides/quickstart' },
						{ label: 'Providers', slug: 'nimgent/guides/providers' },
						{ label: 'Streaming', slug: 'nimgent/guides/streaming' },
						{ label: 'Tools and agents', slug: 'nimgent/guides/tools-and-agents' },
						{ label: 'Structured output', slug: 'nimgent/guides/structured-output' },
						{ label: 'Sessions', slug: 'nimgent/guides/sessions' },
						{ label: 'Core API', slug: 'nimgent/reference/core-api' },
						{ label: 'Examples', slug: 'nimgent/reference/examples' },
					],
				},
				{ label: 'nimterm', slug: 'nimterm' },
				{ label: 'nimwire', slug: 'nimwire' },
				{ label: 'nimlet', slug: 'nimlet' },
			],
			plugins: [
				starlightThemeBlack({
					navLinks: [
						{ label: 'GitHub', link: 'https://github.com/martineastwood' },
					],
					docs: { showMarkdownActions: false },
				}),
			],
		}),
	],
});
