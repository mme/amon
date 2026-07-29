# gaze

Run your coding agents behind a lens:

```sh
gaze claude          # wrap any agent — passthrough is byte-for-byte
gaze codex --resume
gaze status          # what's Idle, Working, or Blocked right now?
gaze install claude  # optional per-agent integration hooks
```

`gaze` wraps an AI coding agent in a PTY, detects its live state (Idle /
Working / Blocked) via a shadow terminal, and reports it to a per-user daemon
(`gaze daemon`, auto-started) that keeps a registry of all connected agents
and pushes live events to subscribers over a unix socket.

See [CONTEXT.md](./CONTEXT.md) for the project language and
[docs/adr/](./docs/adr/) for the architecture decisions.

## Credit

Gaze's terminal emulation, agent state detection (and its manifests), and the
per-agent integration install machinery are derived from
[herdr](https://github.com/ogulcancelik/herdr) by Ogulcan Celik
(Apache-2.0) — an excellent agent multiplexer you should check out. Detection
manifests self-update from herdr's public catalog. See [NOTICE](./NOTICE) for
details.

## License

Apache-2.0. See [LICENSE](./LICENSE) and [NOTICE](./NOTICE).
