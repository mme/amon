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

## Provenance

Lifted from https://github.com/ogulcancelik/herdr (Apache-2.0) at commit
`514e4465ee33d4d81682d06ee2934483982a54ed` (2026-07-29): `src/terminal/` →
`crates/gaze-term`, `src/detect/` (with `manifests/`, `manifest_update.rs`) →
`crates/gaze-detect`. Modifications are noted in per-file headers; see NOTICE.

## Consequences

We own a full terminal-emulator maintenance surface, and re-vendoring upstream
improvements is a manual diff-and-merge. Accepted for detection fidelity.
