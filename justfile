default:
    @just --list

build:
    cargo build --workspace

# One file is the whole install: the daemon and the wrapper are subcommands of
# the same executable, so there is nothing else to place and no service to
# enable — amond starts on demand. ~/.local/bin because Omarchy already has it
# on PATH, where its own agent launchers live.
#
# Build a release amon into ~/.local/bin (override with AMON_PREFIX)
install:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build --workspace --release
    prefix="${AMON_PREFIX:-$HOME/.local/bin}"
    mkdir -p "$prefix"
    # to a temp name first, then rename: installing over a running amon would
    # otherwise fail with ETXTBSY while an agent is wrapped
    install -m 755 target/release/amon "$prefix/.amon.new"
    mv -f "$prefix/.amon.new" "$prefix/amon"
    echo "installed $prefix/amon"
    case ":${PATH}:" in
      *":$prefix:"*) ;;
      *) echo "warning: $prefix is not on PATH — add it before running amon setup" ;;
    esac
    echo "next: amon setup"

# Integrations come out first, so no hook, alias or bar widget is left pointing
# at something that is gone.
#
# Remove the integrations, then the binary
uninstall:
    #!/usr/bin/env bash
    set -euo pipefail
    prefix="${AMON_PREFIX:-$HOME/.local/bin}"
    [ -x "$prefix/amon" ] && "$prefix/amon" remove --all || true
    rm -f "$prefix/amon"
    echo "removed $prefix/amon"

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

# Serve the website locally. Plain static files — no build step, just reload.
site port="8080":
    @echo "http://127.0.0.1:{{port}}"
    python3 -m http.server {{port}} --bind 127.0.0.1 -d website

# Regenerate docs/protocol.schema.json after an intentional protocol change.
schema:
    UPDATE_SCHEMA=1 cargo test -p amon-protocol --test schema

# Re-derive every vendored file from herdr. Bump HERDR_COMMIT in the script to
# move to a newer upstream, then review the diff.
revendor:
    ./scripts/revendor.sh
