#   make build      release build
#   make test       unit, property, and snapshot tests (skips the #[ignore] ones that cost money)
#   make lint       formatting, clippy, docs, dependency and workflow checks; run before every commit
#   make ci         everything CI runs: lint, test, the MSRV build, and the binary size budget
#   make fmt        reformat the code in place
#   make coverage   test coverage report, opened in a browser
#   make mutants    mutation testing: code changes the tests don't catch (slow; run now and then)
#   make setup      install the lint tools and the git pre-commit hook

LINT_TOOLS := cargo-nextest cargo-llvm-cov cargo-deny cargo-machete cargo-insta cargo-mutants zizmor

# The oldest Rust that must build the app, from `rust-version` in Cargo.toml.
MSRV := $(shell sed -n 's/^rust-version = "\(.*\)"/\1/p' Cargo.toml)

# The release binary may not grow past this. When it must, raise it in its own
# commit that says why. It was 14,481 KB when set.
MAX_BINARY_KB := 17000

.PHONY: build clean test lint ci fmt fmt-check clippy doc deny machete zizmor msrv size coverage mutants setup

build:
	cargo build --release --locked

clean:
	cargo clean

# nextest runs each test in its own process, which the env-var test in
# agent/runner.rs relies on to be sound.
test:
	cargo nextest run --locked

lint: fmt-check clippy doc deny machete zizmor

ci: lint test msrv size

fmt:
	cargo fmt

fmt-check:
	cargo fmt --check

clippy:
	cargo clippy --all-targets --all-features --locked -- -D warnings

# Warnings in doc comments, such as broken links, fail the build.
doc:
	RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --locked

deny:
	cargo deny --locked check

machete:
	cargo machete

# Audits the GitHub Actions workflows for injection, over-broad permissions,
# and unpinned actions. Set GH_TOKEN to also run the audits that query GitHub.
zizmor:
	zizmor .github

msrv:
	rustup toolchain install $(MSRV) --profile minimal
	cargo +$(MSRV) check --all-targets --locked

size: build
	@kb=$$(( $$(wc -c < target/release/docket) / 1024 )); \
	echo "release binary: $$kb KB (budget $(MAX_BINARY_KB) KB)"; \
	if [ $$kb -gt $(MAX_BINARY_KB) ]; then \
		echo "over budget; see MAX_BINARY_KB in the Makefile" >&2; exit 1; \
	fi

coverage:
	cargo llvm-cov nextest --locked --html --open

mutants:
	cargo mutants --test-tool nextest

setup:
	rustup component add llvm-tools-preview
	cargo install --locked $(LINT_TOOLS)
	git config core.hooksPath .githooks
