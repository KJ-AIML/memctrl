.PHONY: build test lint fmt install ci hooks
build:
	cargo build --locked --release
test:
	cargo test --locked --all-targets
lint:
	cargo clippy --locked --all-targets -- -D warnings
fmt:
	cargo fmt
install:
	cargo install --path . --locked
# The same checks GitHub CI runs. Run before pushing.
ci:
	cargo fmt --check
	cargo clippy --locked --all-targets -- -D warnings
	cargo test --locked --all-targets
	cargo build --locked --release --bin memctrl --examples
	@if git ls-files '*.py' '*.pyc' 'pyproject.toml' 'requirements*.txt' 'uv.lock' | grep -q .; then \
		echo "Python source or packaging found"; exit 1; \
	fi
# Opt in to running `make ci` before every push (bypass once with --no-verify).
hooks:
	git config core.hooksPath .githooks
