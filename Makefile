#   make build      release build
#   make test       unit, property, and snapshot tests (skips the #[ignore] ones that cost money)
#   make lint       formatting, clippy, docs, dependency checks; run before every commit
#   make fmt        reformat the code in place
#   make coverage   test coverage report, opened in a browser
#   make setup      install the lint tools and the git pre-commit hook

LINT_TOOLS := cargo-nextest cargo-llvm-cov cargo-deny cargo-machete cargo-insta

.PHONY: build clean test lint fmt fmt-check clippy doc deny machete coverage setup

build:
	cargo build --release

clean:
	cargo clean

# nextest runs each test in its own process, which the env-var test in
# agent/runner.rs relies on to be sound.
test:
	cargo nextest run

lint: fmt-check clippy doc deny machete

fmt:
	cargo fmt

fmt-check:
	cargo fmt --check

clippy:
	cargo clippy --all-targets -- -D warnings

# Warnings in doc comments, such as broken links, fail the build.
doc:
	RUSTDOCFLAGS="-D warnings" cargo doc --no-deps

deny:
	cargo deny check

machete:
	cargo machete

coverage:
	cargo llvm-cov nextest --html --open

setup:
	rustup component add clippy rustfmt llvm-tools-preview
	cargo install --locked $(LINT_TOOLS)
	git config core.hooksPath .githooks
