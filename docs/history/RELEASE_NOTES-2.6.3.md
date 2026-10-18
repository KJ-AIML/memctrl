# MemCtrl 2.6.3: fixes found while writing the docs

Writing the [documentation site](https://kj-aiml.github.io/memctrl/) meant trying every setting the way it's described. Two of the problems that turned up could send data where it shouldn't go; the rest made memctrl harder to live with. Each fix has a test.

## Security fixes

- **A `compatible` server gets only the key you name.** The `compatible` LLM provider fell back to `OPENAI_API_KEY` and sent it to whatever `--llm-base-url` you gave. It now takes a key only from `--llm-api-key-env NAME` or `MEMCTRL_LLM_API_KEY`. Local servers still work without one; a remote one without a key is refused before anything is sent.
- **Thai national ids are redacted in every common form.** Only the dashed form (`1-2345-67890-12-1`) was recognized. Now the spaced form on the card (`1 2345 67890 12 1`) is too, and 13 digits in a row (`1234567890121`) when the last digit is the id's check digit. Thai text may touch the id; digits inside a name or path stay as written.
- **`SECURITY.md`** says how to report a vulnerability privately and what counts as one.

## Fixes

- **Session records stay out of git.** The hooks' records in `.memctrl/sessions/` showed up in `git status`, one file per Claude Code session. `.memctrl/.gitignore` now lists `sessions/`, and the folder gets its own `.gitignore` with the first record, which covers projects set up by an older memctrl, since `init` keeps an existing `.memctrl/.gitignore`.
- **Your user config applies over the init template, looser or stricter.** `memctrl init` writes the built-in guard values (`mode = "flag"`, `trusted_sources = ["manual", "user", "cli"]`) into every `.memoryrc`. Since 2.6.1 a stricter user config replaced them quietly, but a looser one, such as `mode = "off"`, was held at the built-in values in every such repository. A `.memoryrc` that holds exactly the built-in values now yields to your config either way. A repository that sets any other value keeps the stricter one, and one you trust still applies its file in full.
- **A mistake in `[semantic]` no longer stops every command.** A typo in your user config's `[semantic]` made every command fail, the Claude Code hooks included, so agents lost their memory until you noticed. Search by meaning is optional, so memctrl now turns it off with a warning and keeps going. `memctrl semantic status` shows why. Mistakes under `[guard]` and `[trust]` still stop every command.
- **`install --hook` keeps the flags you added.** Running it again replaced the hook command and dropped flags such as `--budget 1500`. It now updates only how the hook starts memctrl. It also recognizes a hook with a flag before `--format hook`, which used to get a second copy.
- **The MCP server finds citations from `--rc`.** It looked for the project's `.git` from the folder the client started it in, so citations read `missing` unless you set `MEMCTRL_PROJECT_ROOT`. It now looks from the `--rc` file's folder.
- **Clearer warnings.** A setting a repository can't loosen used to suggest putting it in your user config even when the user config can't hold it (`db_path`, `[citations] root`, `[forget] never`, triggers). Each warning now says what would allow it. `import --accept` says "1 entry", not "1 entries".

## Release builds

The release workflow uses the Node 24 versions of the artifact actions, and pull requests now collect and check the archives the way a tag does, so a change in how artifacts download shows up before a release.

## Upgrade notes

See the 2.6.2 → 2.6.3 section of [MIGRATION.md](../../MIGRATION.md). Two changes can affect a working setup:

- A script that relied on `compatible` reading `OPENAI_API_KEY` for a remote server now stops with "LLM API key is required for non-loopback endpoints". Pass `--llm-api-key-env OPENAI_API_KEY` if that's the key you mean to send.
- If your user config sets `mode = "off"` and you relied on a repository's template lines to keep the guard on there, set `mode = "strict"` in that repository or change your user config.

Nothing in the database or the brief changes.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/KJ-AIML/memctrl/main/install.sh | sh
```

Windows: download `memctrl-x86_64-pc-windows-msvc.zip` from the release and put `memctrl.exe` on your `PATH`.

Previous release notes live in [docs/history](.).
