# Vendor herdr's terminal emulator and detect engine byte-for-byte

Gaze detects agent state (Idle/Working/Blocked/Unknown) by evaluating herdr's
manifest-driven detect engine against a rendered screen grid. Rather than
feeding an off-the-shelf VT crate (avt/vt100) into a ported engine, we vendor
both herdr's terminal emulator (`src/terminal/`) and its detect engine
(`src/detect/`, incl. all agent manifests) byte-for-byte, because the manifest
region semantics (`bottom_non_empty_lines(n)`, `after_last_horizontal_rule`,
`osc_title`, …) were tuned against herdr's grid and any emulation drift would
silently degrade detection. Herdr is binary-only (no `lib.rs`, no library on
crates.io), so source-copying is the only reuse option.

The vendored copy runs as a *shadow terminal*: it consumes a copy of the
agent's PTY output purely for detection. The wrapper passes original bytes
through unmodified, never answers terminal queries (the real terminal does),
and keeps the shadow's dimensions identical to the real terminal at all times.

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
`514e4465ee33d4d81682d06ee2934483982a54ed` (2026-07-29): `src/terminal/` →
`crates/gaze-term`, `src/detect/` (with `manifests/`, `manifest_update.rs`) →
`crates/gaze-detect`. See ADR-0005 for how the lift is kept re-vendorable and
NOTICE for attribution.

## Consequences

We own a full terminal-emulator maintenance surface, and `gaze-term` is
therefore more than an emulator — it also owns effective-state arbitration and
a good deal of upstream lifecycle machinery gaze never calls. Accepted for
detection fidelity.
