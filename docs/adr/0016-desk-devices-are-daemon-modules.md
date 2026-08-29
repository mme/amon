# Desk devices are daemon modules, and the first one is the Creator Micro 2

A Work Louder Creator Micro 2 connected to an amon machine becomes a
hardware agent panel: six keys lit by agent state in the Codex palette, an
ambient ring answering "does anything need me", and a tap on a key focusing
the agent it shows. The other controls have fixed roles — the encoder
scrolls and drives the open agent panel, the joystick moves window focus —
except the seven macro keys, each mapped to a configurable action. The
configurability line is deliberate: what the device *means* (keys are
workspaces, the knob navigates, the stick points) is identity and stays
the same on every desk; only the spare macro keys are the user's to spend.

## Where device support lives

In the daemon, as compiled-in modules on their own threads — the same shape
as the sounds and the update check. The daemon always runs, so devices need
no lifecycle of their own: discovery polls the hidraw layer every second or
two, which makes hotplug, Bluetooth reconnects, and cable switches all the
same event. Community devices arrive as pull requests implementing the same
seams (`discover`, a device loop fed rosters and config, an outcome
channel); anyone wanting an out-of-tree adapter in another language has the
subscriber socket, which remains amon's public extension surface.

The rejected shapes: a sidecar process invents a lifecycle (who starts it,
who restarts it) for no isolation the thread boundary does not already
give; external-adapters-only makes the out-of-box experience a download.

## The protocol, and where it came from

The Micro 2 is not QMK: it speaks Work Louder's vendor protocol — JSON-RPC
in 64-byte HID reports on usage page `0xFF00`, identical over USB and BLE.
The per-key methods (`v.oai.thstatus`, `v.oai.rgbcfg`) and the input
notifications (`v.oai.hid`, `v.oai.rad`) ship only in the Codex desktop
app's private SDK. The protocol was learned from published artifacts for
interoperability — reading, not vendoring; amon contains none of their
code — and every fact was verified against real hardware before any of it
was built: all thirteen keys, the encoder's detents and click, the
joystick's measured angles, and the absence of any handshake.

That last fact shapes the module: there is no mode to enter. Opening the
node and writing a lighting state is the whole attachment, and the first
write doubles as the capability probe — firmware that predates the protocol
answers nothing and is left alone, reported by doctor rather than retried.

## The one root step

`/dev/hidraw*` and `/dev/uinput` are root-only on stock systems, and amon
never runs as root. The single exception to zero-setup is one udev rule
(seat access via `uaccess` for Work Louder's vendor id and for uinput),
written with the user's own sudo: interactive `amon setup` shows the rule
in full and offers to run it, scripts get the command printed, and a device
that appears later without access earns one desktop notification. Machines
that already have equivalent rules — the Work Louder Input app installs
some — are detected by the only test that matters, opening the node, and
never see any of this.

## Costs accepted

Lighting writes are best-effort fire-and-forget; a device that stops
answering is detached and rediscovered rather than diagnosed. The uinput
synthesis speaks the legacy interface with a fixed key table — exotic
chords are not spellable. And the Codex protocol is Work Louder's to
change: the pins hold amon's two ends together, but a firmware that renames
`v.oai.*` turns the module off until someone reads the new sources. The
capability probe makes that failure silent and safe, which is the designed
response to depending on someone else's private protocol.
