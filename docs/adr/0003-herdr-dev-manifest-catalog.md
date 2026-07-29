# Detection manifests self-update from herdr.dev's catalog

Amon mirrors herdr's manifest freshness behavior exactly: all agent manifests
are bundled into the binary (`include_str!`), layered as local override →
remote → builtin, and the lifted updater fetches
`https://herdr.dev/agent-detection/index.toml` — herdr's own catalog — guarded
by each manifest's `min_engine_version` so manifests our vendored engine can't
evaluate are refused. We deliberately depend on herdr's public endpoint instead
of hosting our own catalog (infra we don't want yet) or shipping bundled-only
(a month-old daemon would go stale as agent UIs change).

`amond` refreshes at daemon startup and every 24h thereafter (herdr itself only
checks at app startup; our daemon lingers for weeks, hence the timer). Wrappers
load manifests once at launch — picking up refreshed manifests requires
restarting the wrapper; there is no live-reload event.

## Consequences

Detection freshness is coupled to a third party's endpoint and release cadence.
If herdr.dev disappears or diverges, amon degrades gracefully to its bundled
manifests. Re-vendoring the detect engine keeps the `min_engine_version` gate
honest. Being a good citizen about this dependency (and attribution) matters —
see NOTICE.
