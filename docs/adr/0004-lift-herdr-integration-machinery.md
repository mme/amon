# Lift herdr's integration install/uninstall/status machinery verbatim

`amon install <agent>` / `amon uninstall <agent>` / `amon integrations` are
herdr's `integration install|uninstall|status` lifted wholesale rather than
reimplemented: per-target installers that enforce minimum agent CLI versions,
write version-stamped hook scripts into the agent's config dir, and surgically
edit the agent's own config (JSON/TOML merges that preserve user content,
idempotent reinstalls, custom files beside the managed one survive). Uninstall
reverts exactly what install did; status reports
not-installed/current/outdated per target, and the wrapper prints a one-line
outdated notice to stderr before spawning an agent. The config-editing code is
the hard, battle-tested part — reimplementing it would just re-earn herdr's
bug fixes.

All 15 targets are ported (env vars renamed `HERDR_*` → `AMON_*`, socket
methods renamed to the amon protocol); claude, codex, and pi are smoke-tested
and declared supported, the rest are best-effort.

## Provenance

Lifted from https://github.com/ogulcancelik/herdr (Apache-2.0) at commit
`514e4465ee33d4d81682d06ee2934483982a54ed` (2026-07-29): `src/integration/`
(actions, targets, registry, config_edit, env, version, assets) →
`crates/amon-integration`. Modifications (renames, protocol methods) are noted
in per-file headers; see NOTICE.
