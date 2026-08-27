# A remote agent's lifetime is its ssh session's

A remote agent appears in the local registry exactly as long as the ssh
session carrying its whispers: one session, one row. The wrapper that opened
the session owns the row — it registers as itself (`Unknown`, screen
detection of whatever is on screen), and when a remote wrapper's knock is
answered, that same row *becomes* the remote agent's: same id, remote
facts, local `window` and `workspace` grafted on, timestamps re-stamped
locally so remote clocks never show up in a duration. When the remote agent
says goodbye, or falls silent past the 90-second whisper timeout, the row
reverts to the wrapper's own entry, kept current the whole time.

Rejected: persistence — a remote agent that survives the session, to be
reattached later. It would mean a reconnect protocol, a staleness model,
and amon growing into a session manager, when tmux and herdr already are
ones. Closing the laptop lid on an ssh'd agent ends it exactly as closing a
terminal window ends a local one, which is a rule the registry already has:
an entry exists exactly as long as its wrapper's connection (ADR-0002), and
this decision extends it across the wire rather than inventing a second
lifecycle.

## Consequences

There is no reconnect machinery to maintain and none to get wrong; the
whisper channel needs no sequence numbers, no resume, no snapshot protocol
beyond "a register is a snapshot". The cost is scope: amon sees the agents
inside sessions you opened, not everything running on the remote host. A
daemon-to-daemon bridge over `ssh host amon pipe` remains open as a later,
complementary layer for whole-host visibility, and persistence remains
tmux's or herdr's to provide underneath amon.
