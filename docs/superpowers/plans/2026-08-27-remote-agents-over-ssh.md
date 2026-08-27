# Remote Agents over SSH Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An agent wrapped by amon on a remote host, reached over any SSH, appears and behaves as a first-class entry in the local registry — bar, panel, sounds, `amon status` — with hook authority intact; and amon builds headless for Apple Silicon Macs so a Mac can be the remote end.

**Architecture:** The remote wrapper whispers its daemon-protocol traffic (`agent.register`/`agent.update` frames) into its own terminal output as private DCS escape sequences, gated by a knock-once handshake. The local wrapper — every wrapper, no new subcommand — strips those sequences out of the passthrough, answers the knock, and mirrors the remote entry onto its own registry row. macOS support is subtraction: the desktop integration is compile-gated out, everything else already uses portable primitives.

**Tech Stack:** Rust, portable-pty, libghostty-vt (Zig), newline-delimited JSON (serde_json), GitHub Actions + mise for release builds.

**Spec:** `docs/superpowers/specs/2026-08-27-remote-agents-over-ssh-design.md` — read it first; every rule below argues from it.

## Global Constraints

- **Passthrough stays byte-identical** except for well-formed `ESC P +amon;…` DCS sequences (stripped) — every other byte, including foreign DCS, reaches the terminal unchanged.
- **The remote wrapper emits nothing until armed**, except one knock probe, and only when `SSH_TTY` is set and stdout is a terminal.
- **Whisper writes only at sequence boundaries** — through `Screen`, guarded by `ModeScanner::at_boundary()`, like `reassert_focus_reports`.
- **No daemon wire-protocol change.** `docs/protocol.schema.json` must be untouched by Phase 1 (the schema test enforces this).
- **The whisper envelope is versioned** (`+amon;1;`) and additive-forever, same discipline as `PROTOCOL_VERSION`.
- **macOS target is `aarch64-apple-darwin` only.** No Intel build anywhere: not in CI, not in the installer.
- **Tests run via nextest**: `just test` for the suite, `cargo nextest run -p <crate> <filter>` per task. Never plain `cargo test`.
- **Repo commit style:** plain descriptive sentences (`git log --oneline` shows the register), no `feat:`/`fix:` prefixes. Never credit an AI tool in any commit or text.
- Malformed, oversized (> 64 KiB payload), or unknown whisper frames are **discarded silently** — never a panic, never bytes leaking to the terminal, never a desynchronized stream.

## File Structure

| File | Responsibility |
| --- | --- |
| `crates/amon-wrapper/src/whisper.rs` (new) | Everything whisper: envelope constants, `WhisperFrame`, encode, `OutputScanner` (strip + parse output), `scan_input` (answer detection), `Outbox` (queued emission + armed flag) |
| `crates/amon-wrapper/src/lib.rs` | Wiring: `Screen` gains the scanner and the outbox flush, the read loop routes frames, the stdin thread scans for the answer, startup emits the probe |
| `crates/amon-wrapper/src/observer.rs` | Mirroring: `Signal::Remote`, merge rules, suppression while a remote agent owns the row, revert on bye/silence |
| `crates/amon-wrapper/src/daemon_link.rs` | The tee: armed link traffic is also encoded into the outbox |
| `crates/amon-cli/tests/e2e.rs` | One nested-wrapper end-to-end test, no real ssh |
| `crates/amon-integration/src/lib.rs`, `alias.rs` | macOS gating and zsh support |
| `.github/workflows/ci.yml`, `publish.yml`, `install.sh` | macOS build, release artifact, installer target selection |
| `docs/adr/0017…0019`, `README.md` | The three ADRs the spec names, and user-facing docs |

Phase 1 is Tasks 1–7 (whisper), Phase 2 is Tasks 8–9 (macOS), Task 10 is docs. The phases are independent; do not interleave them.

---

### Task 1: Whisper envelope — frames, encode, payload round-trip

**Files:**
- Create: `crates/amon-wrapper/src/whisper.rs`
- Modify: `crates/amon-wrapper/src/lib.rs` (add `mod whisper;` next to the other mods, line ~16)

**Interfaces:**
- Consumes: `amon_protocol::Method` (already a dependency; serializes as `{"method":…,"params":…}` via its serde tag).
- Produces (later tasks rely on these exact names):
  - `pub const PREFIX: &[u8] = b"\x1bP+amon;1;";` and `pub const ST: &[u8] = b"\x1b\\";`
  - `pub enum WhisperFrame { Knock, Answer, Bye, Event(Method) }`
  - `pub fn encode(frame: &WhisperFrame) -> Vec<u8>`
  - `pub(crate) fn parse_payload(payload: &[u8]) -> Option<WhisperFrame>` — `b"knock"`/`b"here"`/`b"bye"` for the fixed frames, otherwise JSON-decoded `Method`; anything else `None`.

Payload safety fact to state in the module doc: serde_json escapes every control character in strings as `\u00XX`, so a serialized `Method` can never contain a raw ESC — the `ESC \` terminator is unambiguous by construction.

- [ ] **Step 1: Write the failing tests** (in `#[cfg(test)] mod tests` inside `whisper.rs`, matching the style of `focus.rs` tests)

