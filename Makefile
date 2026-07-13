.PHONY: all build release fmt fmt-check lint test check run-review run-dry-run run-print-diff clean

all: check

build:
	cargo build

release:
	cargo build --release

fmt:
	cargo fmt

fmt-check:
	cargo fmt --check

lint:
	cargo clippy

test:
	cargo test

check: fmt-check lint build test

run-review:
	cargo run -- review

run-dry-run:
	cargo run -- dry-run

run-print-diff:
	cargo run -- print-diff

clean:
	cargo clean
