# MemCtrl 2.6.1: quiet repositories under a stricter user config

A fix for the setting 2.6 recommends. If you narrow `[guard] trusted_sources` in your user config so that only memories you add yourself count as yours, every command in a repository set up by `memctrl init` warned that its `.memoryrc` tried to loosen your guard. That file only holds the built-in defaults, so it now yields to your config quietly.

## Fixes

- **No warning for the init template.** A `.memoryrc` whose `[guard]` settings are the built-in ones (`mode = "flag"`, `trusted_sources = ["manual", "user", "cli"]`) takes your stricter user config without a warning.
- **Still warned:** a repository that asks for more than the built-ins, such as `mode = "off"` or `trusted_sources` with `mcp`. Those settings are still ignored unless you trust the repository.

## 2.6 in short

- **Your preferences come first.** User-layer memories you saved are shown under "From the user", and the brief says to follow them unless the request says otherwise.
- **Recorded decisions wait for you.** If what's asked can't be done without going against a recorded decision, the agent names the memory and asks before changing anything.
- **Measured against 2.5.0 in the same rounds:** 29 of 32 standard tasks right (2.5.0: 28) and 13 of 16 reworded tasks (2.5.0: 10). [Full results](docs/benchmarks/brief-authority.md), [2.6.0 notes](docs/history/RELEASE_NOTES-2.6.0.md).

## Upgrade notes

See the 2.6.0 → 2.6.1 section of [MIGRATION.md](MIGRATION.md). Nothing in the database or the brief changes.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/KJ-AIML/memctrl/main/install.sh | sh
```

Windows: download `memctrl-x86_64-pc-windows-msvc.zip` from the release and put `memctrl.exe` on your `PATH`.

Previous release notes live in [docs/history](docs/history).