```rust
#[test]
fn frames_encode_inside_the_envelope() {
    assert_eq!(encode(&WhisperFrame::Knock), b"\x1bP+amon;1;knock\x1b\\");
    assert_eq!(encode(&WhisperFrame::Answer), b"\x1bP+amon;1;here\x1b\\");
    assert_eq!(encode(&WhisperFrame::Bye), b"\x1bP+amon;1;bye\x1b\\");
}

#[test]
fn an_event_round_trips_through_its_payload() {
    let method = Method::AgentUpdate(AgentPatch::new("a1"));
    let encoded = encode(&WhisperFrame::Event(method.clone()));
    let payload = &encoded[PREFIX.len()..encoded.len() - ST.len()];
    match parse_payload(payload) {
        Some(WhisperFrame::Event(parsed)) => assert_eq!(parsed, method),
        other => panic!("expected the same event back, got {other:?}"),
    }
}

#[test]
fn a_serialized_event_never_contains_a_raw_escape() {
    let mut entry = test_entry(); // build an AgentEntry with title Some("\u{1b}]0;sneaky".into())
    entry.title = Some("\u{1b}]0;sneaky".into());
    let encoded = encode(&WhisperFrame::Event(Method::AgentRegister(entry)));
    let payload = &encoded[PREFIX.len()..encoded.len() - ST.len()];
    assert!(!payload.contains(&0x1b), "JSON must escape control characters");
}

#[test]
fn garbage_payloads_parse_to_nothing() {
    assert!(parse_payload(b"nonsense").is_none());
    assert!(parse_payload(b"{\"method\":\"no-such\",\"params\":{}}").is_none());
    assert!(parse_payload(b"").is_none());
}
```

For `test_entry()`, copy the shape used in `crates/amon-protocol/tests/protocol.rs:20` (an `AgentEntry` with every required field filled with dummies). `WhisperFrame` needs `#[derive(Debug, Clone, PartialEq)]`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p amon-wrapper whisper`
Expected: compile failure — module and names don't exist yet.

- [ ] **Step 3: Implement** — constants, enum, `encode` (concatenate PREFIX + payload + ST; the `Event` payload is `serde_json::to_vec(&method)`), `parse_payload` (match the three fixed byte strings first, then `serde_json::from_slice::<Method>` and map `Ok` → `Event`, discarding methods other than `AgentRegister`/`AgentUpdate` is NOT done here — parse any valid `Method`; filtering is the observer's business).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run -p amon-wrapper whisper`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/amon-wrapper/src/whisper.rs crates/amon-wrapper/src/lib.rs
git commit -m "A whisper envelope: amon-to-amon frames inside a private DCS"
```

---

### Task 2: OutputScanner — strip whispers from the output stream

**Files:**
- Modify: `crates/amon-wrapper/src/whisper.rs`

**Interfaces:**
- Produces:
  - `pub struct OutputScan { pub forwarded: Option<Vec<u8>>, pub frames: Vec<WhisperFrame> }` — `forwarded: None` means the chunk was untouched and the caller forwards the original buffer (same convention as `focus::Scan::stripped`).
  - `pub struct OutputScanner { … }` (stateful, `Default`), `pub fn scan(&mut self, chunk: &[u8]) -> OutputScan`

**The core rule** (document it on the struct): bytes are withheld from the output **only while they could still be an amon whisper**. On the first byte that diverges from `PREFIX`, everything withheld is flushed into `forwarded` and scanning resumes — a foreign DCS passes through byte-identical. Once the full `PREFIX` has matched, the sequence is amon's: its bytes never reach the terminal, the payload accumulates (capped at 64 KiB — past the cap, keep consuming to `ESC \` but stop storing), and at the terminator `parse_payload` decides whether a frame comes out. State must survive chunk splits at any byte position.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn plain_output_is_left_alone() {
    let mut scanner = OutputScanner::default();
    let scan = scanner.scan(b"hello \x1b[1mworld\x1b[0m");
    assert!(scan.forwarded.is_none());
    assert!(scan.frames.is_empty());
}

#[test]
fn a_whisper_is_stripped_and_parsed() {
    let mut scanner = OutputScanner::default();
    let mut bytes = b"before".to_vec();
    bytes.extend_from_slice(&encode(&WhisperFrame::Knock));
    bytes.extend_from_slice(b"after");
    let scan = scanner.scan(&bytes);
    assert_eq!(scan.forwarded.unwrap(), b"beforeafter");
    assert_eq!(scan.frames, vec![WhisperFrame::Knock]);
}

#[test]
fn a_whisper_split_across_chunks_is_still_one_frame() {
    let mut scanner = OutputScanner::default();
    let encoded = encode(&WhisperFrame::Bye);
    for split in 1..encoded.len() - 1 {
        let mut scanner2 = OutputScanner::default();
        let first = scanner2.scan(&encoded[..split]);
        let second = scanner2.scan(&encoded[split..]);
        let forwarded: Vec<u8> = first.forwarded.unwrap_or_default().into_iter()
            .chain(second.forwarded.unwrap_or_default()).collect();
        assert_eq!(forwarded, b"", "split at {split} leaked bytes");
        let frames: Vec<_> = first.frames.into_iter().chain(second.frames).collect();
        assert_eq!(frames, vec![WhisperFrame::Bye], "split at {split}");
    }
    let _ = scanner;
}

#[test]
fn a_foreign_dcs_passes_through_byte_identical() {
    let mut scanner = OutputScanner::default();
    let scan = scanner.scan(b"\x1bPqkitty-stuff\x1b\\rest");
    // Divergence comes after withholding began, so the scanner rebuilds the
    // output; the bytes must be identical even when reassembled.
    let out = scan.forwarded.unwrap_or_else(|| b"\x1bPqkitty-stuff\x1b\\rest".to_vec());
    assert_eq!(out, b"\x1bPqkitty-stuff\x1b\\rest");
    assert!(scan.frames.is_empty());
}

#[test]
fn a_prefix_that_diverges_across_a_chunk_boundary_is_flushed() {
    let mut scanner = OutputScanner::default();
    let first = scanner.scan(b"\x1bP+am");          // could still be ours: withheld
    assert_eq!(first.forwarded.as_deref(), Some(&b""[..]));
    let second = scanner.scan(b"ateur\x1b\\x");     // diverges: everything comes back
    assert_eq!(second.forwarded.unwrap(), b"\x1bP+amateur\x1b\\x");
}

#[test]
fn an_unparseable_payload_is_dropped_without_reaching_the_terminal() {
    let mut scanner = OutputScanner::default();
    let scan = scanner.scan(b"a\x1bP+amon;1;not json\x1b\\b");
    assert_eq!(scan.forwarded.unwrap(), b"ab");
    assert!(scan.frames.is_empty(), "discarded, not surfaced");
}

#[test]
fn an_oversized_payload_is_consumed_and_discarded() {
    let mut scanner = OutputScanner::default();
    let mut bytes = PREFIX.to_vec();
    bytes.extend(std::iter::repeat(b'x').take(70 * 1024));
    bytes.extend_from_slice(ST);
    bytes.extend_from_slice(b"tail");
    let scan = scanner.scan(&bytes);
    assert_eq!(scan.forwarded.unwrap(), b"tail");
    assert!(scan.frames.is_empty());
}

#[test]
fn two_whispers_in_one_chunk_both_come_out() {
    let mut scanner = OutputScanner::default();
    let mut bytes = encode(&WhisperFrame::Knock);
    bytes.extend_from_slice(&encode(&WhisperFrame::Bye));
    let scan = scanner.scan(&bytes);
    assert_eq!(scan.frames, vec![WhisperFrame::Knock, WhisperFrame::Bye]);
    assert_eq!(scan.forwarded.unwrap(), b"");
}
```

