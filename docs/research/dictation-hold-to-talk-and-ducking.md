# Dictation and the Micro 2 — hold-to-talk, auto-submit, and ducking while dictating

Research notes, 2026-08-27. Sources are primary throughout: this repo's
source (branch `micro2`, uncommitted), the voxtype source at the exact
installed version (`voxtype-bin 0.7.5`, tag `v0.7.5` of
<https://github.com/peteonrails/voxtype>, MIT), the Omarchy install on this
machine (`omarchy 4.0.0` pacman package, files under
`~/.local/share/omarchy`), and kernel/QMK documentation. Confirmed facts are
stated plainly with a source; anything marked **inference** or **proposal**
is not. Both "recommended approach" sections are proposals, not decisions.

## TL;DR — feasibility verdicts

1. **Hold-to-talk on the dictation key: feasible, low risk.** The Micro 2
   reports press *and* release for every key (`v.oai.hid` with `act` 1/0,
   verified on hardware — [worklouder-micro-2-linux.md](./worklouder-micro-2-linux.md)
   §12); amon's decoder merely *discards* releases today
   (`crates/amon-daemon/src/devices/micro2.rs:328`). Omarchy's dictation is
   **voxtype**, which has first-class, idempotent `record start` / `record
   stop` entry points (SIGUSR1/SIGUSR2, each a no-op in the wrong state) —
   Omarchy itself already binds F9 as press-to-start/release-to-stop.
   Because tap-toggle and push-to-talk *both* begin with "start recording on
   press", the classic tap-vs-hold latency tradeoff dissolves: nothing needs
   to be delayed.
2. **Auto-Enter after dictation: solved inside voxtype, race-free.**
   voxtype has a native per-recording `--auto-submit` flag; the Enter is
   sent by the same output driver that typed the text, strictly after the
   text (and never on the clipboard fallback). amon does not need to inject
   Enter at all, and should not — though its uinput device could, and the
   state file gives a safe "typing finished" edge if it ever must.
3. **Duck music while dictating: mechanically easy — but largely already
   handled.** Omarchy's shipped voxtype config sets `pause_media = true`:
   every playing MPRIS player (Spotify, browsers, …) is *paused* when
   recording starts and resumed after the text is typed. A duck-instead
   would be the delta, and it can reuse amon's existing WirePlumber
   role-bus drop-in unchanged (hold a silent `media.role=Notification`
   stream open while dictation is active). Dictation activity — however it
   was started, OS hotkey included — is observable through voxtype's state
   file `$XDG_RUNTIME_DIR/voxtype/state` (`idle` / `recording` /
   `transcribing` / `streaming`) or `voxtype status --follow`, exactly the
   seam Omarchy's own bar indicator uses.

---

# Topic 1 — hold-to-talk on the Micro button

## 1. How amon receives Micro button events today

**Not evdev.** The dictation key never traverses the kernel input layer at
all. The amon layer maps every Micro 2 control to Work Louder's vendor
keycodes (`KV_OAI_*`), which emit **no HID keystroke**; instead the device
volunteers JSON-RPC notifications on the vendor HID channel
([worklouder-micro-2-linux.md](./worklouder-micro-2-linux.md) §12–13,
verified on hardware):

```json
{"m":"v.oai.hid","p":{"k":"ACT10","act":1}}   // press
{"m":"v.oai.hid","p":{"k":"ACT10","act":0}}   // release
```

`act` is **1 for press, 0 for release** (encoder detents arrive as `act:2`
and are matched before the act check). The pipeline in amon:

- Discovery polls `/sys/class/hidraw` for VID `303a` / PID `8297|8298` with
  a `0xFF00` vendor usage page —
  `crates/amon-daemon/src/devices/mod.rs:46-90`.
- The device loop `micro2::run` reads 64-byte reports, reassembles
  newline-delimited JSON, and decodes notifications —
  `crates/amon-daemon/src/devices/micro2.rs:537-694` (poll timeout 200 ms,
  lines 624-629).
- `decode_notification` (`micro2.rs:315-349`) is where releases die today:
  the arm `_ if act != 1 => None` (line 328) swallows every key release,
  and the test at `micro2.rs:822-824` pins it ("releases are silent").
  **The release information is on the wire and already reaches this
  function; hold detection needs only a decoder change plus a timestamp,
  no new event source.** This is the load-bearing answer.
- The dictation key is `ACT10` → config name `macro_5` (`micro2.rs:376-387`),
  default action `exec:voxtype record toggle` (`micro2.rs:403`), executed
  via `sh -c` fire-and-forget (`devices/actions.rs:330-337`).

Evdev/`EV_KEY` semantics (value 1=press, 0=release, 2=autorepeat —
<https://www.kernel.org/doc/Documentation/input/event-codes.txt>) apply in
this module only on the *output* side: amon's own uinput virtual device
emits `EV_KEY` 1/0 pairs when synthesizing chords
(`devices/actions.rs:184-209`), under the same udev rule that grants hidraw
access (`crates/amon-integration/src/udev.rs:20-25`).

## 2. What `voxtype record …` actually does (Omarchy's dictation)

Omarchy's dictation is **voxtype** (<https://voxtype.io>,
<https://github.com/peteonrails/voxtype>; installed as `voxtype-bin 0.7.5`
from Omarchy's repos). Omarchy wires it up in
`~/.local/share/omarchy/default/hypr/bindings/voxtype.lua`:

```lua
o.bind("SUPER + CTRL + X", "Toggle dictation", "voxtype record toggle")
o.bind("F9", "Start dictation (push-to-talk)", "voxtype record start")
o.bind("F9", "Stop dictation (push-to-talk)", "voxtype record stop", { release = true })
```

So the OS itself already ships a push-to-talk built from the same CLI amon
would use — press/release start/stop is a supported, intended usage.

**Entry points and idempotency** (voxtype v0.7.5 source):

- `voxtype record start` sends **SIGUSR1**, `stop` sends **SIGUSR2** to the
  daemon (`src/main.rs:1095-1097`); `toggle` reads the state file and picks
  the signal (`src/main.rs:1099-1133` — `recording`/`streaming` → stop,
  anything else → start).
- **SIGUSR1 acts only when idle**: the handler is guarded by
  `if state.is_idle()` (`src/daemon.rs:3300-3302`) — a second start during
  recording or transcribing is a no-op. **SIGUSR2 acts only while
  capturing** (`src/daemon.rs:3422-3446`); a stray stop while idle is a
  no-op. So amon can fire start/stop without holding a lock on the truth.
- State machine: `Idle → Recording → Transcribing → Outputting → Idle`
  (`src/state.rs:1-80`; a `Streaming` state replaces Recording+Transcribing
  for streaming engines — not Omarchy's default).

**How text is inserted.** Output mode `"type"` (Omarchy's default config)
walks a driver chain **wtype → eitype → dotool → ydotool → clipboard →
xclip** (`src/output/mod.rs:250-258`), taking the first available tool;
wtype is installed here (optdep of `voxtype-bin`, "recommended"). The whole
chain runs inside `output_with_fallback` (`src/output/mod.rs:470-559`),
which also (a) optionally waits for physical modifier keys to be released
before synthesizing keystrokes, falling back to clipboard on timeout, and
(b) runs configurable `pre_output_command` / `post_output_command` hooks
around the typing (`src/config.rs:2034-2044`).

**When is typing finished?** The daemon's output cycle
(`src/daemon.rs:2228-2266`) is strictly sequential: enter `Outputting`,
run the output chain to completion, resume paused media players, *then*
set state `Idle` and write `"idle"` to the state file. There is no
`"outputting"` state-file value — the file reads `"transcribing"` until
the text (and any auto-submit Enter, see below) has been delivered, and
flips to `"idle"` only after. **The `transcribing → idle` edge of the state
file is therefore an after-the-text signal**, usable by any watcher.

## 3. Auto-Enter: voxtype already owns this, and that kills the race

voxtype has native auto-submit, per-recording:

- `voxtype record start --auto-submit` (and `toggle --auto-submit`) —
  "Auto-submit (press Enter) after this transcription"
  (`src/cli.rs:672-725`). There is also a global config key
  `[output] auto_submit` (`src/config.rs:2013`, default false) and even a
  `--smart-auto-submit` mode (say "submit" to press Enter).
- The flag travels as a one-shot override file
  `$XDG_RUNTIME_DIR/voxtype/auto_submit_override` written by the CLI before
  signaling (`src/main.rs:1080-1085`), read **and deleted** by the daemon at
  output time (`src/daemon.rs:2136-2161`, `read_bool_override` at
  `src/daemon.rs:318-350`), and cleaned up on every cancel/error path.
- The Enter is sent by the *same output driver that typed the text*,
  strictly after the text and any configured append-text — e.g. wtype:
  type text, append, then `wtype -k Return`
  (`src/output/wtype.rs:168-193`, `send_enter` at 130-138). The
  clipboard/xclip fallbacks take no `auto_submit` at all
  (`src/output/mod.rs:299-310` area, `src/output/clipboard.rs`) — **if
  typing fell back to the clipboard, no Enter fires into the wrong
  window.**

So the race the task flags — injecting Enter from outside while text
insertion is still in flight — simply does not exist on the native path:
one process, one ordering. amon should *not* inject Enter itself for this
feature.

**The one wrinkle: the flag is declared at start, but amon only knows
"this was a hold" at release.** `record stop` deliberately carries no
auto-submit flag (`RecordAction::Stop { .. } => return None`,
`src/cli.rs:984`). Options, best first:

1. **Decide at release with `voxtype record toggle --auto-submit`.** At
   release-after-threshold, amon runs `voxtype record toggle
   --auto-submit`: the CLI writes the override file, reads state
   (`recording`) and sends SIGUSR2 — one documented command meaning "stop,
   and submit this one". The override is read at *output* time, after the
   stop, so ordering is safe; it is one-shot (deleted on read), so a later
   tap-dictation cannot inherit it. This works today with no voxtype
   change, but the "overrides are read at output time, whichever command
   wrote them" contract is observed from source, not documented — worth a
   two-minute hardware test.
2. **Propose `voxtype record stop --auto-submit` upstream.** The plumbing
   (override file + output-time read) already exists; the patch is adding
   two fields to `RecordAction::Stop`. Makes option 1's semantics official.
3. amon writes `$XDG_RUNTIME_DIR/voxtype/auto_submit_override` itself
   before `record stop` — same effect as 1 via an internal path; no reason
   to prefer it.
4. amon's own uinput Enter (`VirtualInput::tap(&[28])`,
   `devices/actions.rs:200-209`) after the state file flips
   `transcribing → idle`. Race-free in practice (the flip is written after
   the output chain returns), but blind to the clipboard fallback — it
   would press Enter when no text was typed. Keep as fallback knowledge,
   not the design.

Also noteworthy: `wtype`, `ydotool`, `hyprctl dispatch sendshortcut` were
to be evaluated as injection mechanisms — voxtype itself uses the
wtype→…→ydotool chain (its Omarchy config comment even names ydotool), and
amon already owns a uinput device, which injects below the compositor and
is the least fragile of all of these on Wayland. But with `--auto-submit`,
none of them is amon's problem.

## 4. Press-duration discrimination — design sketch (proposal)

The classic tap-hold problem (QMK: hold ≥ `TAPPING_TERM`, default **200 ms**,
<https://docs.qmk.fm/tap_hold>) forces a choice: delay the tap action until
release/threshold, or fire it immediately and mispredict holds. **Here the
choice disappears**, because both interpretations share the same prefix —
*start recording now*:

- **Press while idle** → `voxtype record start` immediately. Zero added
  latency in either mode; recording begins the instant the finger lands.
- **Release before ~200 ms** (tap) → do nothing. Recording continues;
  this was a toggle-on. (Next tap stops it.)
- **Release at ≥ ~200 ms** (hold) → `voxtype record toggle --auto-submit`
  (or plain `record stop` if auto-enter is configured off). This was
  push-to-talk.
- **Press while recording** → `voxtype record stop` (or `toggle`)
  immediately: the toggle-off. Its release is ignored regardless of
  duration (re-arming PTT from a toggle session adds states for no user
  win).
- Press while `transcribing`: voxtype ignores SIGUSR1 (not idle), so the
  key is naturally inert; amon can simply always send and let voxtype's
  guards decide, or read the state file first for cleaner logs.

Mechanically, inside the existing architecture:

- `decode_notification` passes `act` through instead of dropping releases —
  e.g. `DeviceInput::Control(String)` grows a pressed/released flag (agent
  keys can keep press-only semantics). Tests at `micro2.rs:816-847` change
  accordingly.
- `micro2::run` keeps a `press_at: Option<Instant>` per hold-capable
  control; all decisions are made on the press and release edges, so the
  existing 200 ms poll tick (`micro2.rs:629`) needs no extra wakeups — no
  timer has to *fire* at the threshold, because the tap action is not
  deferred.
- The dictation action stops being a plain `exec:` string and becomes a
  first-class `Action::Dictate` so the engine knows which control is
  hold-capable (see §5). `exec:voxtype record toggle` keeps working as a
  dumb tap action for anyone who configured it by hand.

Two hardware realities worth designing around: voxtype's
`max_duration_secs = 60` safety limit will end a very long hold by itself
(the release's `stop` then lands on idle and is ignored — harmless), and
the dictation key `ACT10` is **not** one of the six individually lightable
keys (only `AG00–AG05` are `thstatus` slots —
[worklouder-micro-2-linux.md](./worklouder-micro-2-linux.md) §13), so a
"recording" indicator on the key itself is not possible; the ambient ring
is amon's if it ever wants one (zone lighting is not layer-gated, §12).

## 5. Where the configuration would live

Per-button behavior lives in `[devices.micro2.keys]`: a
`BTreeMap<String, String>` of macro-key name → action string
(`crates/amon-protocol/src/config.rs:127-158`, mirrored in
`docs/protocol.schema.json` under `Micro2Config`, documented in the starter
config `crates/amon-cli/src/starter.rs:93-102`). Actions are single parsed
strings (`devices/actions.rs:30-55`): `none`, `panel`, `workspace:N`,
`key:<chord>`, `exec:<cmd>`.

**Proposal:** add one action word and one small table:

```toml
[devices.micro2.keys]
macro_5 = "dictate"              # replaces exec:voxtype record toggle as the default

[devices.micro2.dictation]
hold_ms = 200                    # tap below, push-to-talk at or above
auto_submit = true               # Enter after a *hold* dictation's text lands
```

An action word (rather than `dictate:hold:submit` string syntax) keeps
`Action::parse` trivial and gives the two knobs a documented home; the
table stays optional with tuned defaults, consistent with every other
`Micro2Config` field. `schemars` regenerates the schema. If `dictate` is
assigned to several keys they all behave identically — the state is
voxtype's, not the key's.

---

# Topic 2 — duck music while dictating

## 6. How amon's existing ducking works

Ducking is **declarative and install-time**, not a runtime code path
(ADR-0014, `docs/adr/0014-ducking-is-one-declared-drop-in.md`):

- `amon setup --duck` writes one WirePlumber drop-in,
  `~/.config/wireplumber/wireplumber.conf.d/50-amon-ducking.conf`
  (`crates/amon-integration/src/ducking.rs`; the file verbatim at
  `crates/amon-integration/src/assets/wireplumber/50-amon-ducking.conf`).
- The drop-in declares two role loopback buses — Multimedia (priority 10,
  where every untagged stream lands via
  `node.stream.default-media-role = "Multimedia"`) and Notifications
  (priority 20, `action.lower-priority = "duck"`,
  `linking.role-based.duck-level = 0.25`).
- The **trigger is any live playback stream tagged
  `media.role=Notification`**: the daemon's sound player passes exactly
  that (`crates/amon-daemon/src/sound.rs:26-36`, `pw-play` with
  `{ media.role = "Notification" }`). WirePlumber ducks Multimedia while
  the stream is live and restores when it ends. **No amon process is in
  the loop at runtime**, and a crash restores volume by construction —
  ADR-0014 explicitly rejected imperative ducking because PipeWire
  *remembers* per-app volumes a crashed ducker would leave lowered.

**Reusable for a second trigger source? Yes, as-is.** The policy does not
know or care that the Notification-bus stream is a notification sound; any
stream amon holds open on that bus ducks music for exactly its lifetime,
with the same crash-safety. (A dedicated third "Dictation" bus with its own
duck level would also work — but means editing the installed drop-in and
re-answering ADR-0014's merge-conflict questions; not needed for a first
cut.)

## 7. How amon can know dictation is active — including the OS hotkey case

The easy case (amon started it) is trivial. The general case is also
solved, because **every** start path — Super+Ctrl+X toggle, F9
push-to-talk, `voxtype record …` from anywhere, the future Micro key —
funnels into the one voxtype daemon and its state machine, and the daemon
publishes state for exactly this purpose:

- **State file**: `$XDG_RUNTIME_DIR/voxtype/state` (here:
  `/run/user/1000/voxtype/state`), enabled by `state_file = "auto"` —
  which Omarchy's shipped config sets
  (`~/.local/share/omarchy/default/voxtype/config.toml`; also present in
  the live `~/.config/voxtype/config.toml`). Written on every transition
  (`write_state_file`, voxtype `src/daemon.rs:90-105`, callers throughout).
  Values: `idle`, `recording`, `transcribing`, and — code, not the config
  comment — `streaming` (`src/daemon.rs:912`). Removed on daemon shutdown
  (`src/daemon.rs:3841-3848`), so "file absent" must read as idle.
  (Meeting mode uses a separate `meeting_state` file and never touches
  this one.)
- **Push interface**: `voxtype status --follow --format json` streams a
  JSON line per change. This is precisely what Omarchy's bar runs: the
  Dictation indicator (`~/.local/share/omarchy/shell/plugins/bar/indicators/Dictation.qml`)
  spawns `omarchy-voxtype-status`
  (`~/.local/share/omarchy/bin/omarchy-voxtype-status`), which `exec`s
  `setpriv --pdeathsig TERM voxtype status --follow --extended --format
  json` — including the orphan-prevention trick amon would want too.

**Inference/recommendation:** for ducking, `active :=` state ∈
{`recording`, `streaming`} (duck while the mic is hot); `transcribing` no
longer captures audio, and keeping music down through a multi-second
transcription serves nobody. amon's house style is polling (the config
watcher polls mtime at ~1 s, `crates/amon-daemon/src/config.rs:7`), but a
1 s duck lag on recording start is audible; the `status --follow`
subprocess (or inotify on the state file) is the responsive choice, and
the bar already demonstrates the subprocess pattern surviving voxtype
restarts.

## 8. What voxtype/Omarchy already do about playing audio

**Omarchy already pauses music during dictation.** voxtype's
`[audio] pause_media` (default `false` upstream, voxtype
`src/config.rs:65,2354`) is set **`true` in Omarchy's shipped config** —
and in the live config on this machine. Implementation
(`src/audio/media.rs`, wired at `src/daemon.rs:787-799`): on recording
start, `playerctl --all-players` finds every MPRIS player in `Playing`
state and pauses it; the daemon remembers exactly those names and resumes
them after the text has been typed (`resume_media_players()` runs right
before the state flips to idle, `src/daemon.rs:2264-2266`). voxtype
contains no ducking of any kind (zero hits for "duck" in its source) and
no echo cancellation — pausing *is* its answer to speaker bleed into the
mic.

**Correction (found debugging this machine): pausing is broken on a
stock Omarchy install.** `pause_media` shells out to `playerctl`, but
nothing installs it: `voxtype-bin` 0.7.5 does not list it even as an
optional dependency (`pacman -Qi voxtype-bin`: depends only on alsa-lib,
curl, gcc-libs, glibc; optdepends are the typing tools), and Omarchy's
`omarchy-voxtype-install` doesn't pull it in either. voxtype degrades to
a per-recording `WARN playerctl not found or failed to run` in the
journal and records anyway — which is exactly the "music keeps playing
while I dictate" symptom that motivated this topic. Fix on this machine:
`pacman -S playerctl`. Upstream: `playerctl` belongs in voxtype-bin's
optdepends (and arguably in Omarchy's install hook); `amon doctor` could
also warn when voxtype has `pause_media = true` but `playerctl` is
missing.

Consequences for this feature:

- With `playerctl` installed, the requested behavior ("music quiet while
  dictating, restored after") **already happens** for anything speaking
  MPRIS — Spotify, browsers, most players. An amon duck adds value only
  for (a) audio without MPRIS (games, raw PipeWire streams, remote desktop
  audio) and (b) users who prefer duck-to-25% over a hard pause.
- Duck and pause interact, not conflict: with both on, MPRIS players
  pause and the duck lowers whatever else remains. But note the accuracy
  angle: ducked audio still reaches the microphone and Whisper will
  happily transcribe it — a user who *disables* `pause_media` to get
  continuous music trades transcription quality for it. The doc/setting
  should say so.

## 9. Where the setting would live, and the recommended shape (proposal)

**Mechanism:** a small watcher in the daemon (same module shape as sounds/
updates/devices — ADR-0016's pattern) that follows voxtype state and,
while `recording`/`streaming`, holds open one **silent playback stream
tagged `media.role=Notification`** — e.g. `pw-play` (already amon's one
player, `sound.rs:30`) on a bundled silence asset, or `pw-cat --playback`
fed silence for an unbounded stream. The existing drop-in then ducks
Multimedia to 0.25 for exactly the stream's lifetime. Properties worth the
choice: zero changes to the installed audio config, and amon crashing
mid-dictation kills the stream and restores volume — the exact property
ADR-0014 built the bus for. The alternative — `wpctl set-volume` on the
Multimedia loopback sink (a node amon arguably owns) — is imperative
ducking with the crash-leaves-it-quiet failure mode ADR-0014 rejected;
only worth revisiting if the silent-stream trick proves flaky.

**Config:** ducking's only current surface is install-time
(`amon setup --duck`); runtime sound behavior lives in `[sound]`
(`crates/amon-protocol/src/config.rs:65-96`). Proposal:

```toml
[sound]
duck_while_dictating = false   # duck music (needs `amon setup --duck`) while voxtype records
```

Default **off**: Omarchy already pauses MPRIS media, so the setting is an
opt-in for the non-MPRIS/duck-preference niche, and it is inert (worth a
`doctor` note) unless the ducking drop-in is installed and voxtype's state
file exists.

---

# Open questions and risks

1. **`record toggle --auto-submit` as "stop and submit"** rests on
   overrides being read at output time regardless of which command wrote
   them — true in v0.7.5 source (`daemon.rs:2136-2161`), but an internal
   ordering, not documented CLI semantics. One hardware test; and a tiny
   upstream PR (`record stop --auto-submit`) would make it contractual.
2. **Streaming engines** (Parakeet et al.): text is typed incrementally
   during `Streaming`, auto-submit does not appear in that path at all,
   and SIGUSR2 *disowns* the session rather than flushing it
   (`daemon.rs:3425-3436`). Omarchy defaults to Whisper batch mode, so
   this is a documentation caveat, not a blocker — but hold-to-talk +
   auto-enter should be described as "batch engines" behavior.
3. **voxtype is a moving target** (0.7.x, active): the state-file values
   and CLI flags used here are public, documented surface; the override
   file location is not. Pin what amon depends on in tests the way the
   Micro 2 wire shapes are pinned.
4. **Threshold feel**: 200 ms (QMK's default tapping term) is the starting
   point; dictation is not typing, and a slightly longer term (250–300 ms)
   may misclassify fewer deliberate taps. Ship it configurable
   (`hold_ms`), tune on hardware.
5. **Multi-writer race on dictation state**: the user can mix Micro-key,
   Super+Ctrl+X, and F9 freely. voxtype's signal guards make every
   interleaving safe, but amon's tap-vs-hold bookkeeping must treat the
   state file, not its own last action, as truth (e.g. a press while the
   *hotkey* already has it recording is a toggle-off).
6. **Silent-stream duck**: needs one live test that WirePlumber's
   role-based policy treats a silence-playing stream as "live" for the
   duck's duration (it should — the policy keys on stream lifetime, not
   loudness), and that start/stop of that stream doesn't audibly pump the
   music. Also re-confirm the drop-in's known caveat: role buses are
   stereo; surround/passthrough setups skip ducking entirely (ADR-0014).
7. **No per-key recording light**: `ACT10` is not individually lightable;
   if a visual recording indicator matters, it is the ring's job (zone
   lighting works on any layer) and competes with the fleet-status ring
   (`micro2.rs:161-203`). Undecided.
