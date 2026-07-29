# Use Ghostty's emulator and vendor herdr's detection byte-for-byte

Gaze detects agent state (Idle/Working/Blocked/Unknown) by evaluating herdr's
manifest-driven detect engine against a rendered screen. The manifest region
semantics (`bottom_non_empty_lines(n)`, `after_last_horizontal_rule`,
`osc_title`, …) were tuned against the screen herdr sees, so any drift in
either the emulator or the engine silently degrades detection. herdr is
binary-only (no `lib.rs`, nothing on crates.io), so its code can only be
reused by copying.

The screen is produced by a *shadow terminal*: a headless emulator fed a copy
of the agent's PTY output. The wrapper passes the original bytes through
unmodified, never answers terminal queries (the real terminal does), and keeps
the shadow's dimensions identical to the real terminal at all times.

## What we vendor, and what we do not

herdr's `src/terminal/` is not an emulator — it is terminal state and
effective-state arbitration. The emulator underneath is **libghostty-vt**,
Ghostty's VT library (MIT): 19MB of Zig that herdr vendors and builds via
`zig build`, wrapped in 8.2k lines of herdr FFI bindings.

So gaze splits the difference. Emulation comes from the `libghostty-vt` crate
(MIT/Apache-2.0) — the same library herdr uses, through a maintained Rust
wrapper — rather than vendoring herdr's bindings and Zig tree. Detection and
arbitration are vendored from herdr verbatim. Zig 0.15.2 is a hard build
dependency either way, pinned in `.mise.toml`; the alternative was a
pure-Rust VT crate, rejected because emulator drift is exactly what this
decision exists to prevent.

We also skip herdr's `terminal/runtime.rs` and `runtime_registry.rs`: they
spawn and manage multiplexer panes, and gaze spawns its own PTY. That cut
falls along an existing seam — those two files hold all of the module's
dependencies on herdr's pane, layout, and input plumbing, while `state.rs` and
`metadata.rs` need only `detect`, `agent_resume`, and `metadata_tokens`.

`ShadowTerminal::detection_text` reproduces herdr's `ghostty_recent_text`
exactly — same 24-row window, same per-row reads, same trailing-blank
trimming — because that string is what the manifests were written against.

## Consequence: hooks connect to the Wrapper, not the Daemon

Herdr centralizes *effective state arbitration* — reconciling screen-derived
detection with Integration-hook-reported state — in `src/terminal/state.rs`
(`HookAuthority`, `set_detected_state_with_screen_signals_at`,
`set_hook_authority*`, emitting `EffectiveStateChange`). That file is part of
the vendored terminal module, and it requires both signal sources as inputs to
the same object. Splitting them across processes would mean reimplementing the
arbitration state machine, so hooks must reach the process that owns it.

Each Wrapper therefore listens on its own socket
(`$XDG_RUNTIME_DIR/gaze/agents/<id>.sock`, unlinked on exit) and injects
`GAZE_ENV=1`, `GAZE_SOCKET_PATH=<that socket>`, `GAZE_AGENT_ID=<registry id>`
into the agent's environment — mirroring herdr's `HERDR_ENV` /
`HERDR_SOCKET_PATH` / `HERDR_PANE_ID` contract exactly. Hooks report via
`agent.report_state` and `agent.report_session` (herdr's `pane.report_agent`,
`pane.report_agent_session`). The Wrapper reports only the arbitrated effective
state onward, leaving the Daemon a dumb registry and fan-out. This costs
nothing in hook-asset fidelity: the assets only read a socket path from the
environment and do not care what listens on it. Distinct env var names and
hook filenames also let gaze and herdr coexist on one machine.

## Provenance

Lifted from https://github.com/ogulcancelik/herdr (Apache-2.0) at commit
`514e4465ee33d4d81682d06ee2934483982a54ed` (2026-07-29): `src/detect/` (engine,
manifests, updater) → `crates/gaze-detect`; `src/terminal/{state,metadata,id,
title}.rs`, `src/agent_resume.rs`, `src/metadata_tokens.rs` →
`crates/gaze-term`. `scripts/revendor.sh` is the only thing that writes those
files; see ADR-0005 and NOTICE.

## Consequences

Building gaze requires Zig 0.15.2, which `cargo install` users must have.
Prebuilt release binaries are unaffected. herdr has the same property, so this
is not a regression against the tool we are modelling.

Splitting detection and terminal state into two crates puts a crate boundary
where herdr had none, so items herdr declared `pub(crate)` are not visible to
the vendored code that uses them. That is handled by a patch widening two
functions to `pub` (`vendor/patches/0002`), not by restructuring — but it is a
recurring cost to watch at each re-vendor.

gaze also does not vendor herdr's per-platform process scanning: it identifies
an agent from the argv it was given, so `crate::platform`'s foreground-job
lookups are stubbed and the three upstream tests covering them are marked
ignored (`vendor/patches/0001`).
