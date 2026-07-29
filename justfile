default:
    @just --list

build:
    cargo build --workspace

# nextest, not `cargo test`: vendored tests mutate process-global env vars and
# need a process per test. See .mise.toml.
test:
    cargo nextest run --workspace --status-level fail
    cargo test --workspace --doc

lint:
    cargo clippy --workspace --all-targets -- -D warnings
    cargo fmt --all --check

fmt:
    cargo fmt --all

# Regenerate docs/protocol.schema.json after an intentional protocol change.
schema:
    UPDATE_SCHEMA=1 cargo test -p amon-protocol --test schema

# Re-derive every vendored file from herdr. Bump HERDR_COMMIT in the script to
# move to a newer upstream, then review the diff.
revendor:
    ./scripts/revendor.sh