Implementation note for the withhold-then-diverge case: when withheld bytes are flushed, they re-enter the *output*, but they must NOT be re-scanned for a new prefix from their second byte onward in a way that loops forever — flush them verbatim and continue scanning from the byte that caused the divergence. Keep an explicit small state machine: `Passthrough`, `PrefixMatch(usize)`, `Payload`. An `ESC` inside `Payload` moves to a `PayloadEscape` state that either ends the sequence (`\`) or is payload plus re-entry (mirroring `ModeScanner::StringEscape`, `focus.rs:262`).

- [ ] **Step 2: Run to verify failure** — `cargo nextest run -p amon-wrapper whisper` → compile errors.
- [ ] **Step 3: Implement the scanner.**
- [ ] **Step 4: Run to verify pass** — same command, all PASS. Also run the full crate: `cargo nextest run -p amon-wrapper`.
- [ ] **Step 5: Commit** — `git commit -m "Strip amon whispers out of the passthrough, and only amon's"`

---

### Task 3: Input-side answer scan

**Files:**
- Modify: `crates/amon-wrapper/src/whisper.rs`

**Interfaces:**
- Produces: `pub struct InputScan { pub answered: bool, pub stripped: Option<Vec<u8>> }`, `pub fn scan_input(chunk: &[u8]) -> InputScan`.

Same philosophy as `focus::scan` (`focus.rs:32`): stateless per chunk, never holds bytes back (input is keystrokes; latency rules). The answer is one fixed byte string (`encode(&WhisperFrame::Answer)`); a split across reads misses it, which degrades to "never armed" — acceptable because the local side sends the answer in a single small write. Document that trade-off on the function.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn input_without_an_answer_is_left_alone() {
    let scan = scan_input(b"ls -la\r");
    assert!(!scan.answered);
    assert!(scan.stripped.is_none());
}

#[test]
fn the_answer_is_taken_out_of_the_input() {
    let mut bytes = b"a".to_vec();
    bytes.extend_from_slice(&encode(&WhisperFrame::Answer));
    bytes.extend_from_slice(b"b");
    let scan = scan_input(&bytes);
    assert!(scan.answered);
    assert_eq!(scan.stripped.unwrap(), b"ab");
}

#[test]
fn an_answer_split_across_reads_is_forwarded_not_held() {
    let encoded = encode(&WhisperFrame::Answer);
    let first = scan_input(&encoded[..4]);
    assert!(!first.answered);
    assert!(first.stripped.is_none());
}
```

- [ ] **Step 2: Run to verify failure** → compile error.
- [ ] **Step 3: Implement** — a windowed byte-string search, strip on match (pattern copied from `focus::scan`).
- [ ] **Step 4: Run to verify pass** — `cargo nextest run -p amon-wrapper whisper`.
- [ ] **Step 5: Commit** — `git commit -m "Hear the answer to a knock on the input stream"`

---

### Task 4: Local wiring — strip, answer, and signal

**Files:**
- Modify: `crates/amon-wrapper/src/lib.rs` (the `Screen` struct ~line 316, `agent_output` ~line 346, the read loop ~line 273)
- Modify: `crates/amon-wrapper/src/observer.rs` (the `Signal` enum, line 27)

**Interfaces:**
- Consumes: `whisper::OutputScanner`, `whisper::WhisperFrame`, `whisper::encode`.
- Produces:
  - `Signal::Remote(RemoteReport)` and `pub enum RemoteReport { Register(Box<AgentEntry>), Update(Box<AgentPatch>), Bye }` in `observer.rs` (handled as a no-op in this task; Task 5 gives it meaning).
  - `Screen::agent_output` returns `std::io::Result<OutputHandled>` where `pub(crate) struct OutputHandled { pub observed: Option<Vec<u8>>, pub frames: Vec<whisper::WhisperFrame> }` — `observed: None` means "forward the original chunk to the observer".

Wiring rules:
1. `Screen` gains `whispers: whisper::OutputScanner`. Inside `agent_output`, run `whispers.scan(bytes)` **first**; the stripped bytes (or the original when untouched) are what `ModeScanner::feed` sees, what stdout gets, and what the caller forwards to the observer. Whisper bytes must never reach any of the three.
2. In the read loop: for each frame — `WhisperFrame::Knock` → write `encode(&WhisperFrame::Answer)` to `pty_writer` in one `write_all` + flush (clone the `Arc` before the loop, as the stdin thread does at line 179); `Event(Method::AgentRegister(e))` → `signals.send(Signal::Remote(RemoteReport::Register(Box::new(e))))`; `Event(Method::AgentUpdate(p))` → `Update`; `Event(_)` → ignore; `Bye` → `RemoteReport::Bye`; `Answer` in the *output* stream → ignore.
3. Scanning and answering are unconditional — every wrapper does this; there is no `amon ssh` special mode. (`amon ssh host` is just wrapping a program named `ssh`, which already works today.)

- [ ] **Step 1: Write the failing test.** The seams are internal to the wrapper binary, so this task's test is the e2e-lite one: extend the wrapper's unit tests where possible, but the real verification is behavioral. Add to `whisper.rs` tests a `Screen`-shaped test if `Screen` is testable; if `Screen`'s stdout coupling makes that unreasonable, write the test at the observer signal level in Task 5 and treat this task's Step 4 (full suite green + manual check) as its gate. Do NOT contort `Screen` into a testing framework — note in the commit message that coverage lands in Tasks 5 and 7.
- [ ] **Step 2: Implement the wiring** exactly as the three rules above.
- [ ] **Step 3: Run the full suite** — `just test` (from the repo root; nextest, one process per test). Expected: everything green; existing wrapper/e2e tests prove passthrough didn't regress.
- [ ] **Step 4: Manual smoke check** — `cargo build && ./target/debug/amon bash -c 'printf "\x1bP+amon;1;knock\x1b\\hello\n"'` in a terminal: output shows `hello` with no junk (the knock was stripped and answered; the answer goes to bash's stdin and is discarded as junk input by `bash -c` — harmless here, note it).
- [ ] **Step 5: Commit** — `git commit -m "Every wrapper answers a knock and strips the whisper"`

---

### Task 5: Observer mirroring — the row becomes the remote agent

**Files:**
- Modify: `crates/amon-wrapper/src/observer.rs`
- Modify: `crates/amon-wrapper/src/lib.rs` (pass the registered `AgentEntry` into `observer::Setup`)

**Interfaces:**
- Consumes: `RemoteReport` (Task 4), `DaemonLink::{register, update}` (`daemon_link.rs:68-74`), `AgentPatch::apply` (`agent.rs:209`).
- Produces: `observer::Setup` gains `pub entry: AgentEntry` (the wrapper's own registered entry; `lib.rs` clones it at line ~96 where it already builds one). Mirroring behavior as specified below — Task 7's e2e relies on it.

**Mirroring rules** (each becomes at least one test):

1. **Merge on `Register`:** build the mirrored entry from the remote one with: `id` = own entry's id (row continuity — the registry treats a repeat register on the same connection as an update, `daemon_link.rs:36-38`); `window`/`workspace` = the locally stored ones (the observer now *stores* what `Signal::Window` delivers, in new fields, as well as patching); `started_at` = own `started_at`; `state_since` = re-stamped `now_millis()` **only when the state differs from the last mirrored state** (heartbeat re-registers repeat the same state and must not reset durations); everything else from the remote entry. Then `link.register(merged)`, set `remote_active = true`, `last_heard = Instant::now()`.
2. **On `Update`:** rewrite `patch.id` to the own id; clear `patch.window` and `patch.workspace` to `None` (the local compositor owns those); if `patch.state` is present and differs from the last mirrored state, set `patch.state_since = Some(now_millis())`, otherwise clear `patch.state_since`; forward via `link.update`. Update `last_heard`.
3. **Suppression while `remote_active`:** the observer's own state publishes (`publish()`), title patches (`detect()`), branch patches, and focus patches do NOT go to the link. `Signal::Window` still goes through (and updates the stored fields). Detection and the shadow keep running — they are needed for revert.
4. **Own-entry maintenance:** the observer holds `own_entry: AgentEntry` and applies every own-patch to it (via `AgentPatch::apply`) whether or not the patch was sent, so revert has current title/branch/state/focus.
5. **Revert:** on `RemoteReport::Bye`, or when `remote_active` and `last_heard.elapsed() > 90s` (checked in the `run` loop tick, which already wakes at least every 150 ms — constant `WHISPER_TIMEOUT: Duration = Duration::from_secs(90)`, chosen as 3× the daemon link's 30 s max retry so a remote in backoff is not declared dead): `link.register(own_entry.clone())`, `remote_active = false`.

- [ ] **Step 1: Write the failing tests.** The observer is constructed via `Observer::new(setup, link)` and driven by `handle(signal)` — both private. Add `#[cfg(test)]` tests inside `observer.rs` that construct an observer with a test `DaemonLink`. `DaemonLink` is a channel sender; give it a test constructor `#[cfg(test)] pub(crate) fn test() -> (Self, mpsc::Receiver<Message>)` in `daemon_link.rs` that returns the link plus the receiving end, so tests assert exactly what was sent. `Message` needs `pub(crate)` visibility of its payload for assertions. Tests to write (names as behavior statements, repo style):

```rust
#[test]
fn a_remote_register_takes_over_the_row() { /* Register arrives; assert the
    link received AgentRegister with own id, remote agent/cwd/hostname,
    locally stored window */ }

#[test]
fn a_heartbeat_register_does_not_reset_the_state_clock() { /* two Registers,
    same state; second merged entry keeps the first state_since */ }

#[test]
fn remote_updates_are_re_addressed_and_re_stamped() { /* Update with state
    change: id rewritten, state_since fresh, window/workspace cleared */ }

#[test]
fn local_detection_stays_quiet_while_a_remote_agent_owns_the_row() { /* after
    Register, a Signal::Focus and a detect-driven publish send nothing */ }

#[test]
fn window_moves_still_reach_the_daemon_while_remote() { /* Signal::Window
    forwards, and updates the stored graft fields */ }

#[test]
fn bye_reverts_to_the_wrappers_own_entry() { /* Bye after Register: link
    receives AgentRegister with the own entry, own agent name */ }

#[test]
fn silence_reverts_the_row() { /* set last_heard back 91s (make last_heard a
    settable Instant in tests), tick, assert revert */ }
```

`Observer::new` builds a `ShadowTerminal` (Ghostty FFI) — fine in tests (`amon-term` tests already do). If it proves flaky in the test environment, factor the mirroring state into a `struct Mirror` with pure methods (`on_register(&mut self, remote: AgentEntry, …) -> AgentEntry` etc.) and test that instead; the Observer then delegates. Prefer the factored form if any test needs more than the observer's public seams.

- [ ] **Step 2: Run to verify failure** — `cargo nextest run -p amon-wrapper observer`.
- [ ] **Step 3: Implement** rules 1–5.
- [ ] **Step 4: Run to verify pass** — `cargo nextest run -p amon-wrapper`, then `just test`.
- [ ] **Step 5: Commit** — `git commit -m "One session, one row: the entry becomes the remote agent's"`

---

### Task 6: Remote side — knock, tee, and goodbye

**Files:**
- Modify: `crates/amon-wrapper/src/whisper.rs` (the `Outbox`)
- Modify: `crates/amon-wrapper/src/lib.rs` (probe at startup, stdin arm, `Screen::flush_whispers`, bye on exit)
- Modify: `crates/amon-wrapper/src/daemon_link.rs` (the tee)

**Interfaces:**
- Produces:
  - `whisper::Outbox`: `Clone`, internally `Arc<Mutex<Vec<Vec<u8>>>>` + `Arc<AtomicBool>` (armed). Methods: `pub fn enqueue(&self, bytes: Vec<u8>)`, `pub fn drain(&self) -> Vec<Vec<u8>>`, `pub fn arm(&self)`, `pub fn armed(&self) -> bool`.
  - `DaemonLink::spawn(version: String, whisper: Option<Outbox>)` — signature change; the existing call site is `lib.rs:59`.
  - `DaemonLink::whisper_armed(&self)` — sends a new `Message::WhisperArmed` variant.
  - `Screen::flush_whispers(&mut self)` — called from the 50 ms injector thread (`lib.rs:243`) right after `reassert_focus_reports`.

Behavior rules:
1. **Session detection:** `let whispering = std::env::var_os("SSH_TTY").is_some();` computed once in `run`. The probe is enqueued only when `whispering && on_a_terminal`, immediately after the `Screen` is set up (~`lib.rs:155`): `outbox.enqueue(whisper::encode(&WhisperFrame::Knock))`. The injector flushes it within 50 ms — at startup the stream is trivially at a boundary.
2. **`Screen::flush_whispers`:** no-op unless `tracking && scanner.at_boundary()`; otherwise write every drained buffer to stdout and flush. (Same safety argument as `reassert_focus_reports`, `lib.rs:368`.)
3. **Arming:** the stdin thread, when `whispering`, runs `whisper::scan_input` on each chunk *before* the focus scan; on `answered`, strips it and calls `link.whisper_armed()` (clone the link into the thread — `DaemonLink` is `Clone`).
4. **The tee** in `daemon_link.rs::run`: when the outbox is `Some` and armed — on `Message::Register`, enqueue `encode(&Event(AgentRegister(entry)))`; on `Message::Update` *that changed the entry* (the existing `changed` check, line 102), enqueue `encode(&Event(AgentUpdate(patch)))`; on every `Timeout` wake, enqueue a register of the held entry (this is the whispered heartbeat — wakes are at most 30 s apart, inside the local 90 s revert window). On `Message::WhisperArmed`: set armed, and enqueue a register of the held entry immediately (the frames sent before arming are gone; this snapshot replaces them). The tee runs whether or not the *remote daemon* connection is up — whispering must survive a hostless daemon.
5. **Goodbye:** in `run`'s shutdown path (after `Signal::AgentExited`, ~`lib.rs:300`), when armed: `outbox.enqueue(encode(&WhisperFrame::Bye))` then lock the screen and `flush_whispers()` once, *before* `screen.finish()`.

- [ ] **Step 1: Write the failing tests.** `Outbox` and the tee are testable directly:

```rust
// whisper.rs
#[test]
fn the_outbox_hands_back_what_was_queued_in_order() { … }
#[test]
fn arming_is_visible_across_clones() { … }

// daemon_link.rs — drive `run` via a channel and a temp-dir socket path that
// nothing listens on (set AMON_DAEMON_SOCKET is NOT possible per-test safely;
// instead note: the link's connect failures are irrelevant to the tee, which
// is why the tee must not depend on connection state).
#[test]
fn nothing_is_whispered_before_the_answer() { /* send Register, assert outbox
    stays empty */ }
#[test]
fn arming_whispers_the_held_entry() { /* Register, then WhisperArmed: outbox
    holds exactly one encoded AgentRegister with that entry */ }
#[test]
fn changed_updates_are_whispered_and_noop_updates_are_not() { … }
```

Caution: `run` spawns the real connect-or-spawn primitive, which would launch a daemon. For these tests, extract the message-handling core of `run` into `fn step(state: &mut LinkState, message: …)` (a refactor that changes no behavior) and test `step`; leave the socket loop untouched. `LinkState` holds `entry`, `armed`, `outbox`.

- [ ] **Step 2: Run to verify failure.**
- [ ] **Step 3: Implement** rules 1–5 (including the `step` extraction).
- [ ] **Step 4: Run** `cargo nextest run -p amon-wrapper`, then `just test`.
- [ ] **Step 5: Commit** — `git commit -m "Knock once, then whisper the link traffic into the stream"`

---

### Task 7: End-to-end — nested wrappers, no ssh

**Files:**
- Modify: `crates/amon-cli/tests/e2e.rs`

**Interfaces:**
- Consumes: the whole feature; the harness helpers already in `e2e.rs` (read the first ~200 lines before writing anything — there is a fake-daemon pattern near line 195 and a raw-register example near line 2005; reuse the file's existing helpers for PTY-spawning amon and for fake daemon sockets).

The test: an outer wrapper (playing "local") wraps a command that runs an inner wrapper (playing "remote") — the PTY chain stands in for ssh, which is legitimate because ssh's contribution is exactly "carries terminal bytes".

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn a_remote_agents_entry_crosses_the_terminal_stream() {
    // Two fake daemons: `local_daemon` for the outer wrapper, `remote_daemon`
    // for the inner one (use the harness's fake-daemon helper twice).
    // Outer amon, in a PTY, wrapping:
    //   env SSH_TTY=/dev/fake AMON_DAEMON_SOCKET=<remote.sock> \
    //       <amon binary> sh -c "sleep 5"
    // where <amon binary> is env!("CARGO_BIN_EXE_amon").
    //
    // Assert, against local_daemon, in order:
    // 1. an agent.register whose agent is "env" (the outer wrapper's own
    //    entry — it wraps `env`);
    // 2. a second agent.register with the SAME id whose agent is "sh":
    //    the mirrored remote entry (knock → answer → armed → whispered
    //    register). Give it a generous deadline (~10s).
    // 3. that entry's hostname is non-empty and its cwd is the test cwd.
    //
    // Then kill the inner process group and assert a further register
    // reverts the row to agent "env" (the Bye path), OR — if kill ordering
    // makes Bye racy — end the outer wrapper and assert the connection
    // closed. Prefer the Bye assertion; fall back only with a comment.
}
```

- [ ] **Step 2: Run to verify failure** — `cargo nextest run -p amon-cli remote_agents` — fails at the second-register assertion until Tasks 4–6 are correct together (if Tasks 4–6 are done, this may pass immediately; that is the point of the task — an integration gate, not new code).
- [ ] **Step 3: Fix whatever it finds.** Likely first failures: answer written before the inner wrapper's stdin scanner starts (startup ordering), or the probe emitted before `Screen` enabled tracking. Debug with the harness's transcript helpers.
- [ ] **Step 4: Run the full suite** — `just test`.
- [ ] **Step 5: Commit** — `git commit -m "Prove the whisper end to end with nested wrappers"`

---

### Task 8: Headless macOS — gate the desktop out

**Files:**
- Modify: `crates/amon-integration/src/lib.rs` (module declarations and re-exports)
- Modify: `crates/amon-integration/src/alias.rs` (`shell_config()`, line 47)
- Modify: `crates/amon-cli/src/main.rs` (the `focus` subcommand and `--duck` flags), `crates/amon-cli/src/doctor.rs`, `crates/amon-cli/src/setup.rs`
- Modify: `crates/amon-daemon/src/lib.rs` only if sound fails to compile on mac (it shouldn't — `pw-play` is spawned by name and degrades at runtime, `sound.rs:26-30`; leave it).

**Interfaces:**
- Consumes: `amon_integration::{statuses, candidates, all_targets, install, uninstall}` (call sites listed by `grep -n amon_integration crates/amon-cli/src`).
- Produces: a workspace that passes `cargo check --workspace --target aarch64-apple-darwin` with the desktop absent, and unchanged Linux behavior.

Gating approach — keep types, gate implementations and listings:
1. In `amon-integration/src/lib.rs`: `#[cfg(target_os = "linux")]` on `mod desktop; mod ducking; mod shims; mod bindings; mod udev;` and their `pub use`s. `fence` and `alias` stay (the alias needs fenced shell-config editing everywhere). The `IntegrationTarget` enum keeps its `Desktop` variant on every platform (no cfg on enum variants — match arms elsewhere would break); instead `all_targets()`/`candidates()`/`statuses()` omit Desktop off Linux, and `install(Desktop)`/`uninstall(Desktop)` return an error naming the platform.
2. `alias.rs::shell_config()`: on macOS return `~/.zshrc`, on Linux `~/.bashrc` (via `cfg!(target_os = "macos")`). The alias line syntax is identical in both shells; the symlink refusal and fence logic apply unchanged. Update the module comment that currently says "Bash only" (`alias.rs:44`) to state the per-platform rule and why.
3. `amon-cli`: `focus` subcommand gated to Linux (`#[cfg]` on the subcommand arm plus a clear error `amon focus needs a compositor; there is none on macOS` if invoked via an old script — prefer hiding it from `--help` AND erroring, both); `--duck`/`--no-duck` accepted but answered with a one-line "audio ducking is Linux-only" message off Linux (flags must not become parse errors — scripts pass them). `doctor` and `setup` largely follow `statuses()`/`candidates()` automatically; sweep both files for direct `desktop::`/`ducking::`/`shims::` calls (the grep above lists them, e.g. `main.rs:388-389`, `doctor.rs:11`) and gate each.
4. The wrapper's `hypr::spawn` is already env-guarded (`hypr.rs:263`) — no change; `libc::gethostname`, termios, `waitpid` are POSIX — no change.

- [ ] **Step 1: Add the cross-check target** — `rustup target add aarch64-apple-darwin`, then run `cargo check --workspace --target aarch64-apple-darwin` to enumerate today's failures. Expected: a burst of errors in `amon-integration`/`amon-cli` (this is the failing-test equivalent for a build-shape task). If the `libghostty-vt` build script cannot cross-build here, scope the check to `-p amon-integration -p amon-cli -p amon-daemon -p amon-protocol` and note that the full gate is Task 9's CI job.
- [ ] **Step 2: Implement the gating** (points 1–4).
- [ ] **Step 3: Verify both shapes** — `cargo check --workspace --target aarch64-apple-darwin` (or the scoped form) clean, AND `just test` on Linux fully green (the gates must not change Linux behavior; the e2e suite is the proof).
- [ ] **Step 4: Commit** — `git commit -m "Headless macOS: the desktop is Omarchy's, the CLI is everyone's"`

---

### Task 9: Release the Mac build — CI and installer

**Files:**
- Modify: `.github/workflows/ci.yml` (add a macOS build+test job)
- Modify: `.github/workflows/publish.yml` (add a macOS release job)
- Modify: `install.sh` (target selection, lines 24-31)

**Interfaces:**
- Consumes: the existing ubuntu jobs as templates; `mise` provides Zig 0.15.2 and nextest from `.mise.toml` via `jdx/mise-action` (already used, `publish.yml:65`).
- Produces: a release asset named `amon-$TAG-aarch64-darwin.tar.gz` alongside the existing `amon-$TAG-x86_64-linux.tar.gz`, same tarball layout (the `amon` binary + license files — copy the staging steps from the linux job, `publish.yml:109-111`).

1. `ci.yml`: add `build-macos` on `runs-on: macos-14` (Apple Silicon runner): checkout, mise-action, `cargo build --workspace`, `cargo nextest run --workspace` — with the `--no-fail-fast` and per-test-process settings the justfile's `test` recipe uses (copy the exact nextest invocation from `justfile`; do not invent flags). Linux-only tests must already be `cfg`-gated by Task 8; any test that fails on mac for environment reasons gets a `#[cfg(target_os = "linux")]` with a one-line justification, never an `#[ignore]`.
2. `publish.yml`: duplicate the build job as `build-macos` on `macos-14`, producing and uploading the darwin tarball to the same release. Mirror every step that pins or verifies (sha256 emission included) — read the whole existing job first and keep the two jobs structurally parallel so a future edit touches both or fails review.
3. `install.sh`: replace the hard refusal with a case:
   - `Linux/x86_64` → `amon-$TAG-x86_64-linux.tar.gz` (unchanged)
   - `Darwin/arm64` → `amon-$TAG-aarch64-darwin.tar.gz`
   - `Darwin/x86_64` → refuse: `prebuilt amon for Mac is Apple Silicon only. build from source instead: …#building`
   - anything else → the existing refusal.
   On Darwin, after install, check `case ":$PATH:" in *":$HOME/.local/bin:"*)` and, when absent, print the one line to add to `~/.zshrc` — print, never edit (`install.sh` owns no user file).

- [ ] **Step 1: Write the workflow and installer changes.**
- [ ] **Step 2: Verify locally what can be** — `shellcheck install.sh` (if available; otherwise `bash -n install.sh`), and `actionlint` on the workflows if available. Simulate the installer's case logic: `OS=Darwin ARCH=arm64 bash -n` plus by-hand review of the case block.
- [ ] **Step 3: Push the branch and watch the `ci.yml` mac job** — this is the real test for Task 8's gating. Fix what it finds (expect Zig/Ghostty surprises here rather than in Rust code). Do not merge anything; CI green on the branch is the gate.
- [ ] **Step 4: Commit** — `git commit -m "Build and install amon on Apple Silicon"` (commit before pushing, obviously; iterate with follow-up commits as CI demands).

---

### Task 10: Documentation — ADRs, README, protocol notes

**Files:**
- Create: `docs/adr/0017-agent-events-ride-the-terminal-stream.md`
- Create: `docs/adr/0018-a-remote-agents-lifetime-is-its-ssh-session.md`
- Create: `docs/adr/0019-macos-is-a-headless-target.md`
- Modify: `README.md`

Numbering: `0016` exists on another branch (`micro2`); if it has merged by execution time these become 0017–0019 as written, otherwise renumber to the next free numbers — check `ls docs/adr` first.

1. Each ADR in the house style (read `0002-daemon-lifecycle.md` and `0007-…` as models: decision in the title, consequences section, considered options where real alternatives were rejected). Content sources: the spec's "Decisions to record" section plus its knock and lifecycle sections. 0017 must state the passthrough cost honestly the way ADR-0007 does: the promise is bent by exactly one discardable probe per ssh session, and by whispers only where a listener proved itself.
2. README: a short section under "How it works" — running an agent under amon inside `amon ssh <host>` (or any amon-wrapped ssh) shows it on the local desktop; requirements (amon on both ends, an interactive session); the tmux/mosh limits, one line each. A Mac paragraph in "Installing": the same install command works on Apple Silicon; what exists there (status, setup for agents, doctor, the daemon) and what doesn't (bar, panel, focus, sounds). Match the README's voice — plain sentences, no feature-list bullets where prose does.
3. `CHANGELOG.md`: one entry per phase under the unreleased heading, following the file's existing format.

- [ ] **Step 1: Write the three ADRs.**
- [ ] **Step 2: Write the README and CHANGELOG changes.**
- [ ] **Step 3: Self-check** — every claim in the docs is something a task above actually built; no promised flag or command that doesn't exist.
- [ ] **Step 4: Commit** — `git commit -m "Record the ssh whisper and the headless Mac in ADRs and the README"`

---

## Self-Review (completed at planning time)

- **Spec coverage:** knock (T1, T3, T6), strip/mirror (T2, T4, T5), field boundary table (T5 rules 1–2), one-row cycle (T5 rules 1, 5), tee + heartbeat + bye (T6), e2e (T7), macOS headless incl. zsh (T8), Apple-Silicon-only CI + installer (T9), ADRs (T10). The spec's "sequence format" constraints are T1's module doc; its trust posture is T2's discard tests.
- **Known open point carried into execution:** whether `Observer` tests need the `Mirror` extraction (T5 Step 1 names the fallback); whether `cargo check --target aarch64-apple-darwin` works from Linux (T8 Step 1 names the scoped fallback and T9's CI as the true gate).
- **Type consistency:** `WhisperFrame`/`OutputScan`/`InputScan`/`Outbox` names match across T1–T6; `RemoteReport` is defined in T4 and consumed in T5; `DaemonLink::spawn(version, Option<Outbox>)` changed in T6 matches the call-site note in T6.
