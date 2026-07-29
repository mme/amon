default:
    @just --list

build:
    cargo build --workspace

test:
    cargo test --workspace

lint:
    cargo clippy --workspace --all-targets -- -D warnings
    cargo fmt --all --check

fmt:
    cargo fmt --all

# Regenerate docs/protocol.schema.json after an intentional protocol change.
schema:
    UPDATE_SCHEMA=1 cargo test -p gaze-protocol --test schema
