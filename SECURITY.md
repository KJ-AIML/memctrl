# Security policy

memctrl replays memories into every agent session, so a flaw in it can steer an agent long after the bad write. The [security model](https://kj-aiml.github.io/memctrl/security/model/) describes what memctrl defends against and where its limits are.

## Supported versions

Fixes go into the latest release. Upgrade to it before reporting, if you can.

## Report a vulnerability

Report privately through [GitHub's private vulnerability reporting](https://github.com/KJ-AIML/memctrl/security/advisories/new). Please don't open a public issue, pull request or discussion with the details.

If that page isn't available, open an [issue](https://github.com/KJ-AIML/memctrl/issues) that asks for a private contact and leave the details out.

Include:

- the memctrl version (`memctrl --version`) and your platform;
- the smallest steps that reproduce it, such as a `.memoryrc`, a `memory.md` or the commands you ran;
- what an attacker gains, and what they need first (push access to a repository you clone, a tool output your agent reads, a shell on your machine).

## What counts

These are vulnerabilities:

- An agent, a tool output, an import or a teammate's `memory.md` releasing a memory the guard held, or skipping the person who has to release it.
- A repository's `.memoryrc`, database or symlinks loosening a setting your user config controls, or reading or writing outside the project.
- memctrl sending memory text, prompts or keys over the network when you didn't turn that on, or to a host you didn't name.
- A secret or personal data in a format the security model lists ending up stored, exported or printed unredacted.

These are known limits, not vulnerabilities:

- Text that gets past the guard's patterns. The guard is a tripwire: what protects you is that a person releases what it holds.
- An agent acting on what a memory says. The brief asks agents to treat memories as notes, and that's a request.
- Secrets written in prose or in formats memctrl doesn't recognize.
- An agent passing `--source user` or `--layer user` to write as you. memctrl doesn't record who typed a memory.
- Anything that needs write access to your user config, your local database or the memctrl binary.

If you aren't sure, report it privately anyway.
