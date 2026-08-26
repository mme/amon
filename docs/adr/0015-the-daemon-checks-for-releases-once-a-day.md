# The daemon checks for releases once a day, and only tells the panel

A release build's daemon resolves GitHub's stable `/releases/latest`
redirect at most daily — one HEAD request via curl, the same contract
`install.sh` reads a version from, carrying nothing about the machine. A
strictly newer version (numeric compare, the socket-takeover ordering)
becomes an `update_available` event, stored and replayed to subscribers that
connect later; the agent panel draws it as a one-line footer with the
install command, click-to-copy. Nothing downloads or installs itself —
upgrading stays the user re-running the installer.

## The boundaries, each deliberate

- **Off switch**: `[updates] check = false` in `config.toml`, read live by
  the existing watcher. On by default for the reason the sounds are: a
  notification nobody knows exists notifies nobody, and the starter config
  file `amon setup` writes shows the switch.
- **Debug builds never check.** Every test sandbox and dev daemon would
  otherwise reach the real network. Pointing `AMON_UPDATE_URL` somewhere is
  both the test hook and the dev enabler.
- **Strictly newer only**: a from-source build ahead of the last release
  must never be told to downgrade, and the current release is not an update.
- **No release yet reads as silence**: the redirect lands somewhere that is
  not a version tag, and a check that cannot answer says nothing rather than
  guessing.
- **The redirect, never the API**: nothing to rate-limit, nothing to parse,
  and the installer and the check can never disagree about what "latest"
  means — a test pins the two to the same contract.
