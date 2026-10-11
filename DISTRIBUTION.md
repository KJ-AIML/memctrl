# Distribution and releases

MemCtrl ships as a native executable. Users install it with:

- **Prebuilt binaries** from GitHub Releases, via `install.sh` (macOS/Linux, checksum-verified) or the Windows zip.
- **crates.io**, via `cargo install --locked memctrl`.
- **Source**, via `cargo install --path . --locked`.

There is no Python runtime dependency, PyPI package or compatibility bridge.

## How a release is built

`.github/workflows/release.yml` builds five targets: `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`, `aarch64-apple-darwin`, `x86_64-apple-darwin` and `x86_64-pc-windows-msvc`. Every target that can run on its build machine is smoke-tested with `--version` plus an `add` and a `query`.

- **Pull requests** that touch the release path build all targets and publish nothing.
- **A `vX.Y.Z` tag** builds the same artifacts, checks the tag matches `Cargo.toml`, writes `SHA256SUMS` and creates a **draft** GitHub release using `RELEASE_NOTES.md`.

## Maintainer checklist

1. Bump `version` in `Cargo.toml` and update `RELEASE_NOTES.md` and `MIGRATION.md`.
2. Run `make ci`, then merge to `main`.
3. Create an annotated tag on the merge commit (`vX.Y.Z`) and push it.
4. Review the draft release: all five archives plus `SHA256SUMS` are present. Then publish it.
5. Publish the crate with `cargo publish --locked` (check first with `cargo publish --locked --dry-run`).

## Post-release proof

The release is done only when these agree:

- The tag points at the merge commit, and `memctrl --version` from a release binary reports `X.Y.Z`.
- A fresh `install.sh` run installs `X.Y.Z` and its checksum verifies.
- `cargo install --locked memctrl` installs `X.Y.Z`.
- The crates.io page shows `X.Y.Z`.

## Rollback

1. Unpublish the GitHub release by turning it back into a draft.
2. Yank the crate version (`cargo yank --version X.Y.Z`).
3. 1.x users can stay on the Python release, and MIGRATION.md tells them to back up their databases before upgrading.
