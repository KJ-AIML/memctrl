# Distribution

Build/install from this source branch using `cargo install --path . --locked` or `cargo build --release --locked`. Cargo.lock is committed. There is no Python runtime dependency, PyPI workflow, or compatibility bridge.

The CI matrix builds and tests Linux, macOS and Windows. This change does not publish crates.io packages, sign binaries, create releases, deploy services, or merge its PR.
