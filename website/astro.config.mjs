// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import starlightLinksValidator from 'starlight-links-validator';

// Served from GitHub Pages at https://kj-aiml.github.io/memctrl/, so every page and every
// absolute link starts with /memctrl/.
export default defineConfig({
  site: 'https://kj-aiml.github.io',
  base: '/memctrl',
  trailingSlash: 'always',
  integrations: [
    starlight({
      title: 'memctrl docs',
      description: 'Local, explainable memory for coding agents.',
      favicon: '/favicon.svg',
      social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/KJ-AIML/memctrl' }],
      editLink: { baseUrl: 'https://github.com/KJ-AIML/memctrl/edit/main/website/' },
      lastUpdated: true,
      pagination: true,
      tableOfContents: { minHeadingLevel: 2, maxHeadingLevel: 3 },
      customCss: ['@fontsource-variable/plus-jakarta-sans', '@fontsource-variable/geist-mono', './src/styles/custom.css', './src/styles/home.css'],
      components: {
        PageTitle: './src/components/PageTitle.astro',
        SiteTitle: './src/components/SiteTitle.astro',
        ThemeProvider: './src/components/ThemeProvider.astro',
        ThemeSelect: './src/components/ThemeSelect.astro',
      },
      expressiveCode: {
        themes: ['tokyo-night'],
        styleOverrides: {
          borderRadius: '0.85rem',
          borderColor: 'var(--mc-border)',
          codeFontFamily: "'Geist Mono Variable', ui-monospace, monospace",
          uiFontFamily: "'Plus Jakarta Sans Variable', ui-sans-serif, system-ui, sans-serif",
          codeFontSize: '0.84rem',
          codeLineHeight: '1.7',
          frames: {
            editorBackground: 'var(--mc-code-bg)',
            terminalBackground: 'var(--mc-code-bg)',
            editorTabBarBackground: 'var(--mc-code-bar)',
            editorActiveTabBackground: 'var(--mc-code-bg)',
            editorActiveTabIndicatorTopColor: 'var(--mc-violet)',
            editorTabBarBorderBottomColor: 'var(--mc-border)',
            terminalTitlebarBackground: 'var(--mc-code-bar)',
            terminalTitlebarBorderBottomColor: 'var(--mc-border)',
            terminalTitlebarDotsOpacity: '0.3',
            frameBoxShadowCssValue: 'none',
          },
        },
      },
      plugins: [starlightLinksValidator({ errorOnRelativeLinks: true })],
      sidebar: [
        {
          label: 'Get started',
          items: [
            { label: 'What is memctrl', slug: 'get-started/overview' },
            { slug: 'get-started/quick-start' },
            { slug: 'get-started/install' },
          ],
        },
        {
          label: 'Connect your agent',
          items: [
            { slug: 'agents/claude-code' },
            { slug: 'agents/codex-and-gemini' },
            { slug: 'agents/codex-and-cursor' },
            { slug: 'agents/mcp' },
          ],
        },
        {
          label: 'Concepts',
          items: [
            { slug: 'concepts/how-it-works' },
            { slug: 'concepts/memories' },
            { slug: 'concepts/session-brief' },
            { slug: 'concepts/recall' },
            { slug: 'concepts/search' },
            { slug: 'concepts/search-by-meaning' },
            { slug: 'concepts/citations' },
            { slug: 'concepts/guard-and-trust' },
          ],
        },
        {
          label: 'Guides',
          items: [
            { slug: 'guides/write-good-memories' },
            { slug: 'guides/review-candidates' },
            { slug: 'guides/correct-and-retire' },
            { slug: 'guides/keep-memories-fresh' },
            { slug: 'guides/share-with-your-team' },
            { slug: 'guides/set-up-search-by-meaning' },
            { slug: 'guides/other-languages' },
            { slug: 'guides/measure-retrieval' },
            { slug: 'guides/llm-providers' },
          ],
        },
        {
          label: 'Reference',
          items: [
            { label: 'CLI commands', collapsed: true, items: [{ autogenerate: { directory: 'reference/cli' } }] },
            { slug: 'reference/memoryrc' },
            { slug: 'reference/user-config' },
            { slug: 'reference/environment-variables' },
            { slug: 'reference/mcp-tools' },
            { slug: 'reference/hooks' },
            { slug: 'reference/json-output' },
            { slug: 'reference/memory-md' },
            { slug: 'reference/storage' },
          ],
        },
        { label: 'Security', items: [{ slug: 'security/model' }] },
        {
          label: 'Benchmarks',
          collapsed: true,
          items: [
            { slug: 'benchmarks/overview' },
            { slug: 'benchmarks/release-2-6' },
            { slug: 'benchmarks/agent-benchmark' },
            { slug: 'benchmarks/brief-authority' },
            { slug: 'benchmarks/reworded-tasks' },
            { slug: 'benchmarks/search-by-meaning' },
            { slug: 'benchmarks/retrieval-comparison' },
          ],
        },
        {
          label: 'Releases',
          items: [{ slug: 'releases/changelog' }, { slug: 'releases/upgrading' }],
        },
        {
          label: 'Help',
          items: [{ slug: 'help/troubleshooting' }, { slug: 'help/limits' }, { slug: 'help/faq' }],
        },
      ],
    }),
  ],
});
