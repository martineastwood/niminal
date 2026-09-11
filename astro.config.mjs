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
					label: 'nimlet',
					items: [
						{ label: 'Introduction', slug: 'nimlet' },
						{ label: 'Quickstart', slug: 'nimlet/guides/quickstart' },
						{ label: 'Configuration', slug: 'nimlet/guides/configuration' },
						{ label: 'Interactive TUI', slug: 'nimlet/guides/interactive-tui' },
						{ label: 'Plan and act mode', slug: 'nimlet/guides/plan-and-act' },
						{ label: 'Permissions', slug: 'nimlet/guides/permissions' },
						{ label: 'Sessions', slug: 'nimlet/guides/sessions' },
						{ label: 'Context and compaction', slug: 'nimlet/guides/context-and-compaction' },
						{ label: 'Models and providers', slug: 'nimlet/guides/models-and-providers' },
						{ label: 'Instructions', slug: 'nimlet/guides/instructions' },
						{ label: 'Skills', slug: 'nimlet/guides/skills' },
						{ label: 'Prompt templates', slug: 'nimlet/guides/prompt-templates' },
						{ label: 'External tools', slug: 'nimlet/guides/external-tools' },
						{ label: 'Extensions and hooks', slug: 'nimlet/guides/extensions-and-hooks' },
						{ label: 'Built-in tools', slug: 'nimlet/reference/tools' },
						{ label: 'Commands and shortcuts', slug: 'nimlet/reference/commands' },
						{ label: 'JSON mode', slug: 'nimlet/reference/json-mode' },
						{ label: 'RPC mode', slug: 'nimlet/reference/rpc-mode' },
						{ label: 'Files and directories', slug: 'nimlet/reference/files-and-directories' },
						{ label: 'Architecture', slug: 'nimlet/reference/architecture' },
					],
				},
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
