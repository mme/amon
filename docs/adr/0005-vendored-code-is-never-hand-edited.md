# Vendored herdr code is never hand-edited

Amon vendors three substantial herdr modules (ADR-0001, ADR-0004) and wants to
keep pulling upstream improvements — especially in the terminal emulator and
the agent config-file editing, which are the parts most likely to gain fixes we
want. Hand-editing vendored files would make every future re-vendor an
archaeology exercise, so we forbid it outright and express every deviation
mechanically instead:

1. **Import paths are not edited.** Each amon crate recreates the
   crate-internal namespace the vendored code expects (`crate::api::schema`,
   `crate::platform`, `crate::logging`, …) as thin shim modules we own, so
   vendored files compile byte-identical. Unused upstream surface is silenced
   with `allow(dead_code)` at the shim level.
2. **The herdr → amon rename is a deterministic token map** applied while
   copying upstream into `crates/`: `HERDR_`→`AMON_`, `herdr`→`amon` in
   filenames and identifiers, `pane.report_agent`→`agent.report_state`,
   `pane.report_agent_session`→`agent.report_session`, and — in hook assets
   only — the CLI verb `pane`→`hook`, because the assets that cannot speak the
   socket shell out to `amon hook report-agent-session` where herdr's shell
   out to `herdr pane …`. Generated files carry a
   "generated from herdr `<commit>` `<path>`; do not edit" header. The rename is
   mandatory, not cosmetic: distinct env var names and hook filenames are what
   let amon and herdr be installed on the same machine.
3. **Anything else lives in `vendor/patches/*.patch`**, re-applied
   mechanically, so a conflicting upstream change fails loudly at re-vendor
   time instead of being silently dropped.
4. **`just revendor`** clones herdr at a pinned commit, applies the token map
   and patches, builds, tests, and shows the diff. Upgrading is one command
   plus a diff review. A `cargo fmt` pass closes it out, because the token map
   does not preserve line width — `amon` is a character shorter than `herdr`,
   so lines upstream had wrapped can now fit on one. It runs after the patches,
   whose context therefore stays upstream-shaped.

## Consequences

Deviations we might otherwise make casually (renaming a function, deleting
unused code, reformatting) are effectively banned — they would have to become
patches, and patches have an ongoing cost. That friction is the point.
