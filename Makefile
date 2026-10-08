.PHONY: build test lint fmt install
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
