# memctrl docs site

The documentation site at https://kj-aiml.github.io/memctrl/, built with [Starlight](https://starlight.astro.build) (Astro). Pages are MDX files in `src/content/docs/`; the sidebar is defined in `astro.config.mjs`.

```bash
cd website
npm ci
npm run dev        # http://localhost:4321/memctrl/
npm run build      # builds dist/ and fails on any broken internal link
```

## The CLI reference is generated

`src/content/docs/reference/cli/` is written by `scripts/gen-cli.mjs` from the binary's own `--help`, so it can't drift from the CLI. Regenerate it after changing a command or an option:

```bash
cargo build --release
node scripts/gen-cli.mjs ../target/release/memctrl
```

Options whose help text is empty in the binary get their description from the `FILL` table in the script. Prefer adding help text to the clap definition in `src/main.rs`.

## Writing pages

- **Accuracy first.** Every command, option, default, path and number must come from the code or the existing docs: `README.md`, `MIGRATION.md`, `RELEASE_NOTES.md`, `docs/history/`, `docs/benchmarks/`, the generated CLI reference, or `src/`. When in doubt, run the binary in a scratch folder. Leave out what you can't verify.
- **One topic per page, explained once.** Concepts explain how something works; guides walk through a task; reference lists every option. Link to the page that owns a topic instead of repeating it.
- **Start with what the reader gets.** One or two sentences on what the page covers and why it matters, then `##` sections (they make the "On this page" list) and `###` for steps inside them.
- **Plain and direct.** Second person, present tense, short sentences, no marketing language or emoji.
- **Links are absolute and include the base path:** `/memctrl/concepts/session-brief/`, with the trailing slash. Commands link to `/memctrl/reference/cli/<command>/`. The build fails on a broken link or anchor.
- **Code blocks name their language**, and files get a title: ` ```toml title="~/.config/memctrl/config.toml" `.
- **Components** come from `@astrojs/starlight/components`: `Aside` (note, tip, caution, danger), `Steps`, `Tabs`/`TabItem`, `Card`/`CardGrid`, `LinkCard`, `FileTree` and `Badge`.
- **Tables** compare options with a column that says why or when, not only names.

## Design

Charcoal with a dot grid, white type and a periwinkle accent, after the brand references the maintainer chose: periwinkle `#8B93FF`, soft periwinkle `#A9AFFF`, lavender `#C9CCFF` and mist `#E9EAFF` on charcoal `#19191B`. Type is Plus Jakarta Sans, with Geist Mono for code (both OFL, self-hosted through Fontsource).

- The site has one theme: there is no light theme or theme picker. `src/styles/custom.css` holds every color as a token, and `src/styles/home.css` styles the home page.
- `src/components/` overrides Starlight parts: `SiteTitle` (the mark and wordmark), `PageTitle` (breadcrumbs), `ThemeProvider` and `ThemeSelect` (one theme, no picker), plus `Home` (the home page, used by `index.mdx`) and `NotFound` (the 404 page, used by `404.mdx`).
- `src/scripts/pixel-field.ts` runs the hero's pixel avalanche (gravity, sliding, pointer push and click). `src/scripts/reveal.ts` reveals home sections as they scroll in. Both draw everything settled when the reader asks for reduced motion.
- The home page's benchmark chart uses `#8088F7` for memctrl and `#646472` for other setups, checked for contrast and color-blind separation on the charcoal background.
- Code blocks use the `tokyo-night` theme. Tag program output as `text`, not `md`.

## Deploying

`.github/workflows/docs.yml` builds the site for every pull request and push to `main` that touches `website/`, so a broken link fails the check. It publishes to GitHub Pages only once Pages is enabled (source: GitHub Actions) and the repository variable `DOCS_DEPLOY` is `true`.
