# Showing Claude Desktop sessions in amon, and what a remote client can and cannot reach

Status: exploration (2026-08-31), against Anthropic's official Linux package
`claude-desktop` 1.40609.0 (amd64, Electron 42.10.0, bundled
`@anthropic-ai/claude-agent-sdk` 0.3.247), the AUR `claude-desktop`
1.40609.0-1 PKGBUILD that repackages it, and amon main (`24bcbd2`). Nothing
here is implemented.

**Method caveat, up front.** Claude Desktop is *not installed on this
machine* (`pacman -Q claude-desktop` → not found; no `/opt/claude*`, no
`~/.config/Claude`), and the brief for this research was read-only: do not
install, do not start services. So unlike
`docs/research/codex-desktop-integration.md`, this note has **no live
verification section**. Every claim below is read out of the shipped
artifact — the `.deb`, its maintainer scripts, the unminified GJS helper it
ships, the Go helper binary's symbols, and hand-deminified `app.asar` — or
out of Anthropic's own documentation. Claims that would need the app running
are marked **unknown** with the experiment that settles them, in
"Next experiments". Nothing is asserted from a news write-up.

The question: OpenAI's desktop app turned out to be unreachable (that note is
parked). Anthropic now ships an official Linux desktop that runs Chat, Cowork
and Claude Code sessions amon never wrapped. Can a client on the box — amon —
connect to it and enumerate its sessions and their state?

Short answer: **enumerate, yes; connect, no; state, only indirectly.**

- There is **no control socket, port, or IPC surface on the app**. The app
  hosts no server for third parties; it *refuses to start* if a debugging
  switch is on its command line.
- Sessions are nevertheless **plainly readable on disk**, in a layout
  Anthropic itself reads from outside the app: the GNOME Shell search
  provider they ship is a 347-line unminified GJS script that enumerates
  Claude Code sessions from `~/.config/Claude/claude-code-sessions/` and
  opens one with a `claude://` deep link. That is a first-party reference
  implementation of exactly the read amon wants.
- Those records carry identity, title, cwd, archived-ness and
  `lastActivityAt`. They do **not** carry run state. Working / Blocked /
  Idle lives in the app's memory and, for local Claude Code sessions, in the
  `claude` child process it spawns and in that process's hooks — which is
  where amon's opening is.
- Cowork runs in a QEMU/KVM VM. The one unix socket in the whole picture
  belongs to that VM's helper, is same-uid-gated, and speaks VM lifecycle,
  not sessions.
- Remote access is inverted from what the question assumes: the desktop is
  an SSH *client* of other machines, and its remote surface (Remote Control)
  is a relay through Anthropic's servers to claude.ai — not a listener on
  this box.

## What the Linux package actually is

Verified, from the artifact. The `.deb` was downloaded from Anthropic's own
pool and extracted under `~/.cache/claude-research/claude-desktop/`; its
sha256 `a96e96ff8eb4d4d7ffa785aba7fc23f8684b12ac83ed2ef4060f0f09f4177a98`
matches `sha256sums_x86_64` in the AUR `claude-desktop` PKGBUILD, so the AUR
package and this read are the same bytes.

- Source of truth:
  `https://downloads.claude.ai/claude-desktop/apt/stable/pool/main/c/claude-desktop/claude-desktop_1.40609.0_amd64.deb`,
  indexed in
  `https://downloads.claude.ai/claude-desktop/apt/stable/dists/stable/main/binary-amd64/Packages.gz`
  (33 published versions, 1.17180.0 → 1.40609.0; `Origin: Anthropic`,
  `Label: Anthropic`, `Suite: stable` per `dists/stable/Release`).
- `control`: `Maintainer: Anthropic PBC <support@anthropic.com>`,
  `Installed-Size: 563639`. **`Depends:` contains no `socat` and no QEMU.**
  `Recommends: … qemu-system-x86, ovmf, virtiofsd`. (The AUR PKGBUILD
  promotes the Recommends to hard `depends` deliberately — its own comment
  says so — and adds `socat`, which upstream does not require. See "What
  socat is for".)
- `postinst` (root, via dpkg) does three things and says so in comments:
  writes an `flags=(unconfined)` AppArmor profile for Ubuntu 24.04+ userns
  restriction; registers Anthropic's apt repo
  (`https://downloads.claude.ai/claude-desktop/apt/stable`, key fingerprint
  `31DD DE24 DDFA B679 F42D 7BD2 BAA9 29FF 1A7E CACE`, opt-out via
  `CLAUDE_DESKTOP_ADD_REPO="false"` in `/etc/default/claude-desktop`); and
  **registers a GNOME Shell search provider** — an `.ini` plus a D-Bus
  `.service` that starts a bundled GJS script on demand. `postrm` removes
  them and notes it deliberately leaves "per-user residue (`~/.config/autostart`
  entry, per-browser `NativeMessagingHosts` manifests)" alone.
- Payload: `/usr/bin/claude-desktop` is a **symlink** to
  `/usr/lib/claude-desktop/claude-desktop` (the Electron binary — no wrapper
  script, so no ozone/Wayland flags are applied by the package);
  `/usr/lib/claude-desktop/version` = `42.10.0`.
- `usr/share/applications/com.anthropic.Claude.desktop`: `Exec=claude-desktop %U`,
  `StartupWMClass=com.anthropic.Claude` ("Matches the Wayland app_id / X11
  WM_CLASS Chromium derives from package.json desktopName"),
  `SingleMainWindow=true` ("second-instance just focuses mainWindow"),
  `MimeType=x-scheme-handler/claude`, and two actions:
  `claude://claude.ai/new?surface=chat&source=desktop_action` and
  `claude://code/new?source=desktop_action`.
- `resources/`: `app.asar` (the app), `cowork-linux-helper` (static Go ELF,
  3.3 MB), a bundled `virtiofsd`, `smol-bin.x64.img` (26 MB boot image),
  `chrome-native-host` (native-messaging host for the Claude browser
  extension), `gnome-search-provider/`, and `ion-dist/` (a Claude web SPA
  bundle — `<title>Claude</title>`, `frame-shell.html`, `frame-connect.html`,
  ~160 MB — the in-app web surface; not an IPC surface).
- `app.asar` `package.json`: `"name": "@ant/desktop"`,
  `"productName": "Claude"`,
  `"desktopName": "com.anthropic.Claude.desktop"`, main
  `.vite/build/index.pre.js`, `"@anthropic-ai/claude-agent-sdk": "0.3.247"`,
  and workspace deps that name the moving parts:
  `@ant/claude-ssh`, `@ant/chrome-native-host`, `@ant/rfb-client`,
  `@ant/cowork-win32-service`, `@anthropic-ai/electron-devtools-mcp`.

Anthropic's docs agree and add the platform envelope
(https://code.claude.com/docs/en/desktop-linux): beta; Ubuntu 22.04+/Debian
12+; x86_64 or arm64; "On distributions that aren't Debian-based, such as
Fedora or Arch, run the CLI instead"; Cowork needs KVM, `/dev/vhost-vsock`
and the `kvm` group; Computer Use and Dictation are not in the Linux beta;
"Quick Entry global hotkey: works on X11. On native Wayland it requires your
desktop environment's GlobalShortcuts portal."

**userData on Linux is `~/.config/Claude`** — verified not by inference from
Electron's defaults but from Anthropic's own shipped code:
`resources/gnome-search-provider/searchProvider.js` computes it as
`GLib.build_filenamev([GLib.get_user_config_dir(), "Claude"])`, overridable
by `CLAUDE_USER_DATA_DIR`.

## 1. Local IPC surface

### There is no app-owned socket, port, or CDP endpoint

Verified. Three separate reads say the same thing.

**a. Debugging switches are a hard refusal, not a toggle.** In
`.vite/build/index.pre.js` (the Electron main entry, before `app.ready`):

```
bH = [`remote-debugging-port`,`remote-debugging-pipe`,`ignore-certificate-errors`,
      `host-resolver-rules`,`host-rules`,`disable-web-security`,`log-net-log`,
      `net-log-capture-mode`,`ssl-key-log-file`,`renderer-cmd-prefix`,
      `utility-cmd-prefix`,`gpu-launcher`,`zygote-cmd-prefix`,
      `ppapi-plugin-launcher`,`nacl-loader-cmd-prefix`,`browser-subprocess-path`]
...
if (SH(process.argv) && !kH()) {
  fs.writeSync(2, `Claude: refusing to start — a debugging or network-override switch is present on the command line.\n`)
  process.exit(1)
}
```

`kH()` is the only escape: it requires `CLAUDE_CDP_AUTH` **and**
`CLAUDE_USER_DATA_DIR`, splits the token into `timestamp.base64(userDataDir).sig`,
requires the middle segment to equal `CLAUDE_USER_DATA_DIR` exactly, requires
the token to be under 5 minutes old (`EH = 3e5`), and verifies an Ed25519
signature against a public key compiled into the binary
(`MCowBQYDK2VwAyEApH/vaEiLV0sNY/eS+Ct/IMbMqw8i/vC/cNC84BAbBq8=`). Only
Anthropic holds the private key. Immediately after, when the token is absent
and the app is packaged, it also `delete process.env.CLAUDE_USER_DATA_DIR`
and `delete process.env.SSLKEYLOGFILE`.

So: no CDP, no `--remote-debugging-port`, and no way for a third party to
turn one on. This is a stronger position than Codex desktop's — there the
surface existed and was merely unreachable by regression; here it is
deliberately closed.

**b. The app opens no listening socket of its own.** A search of the
extracted `app.asar` for `net.createServer` / `http.createServer` /
`WebSocketServer` finds hits only inside the vendored `ws` package's own
README and `lib/websocket-server.js` — never called from the app's chunks.
There is a `node:inspector` debug menu item ("Enable Main Process Debugger",
`.vite/build/index.chunk-Ci77nY41.js`) but it is a developer-menu action that
calls `inspector.open()` in the running process, and its follow-up only knows
how to open `chrome://inspect` on darwin and win32.

**c. Single-instance is argv forwarding, not IPC.** `app.requestSingleInstanceLock()`
plus a `second-instance` handler that parses the forwarded argv as a
`claude://` deep link (`.vite/build/index.chunk-Ci77nY41.js`, near
`startup: oversized deep-link argv; relaunching with it capped`). That is a
one-way "go here" channel — useful to amon as a *focus* hop, useless as a
query.

### The one real unix socket: the Cowork VM helper

Verified from code and from the helper binary's symbols.

Path (`.vite/build/index.chunk-Ci77nY41.js`):

```
fIt() = process.env.XDG_RUNTIME_DIR || join(os.tmpdir(), `claude-cowork-${uid}`)
mIt() = join(fIt(), `claude-cowork-vm.sock`)
```

The app **spawns** `resources/cowork-linux-helper -socket <that path>` and
**connects** to it; the helper is the server (`[server] listening on %s` in
the binary's strings). Framing is a 4-byte big-endian length prefix around a
JSON object; requests are `{method, params?, id?}` and replies
`{id, success, result|error}` (`tA` / `zIt` / `qIt` in the same chunk, 10 MB
message cap). Both a persistent pipe and one-shot connections are used, and
the helper broadcasts events to subscribers (`[server] dropping %s event for
slow subscriber %v`).

Methods the app calls, extracted by matching the call sites:

```
isRunning  isGuestConnected  isProcessRunning  readFile
configure  startVM  stopVM  mountPath  setDebugLogging
spawn  kill  writeStdin  sendGuestResponse
getNetworkDrives  getSessionsDiskInfo  deleteSessionDirs  pruneSessionCaches
installSdk  addApprovedOauthToken
```

Access control is real and is on the accept side: the helper links
`golang.org/x/sys/unix.GetsockoptUcred` and carries the strings
`SO_PEERCRED: %w` and `peer uid %d != our uid %d` alongside
`[server] rejecting connection: %v`. The client side mirrors it — the app
refuses to connect without `@ant/claude-native`'s `connectUnixSocketSameUid`
("refusing to connect to the cowork helper socket without peer-credential
verification"), and when `XDG_RUNTIME_DIR` is unset it creates its fallback
dir at mode 0700 and refuses a socket it does not own.

What this means for amon: a same-uid process **can** connect (amon runs as
the user), and this is the only speakable local API in the package. But it is
a VM-lifecycle API. There is no `listSessions`, no status stream, no
per-session identity — `getSessionsDiskInfo` / `deleteSessionDirs` /
`pruneSessionCaches` are disk-hygiene calls over session *directories*.
Speaking it to enumerate agents would be both wrong and rude: `spawn`,
`kill` and `writeStdin` are on the same socket, so anything amon sent would
be indistinguishable from the app driving its own VM. Not a candidate.

### The D-Bus surface that *does* answer the question

Verified, and the most useful thing in the whole package:
`/usr/lib/claude-desktop/resources/gnome-search-provider/searchProvider.js` —
347 lines of unminified, commented GJS, D-Bus-activated as
`com.anthropic.Claude.SearchProvider` on `org.gnome.Shell.SearchProvider2`
(`com.anthropic.Claude.search-provider.ini`,
`com.anthropic.Claude.SearchProvider.service` →
`/usr/bin/gjs -m …/searchProvider.js`).

Its header states the whole design: *"GNOME Shell search provider
(org.gnome.Shell.SearchProvider2), D-Bus activated. Reads session titles and
ids only; opens results as claude:// URLs."*

It is a standalone process that does not talk to the app at all. It:

1. reads `<userData>/os-entry-points.json` and requires
   `{enabled: true, account: <uuid>, org: <uuid>}`;
2. lists `<userData>/claude-code-sessions/<account>/<org>/local_*.json`,
   newest-mtime first, capped at 500 files and 10 MB each;
3. keeps `{sessionId, title, cwd, folder: basename(cwd), at: lastActivityAt}`
   for every record whose `sessionId` matches `/^local_[A-Za-z0-9-]{1,64}$/`
   and whose `isArchived !== true`, sorted by `lastActivityAt` desc;
4. on activation opens
   `claude://code/continue?session=<sessionId>&source=gnome_search` via
   `Gio.DesktopAppInfo.new("com.anthropic.Claude.desktop").launch_uris()`.

This is Anthropic shipping, on Linux, the exact read-and-jump amon would
write. It is not an API amon should *call* (it is a search provider, and it
exits after 120 s idle), but it is the authoritative statement of the file
layout and the deep link, and it settles the licensing/ethics question about
reading those files: the vendor reads them from a separate process itself.

### Native messaging, and what socat is for

- `resources/chrome-native-host` + a manifest written to each browser's
  `NativeMessagingHosts` dir as
  `com.anthropic.claude_browser_extension.json`, `type: stdio`, with
  `allowed_origins` pinned to three `chrome-extension://` ids
  (`dihbgbndebgnbjfmelmegjepbnkhlgni`, `fcoeoabgfenejglbffodgkkbkcdhcgfn`,
  `dngcpimnedloihjnnfngkgjoidhnaolf`). Chrome native messaging is
  browser-launched stdio with an origin allowlist — not reachable by amon,
  and it carries browser-tool traffic (`mcp__claude-in-chrome__…`), not
  sessions.
- **`socat` has nothing to do with IPC, the VM, or the desktop app.** In
  `app.asar` it appears in exactly two places, both inside the bundled
  Claude Code CLI's settings schema
  (`.vite/build/index.chunk-Clv3sLGS.js`): the managed-settings key
  `socatPath`, described as *"Linux/WSL only: Absolute path to the socat
  binary used for the sandbox network proxy. Overrides auto-detection via
  PATH. Only honored from admin-controlled managed settings"* (beside
  `bwrapPath` and `ripgrep`), and an error classifier that maps output
  mentioning `ripgrep` / `bubblewrap` / `socat` to
  `category: sandbox_deps_missing`. Anthropic documents the same thing:
  https://code.claude.com/docs/en/sandboxing — *"On Linux and WSL2, the
  sandbox relies on two packages: `bubblewrap` … and `socat`: the relay used
  to route network traffic through the sandbox proxy."* The official `.deb`
  does not depend on socat; the AUR maintainer added it so the bundled CLI's
  sandboxed Bash tool works. Treat the AUR dependency list as the packager's
  opinion, not upstream's manifest.

### What amon can actually address, then

One inbound verb, no query: `claude://` deep links, delivered either by
`xdg-open` or by `claude-desktop "<url>"` (the running instance receives it
through `second-instance`). Routes seen in the shipped code:

| link | source |
| --- | --- |
| `claude://code/continue?session=<local_uuid>` | `searchProvider.js` `ActivateResult`; also `?session=last` |
| `claude://code/new` | `.desktop` action; `Iyr()` OS-entry table |
| `claude://claude.ai/new?q=…&surface=chat` | `searchProvider.js`; `.desktop` action |
| `claude://code/needs-input` | `Iyr()`, described as *"open the session that has waited longest for a permission answer"*, labelled **"Sessions Waiting for You"** |
| `claude://cowork/shared-artifact?uuid=…` | cowork artifact sharing |
| `claude://resume` | error strings for a failed CLI-session import |
| `claude://claude.ai/mcp-auth-callback/sdk` | MCP OAuth callback |

`claude://code/needs-input` is worth flagging: the app itself has a
first-class "which session is blocked" notion and exposes it as a *jump*, and
`disableDeepLinks` is a managed-settings kill switch for the whole scheme.
That there is a needs-input route and no needs-input *query* is the shape of
this entire integration.

## 2. Session enumeration

### The on-disk layout

Verified from two independent readers in the package — the search provider
above, and the app's own "import sessions from another Claude install"
scanner in `.vite/build/index.chunk-Ci77nY41.js` (log prefix
`[claude-ai-import:local-scan]`).

```
~/.config/Claude/                                   # userData; $CLAUDE_USER_DATA_DIR overrides
  os-entry-points.json                              # {enabled, account, org} — the search provider's gate
  config.json  Local State  claude_desktop_config.json  developer_settings.json
  claude-code/<version>/claude                      # the Claude Code CLI the app downloads and runs
  claude-code-vm/<version>/claude                   # the copy it runs inside the Cowork VM
  claude-ssh-remote/<version>/claude-ssh-<platform> # the remote-host daemon it deploys over SSH
  vm_bundles/claudevm.bundle/{rootfs.img,kernel,initrd,sessiondata.img}
  logs/
  claude-code-sessions/<accountUuid>/<orgUuid>/     # Claude Code sessions
      local_<uuid>.json                             # one record per session
      local_<uuid>/            (or local_<first-8-hex>/ short form)
          .claude/projects/<slug>/<cliSessionId>.jsonl
          .claude/debug/*.txt, *-diagnostics.json
          outputs/
      spaces.json  scheduled-tasks.json  artifacts.json  cowork_settings.json  memory/
  local-agent-mode-sessions/<accountUuid>/<orgUuid>/  # Cowork + Chat sessions, same shape
```

Citations for each piece: the two tree names are a constant pair throughout —
`AB = 'claude-code-sessions'`, `ME = 'local-agent-mode-sessions'`, and
`index.pre.js` lists both in the set of userData entries the app owns. The
`<account>/<org>` nesting is `oPr()` (two `readdir` levels, names matched
against `/^[0-9a-fA-F-]{8,36}$/`, symlinks refused). The record filename rule
is `_Pr(name) = name.startsWith('local_') && name.endsWith('.json')` and the
parse gate is `yPr()`: valid iff `typeof sessionId === 'string' &&
typeof createdAt === 'number' && typeof lastActivityAt === 'number'`. The
per-session directory and the transcript path are `oDt()` and `EPr()`:
`join(storageDir, sessionId)` (falling back to the 8-hex short name) then
`.claude/projects/<slug>/<cliSessionId>.jsonl`. `getSessionStorageDir()` +
`.claude/debug` is used by the diagnostics bundler in the same chunk. The VM
bundle dir is `join(userData, 'vm_bundles', 'claudevm.bundle')` (`rvn()` /
`$H()`), downloaded from `https://downloads.claude.ai/vms/linux/<arch>/<sha>`.

### What a record contains

Verified. The app's own persisted projection is explicit
(`UPr()`, `.vite/build/index.chunk-Ci77nY41.js`):

```
{ sessionId, cliSessionId, title, sessionType, createdAt, lastActivityAt,
  hostLoopMode, isArchived, spaceId }
```

and readers additionally consume `cwd` (the search provider),
`importedFrom`, `userSelectedProjectUuids`, and `isArchived` (the import
scanner, which also derives `kind` as `code` when the record is in the
claude-code tree, else `chat` when `sessionType === 'chat'`, else `cowork`).
`spaces.json` beside the records maps `spaceId` → `{id, name, projects[],
folders[]}`.

So a third party gets, per session, for free: **stable id, CLI session id,
title, cwd, kind, created/last-activity timestamps, archived flag, project
grouping** — plus the full Claude Code JSONL transcript, which is byte-for-byte
the format amon already knows from `~/.claude/projects/*.jsonl`.

**`isRunning` is not in that list, and that is the whole problem.** The app
carries `isRunning` in memory — `getAllSessions()` returns
`{sessionId, isRunning, isArchived, createdAt, lastActivityAt}` to the
diagnostics bundler, and the scheduled-tasks bridge maps
`isRunning: t.Qs(e)` — and it has a blocked notion too
(`(e.pendingToolPermissions?.length ?? 0) > 0`, and
`status_category ∈ {blocked, need_input, failed}` folded to
`needs_input` vs `turn_complete` by `ttr()`). None of it is persisted. The
record on disk is a catalogue entry, not a status board.

### Where the desktop's Claude Code sessions actually run — and the opening

Verified: **the desktop does not embed Claude Code, it spawns the real
binary.** It uses `@anthropic-ai/claude-agent-sdk` 0.3.247 with
`pathToClaudeCodeExecutable` / `executableArgs`
(`.vite/build/index.chunk-Clv3sLGS.js`), and the binary is the one it manages
itself at `~/.config/Claude/claude-code/<version>/claude`
(`fV.storageDir = join(userData, 'claude-code')`, log prefix `[CCD]`), with
`CLAUDE_CODE_LOCAL_BINARY` as an override. So while a local Claude Code
session is running there is a **`claude` process on the host, descended from
the Electron main process** — kernel-visible, `/proc`-detectable, with a cwd.

Verified, and load-bearing: the host session spawn passes

```
settingSources: [`user`, `project`, `local`]
```

(`.vite/build/index2.chunk-lz5Un0_t.js`, the CCD session start), and Cowork's
host loop passes `settingSources: ['user']`
(`.vite/build/index2.chunk-CGh0k4sB.js`). "user" settings are
`<CLAUDE_CONFIG_DIR>/settings.json` — i.e. **`~/.claude/settings.json`, where
amon installs its Claude Code hook today**
(`crates/amon-integration/src/vendor/integration/assets/claude/amon-agent-state.sh`).

Two consequences, one certain and one not:

- **Certain:** desktop-run Claude Code sessions load the same user settings
  file as terminal sessions, so amon's hook entries are *present* in those
  sessions. They currently do nothing there: the script's first three guards
  are `[ "${AMON_ENV:-}" = "1" ]`, `[ -n "${AMON_SOCKET_PATH:-}" ]`,
  `[ -n "${AMON_AGENT_ID:-}" ]` — env the amon *wrapper* sets, which the
  Electron app does not. Exactly the failure mode the Codex note found, for
  the same reason.
- **Certain, and a negative result worth recording:** nothing in the package
  suppresses Claude Code hooks. Grepping the whole asar for
  `disableHooks|noHooks|skipHooks|hooksDisabled|DISABLE_HOOKS` returns exactly
  one token, `skipHooks`, and it is git's: `commitAllChanges(…, {skipHooks})`
  appends `--no-verify` to `git commit` for the WIP commit taken before a
  branch switch (`GitStatusService.commitWipForBranchSwitch`,
  `index.chunk-BN6cB1LT.js` / `index2.chunk-lz5Un0_t.js`). Unrelated to Claude
  Code hooks, and there is no other kill switch anywhere in the bundle.
- **Unknown:** whether those host sessions run with the default
  `CLAUDE_CONFIG_DIR` (`~/.claude`) or a per-session one. The Cowork path
  demonstrably overrides it (`env: {…, CLAUDE_CONFIG_DIR: M, …}` in
  `index2.chunk-CGh0k4sB.js`); the CCD host path builds its env in
  `buildSessionEnv()` and **no `CLAUDE_CONFIG_DIR` assignment appears in that
  chunk** — only `${CLAUDE_CONFIG_DIR:-$HOME/.claude}` inside *remote* shell
  snippets and a `canTrustTranscriptMissingVerdict()` check that asserts every
  observed `CLAUDE_CONFIG_DIR` equals the default. The import scanner hedges
  by searching both `<sessionStorageDir>/.claude/projects` **and**
  `~/.claude/projects` for `code`-kind sessions, which is consistent with
  either. Narrowing it: the bundle treats a non-default config root as an
  explicit, user-visible opt-in — the relocation error string names the only
  two supported forms, *"symlink the whole config root at the home level
  (`~/.claude`) or point `CLAUDE_CONFIG_DIR` at the relocated directory via
  Desktop Settings"* (`index.pre.js`) — so `~/.claude` is the default and a
  move is something the user chose. That is not proof about the CCD spawn's
  own env, which only `/proc` can settle. It decides whether desktop sessions
  write into the same `~/.claude/projects` tree the CLI uses, and which
  `settings.json` their hooks are read from — see experiment 3.

### So: can state be derived?

| signal | available to a third party | quality |
| --- | --- | --- |
| session exists, id, title, cwd, kind, archived | **yes**, `local_*.json` | exact, vendor-sanctioned |
| last activity | **yes**, `lastActivityAt` | a clock, not a state |
| running vs. not | **inferred**, via the `claude` child process on the host (`/proc`, parent = Electron main) | good for Cowork/host sessions; blind to anything running only inside the VM guest |
| Working / Blocked / Idle | **not on disk.** Reachable only by (a) Claude Code hooks in user settings, if they fire, or (b) tailing the session's JSONL transcript | (a) is the honest channel; (b) is polling a private file |
| "seen" | **no source at all** | same gap as Codex desktop; the app is not a terminal, so ADR-0007 focus reporting does not apply |
| blocked-session jump | **yes**, `claude://code/needs-input` | a verb, not a query |

## 3. The Cowork VM

Verified from the app's launcher code and the helper binary's symbols.

- Guest: QEMU + KVM. `WFt = 'qemu-system-x86_64'` (aarch64 on arm),
  firmware probed at `/usr/share/OVMF/OVMF_CODE_4M.fd`,
  `/usr/share/OVMF/OVMF_CODE.fd`, or `/usr/share/AAVMF/AAVMF_CODE.fd`;
  virtiofsd at `/usr/libexec/virtiofsd` or `/usr/bin/virtiofsd`, else the
  bundled `resources/virtiofsd`. Boot image `resources/smol-bin.x64.img`
  plus a downloaded bundle (`rootfs.img`, `kernel`, `initrd`) and a
  per-install `sessiondata.img`. (The AUR PKGBUILD's symlink shims exist
  precisely because these Debian paths are hardcoded.)
- Preflight: the app checks `access(/dev/kvm)` and `access(/dev/vhost-vsock)`
  and reports `kvm_permission_denied`, `vhost_vsock_module_missing`,
  `vhost_vsock_kernel_unsupported`, `virtualization_tools_missing` — the same
  vocabulary as https://code.claude.com/docs/en/desktop-linux.
- The boundary, from the helper's own QEMU argument strings:
  - **vsock** — `vhost-vsock-pci,guest-cid=%d`; the helper runs a
    `guestRPCServer` on a `vsock.Listener` (`github.com/mdlayher/vsock`
    v1.2.1 is in its build info). This is host↔guest control and stdio
    (`[wire] received message: type=%s method=%s event=%s`,
    `guest connected from %s`, `guest disconnected`).
  - **virtiofs** — `vhost-user-fs-pci,chardev=virtiofs0,tag=claudeshared`
    over `virtiofsd-%d.sock`, started with `--shared-dir` and
    `--sandbox none`. One tag, `claudeshared`. Host directories the user
    grants are mounted through it: the app's mount map is
    `{ '<name>': {path: <host path>, mode: 'ro'|'rw'|'rwd'} }` and the
    helper has a `mountPath` RPC.
  - **block** — `virtio-blk-pci,drive=sessiondisk` and `drive=smolbindisk`.
  - **QMP** — `{"execute":"qmp_capabilities"}`, `system_powerdown`.
- Nothing about a Cowork session is exported to the host as an API. What
  crosses to the host is: the helper's own unix socket (same-uid gated, VM
  lifecycle), the shared host directories the user granted (so the *work* is
  visible as ordinary files), `kernel-console.log` and helper stdout/stderr
  into `~/.config/Claude/logs`, and the session's record + transcript under
  `local-agent-mode-sessions/`.
- **Is a Cowork session visible from the host?** Its existence and metadata,
  yes (the record file). Its *process*, no — the agent runs inside the guest,
  and no `claude` process for it appears in the host `/proc`. The host-side
  `claude` process signal therefore covers Claude Code sessions and Cowork's
  host loop, and is blind to work happening in the guest. **Unknown** exactly
  where that line falls in 1.40609.0 — the code has both a `hostLoopMode`
  field on records and a VM spawn path (`s.spawn(…, 'claude', …)` with
  `CLAUDE_CONFIG_DIR` pointing into the guest mount) — see experiment 4.

## 4. Remote access

Verified, and the direction is the opposite of the question's premise.

- **No first-party headless or listen mode.** Nothing in the package binds a
  port or accepts a network connection. `claude-desktop --help` was not run
  (not installed), but the switch blocklist above means an operator cannot
  add one either.
- **The desktop is an SSH client, not a server.** `@ant/claude-ssh` is a
  first-party daemon the app *deploys to other machines*: it downloads
  `claude-ssh-<platform>` from
  `https://downloads.claude.ai/claude-ssh-releases` into
  `~/.config/Claude/claude-ssh-remote/<version>/`, uploads it (remote fetch,
  else SFTP with resumable upload), installs a matching `claude` CLI there,
  and runs `claude-ssh --serve --socket <path> --token-file <path>` on the
  remote host, reattaching with a persisted token
  (`.vite/build/index.chunk-ISElr9gv.js`). Managed settings gate it:
  `sshHostAllowlist`, beta key `sshRemoteSessions`. So the *remote box* gets
  a socket; this box does not.
- **The account-level surface is Remote Control, relayed by Anthropic.**
  https://code.claude.com/docs/en/remote-control: "Remote Control connects
  claude.ai/code or the Claude app for iOS and Android to a Claude Code
  session running on your machine… Claude keeps running locally the entire
  time." Sessions appear in "the session list at claude.ai/code". The desktop
  exposes the same toggle (docs: "**Desktop app**: Settings > Claude Code >
  Enable remote control by default"), and the app carries the settings to
  match — `ccRemoteControlDefaultEnabled`, `remoteControlStayReachable`,
  `remoteControlExcludedFolders`, `remoteControlPinnedFolders`,
  `remoteControlSpawnMode: 'same-dir'|'worktree'`,
  `remoteControlAtStartup` — and the user-facing string *"Remote Control was
  started from your phone or claude.ai and stays on for this session."*
  This is opt-in per session (or per install), account-scoped, and reaches
  the machine through Anthropic's servers.
- **A public API that lists an account's sessions: none found.** I did not
  find one in Anthropic's docs, and I am not going to assert its absence from
  a failed search — **unverified absence**. Either way it would be the wrong
  tool: it would list Remote-Control-enabled and web sessions, not the
  desktop's local ones, and it would need the user's credentials.
- **Is Claude Code's own session data a better proxy?** For this question,
  yes — decisively. `~/.claude/projects/<slug>/<uuid>.jsonl` plus
  `~/.claude/settings.json` hooks are documented, stable, shared by the CLI
  and (per §2) loaded by the desktop's sessions, and amon already owns an
  integration there. Any remote client is better served by SSH-to-the-box and
  reading that than by anything the desktop offers.

## 5. Verdict for amon

### Against the `Hosted` contract

`crates/amon-daemon/src/runtime/mod.rs` states the contract herdr and luvus
satisfy: *"Each runtime is a detached server owning the agents' terminals, a
tty-owning client the user typed, and a JSON-lines control socket."* Claude
Desktop satisfies **none of the four load-bearing parts**, and adopting it
would need a different projection, not a third `impl Hosted`.

| `Hosted` requirement | Claude Desktop |
| --- | --- |
| `COMM` + `classify(argv, is_session_leader, has_tty, environ) -> Role::{Client,Server}` — two process roles told apart by a tty | **Fails.** One Electron process tree, one comm, no tty anywhere. There is no client/server split to classify, so `discover.rs`'s premise ("amon shows a session only while it has an attached client") has nothing to bind to. |
| `socket_for(argv, environ)` → a JSON-lines control socket | **Fails.** The only socket is the Cowork helper's, and it is (a) length-prefixed JSON, not JSON-lines, (b) the *VM's* socket, not the app's, (c) reachable only when Cowork has started a VM, and (d) a spawn/kill/writeStdin control plane it would be wrong to join. |
| `attach(socket, panes) -> (Vec<HostedAgent>, EventStream)` — snapshot plus pushed events | **Fails.** There is no snapshot call and no event stream. The nearest equivalents (`list_sessions`, `session_info`, `read_transcript`, `start_task`, `send_message`, `list_projects`) are **in-process SDK MCP tools** built with `createSdkMcpServer` and served to the model over an in-memory transport under server names `cowork-local` / `cowork-dispatch-child` / `cowork-scheduled` / `scheduled-tasks` — never bound to a socket. Enumeration for amon is a directory scan, which is a poll, not a stream. |
| `HostedAgent { identity, pane, agent, status, cwd, life }` | **Partial.** `identity` = `sessionId` (`local_<uuid>`, stable, ✓). `cwd` ✓ from the record. `agent` ✓ (title / kind). **`pane` has no analogue** — one window, no panes, so the second focus hop is a deep link, not an `agent.focus` on a pane. **`status` is absent from every readable source** (§2). **`life`** — no per-life counter; `createdAt` is the closest and does not move on restart. |

`HostedEvent::{Status, Departed, Structural}` likewise has no producer: a
directory watch can only synthesise `Structural` (a record appeared or
vanished), never `Status`.

The one part that *does* transfer is the tail of the herdr shape: window by
ancestry (`amon-hypr::resolve` on the Electron main pid, or the compositor
class `com.anthropic.Claude`), the "no window, no rows" gate, and focus as
two hops through `crates/amon-cli/src/focus.rs::go_to` — compositor focus,
then `xdg-open claude://code/continue?session=<id>`, carried on a new
`AgentEntry::claude_desktop: Option<ClaudeDesktopInfo { user_data_dir,
session_id, kind }>` beside `herdr`, additive and skip-serialized.

### The projection that would work

Not `Hosted`. A **file-plus-process** module, with hooks as the state channel:

1. **Discovery.** Watch `~/.config/Claude/{claude-code,local-agent-mode}-sessions/*/*/`
   for `local_*.json` (inotify, or the 2 s rescan the herdr watcher already
   runs). Parse with the vendor's own gate (`sessionId` string, `createdAt`
   and `lastActivityAt` numbers), skip `isArchived === true`. Row id
   `claude-desktop:<userData digest>:<sessionId>`, the digest keeping
   profiles apart the way the socket digest keeps herdr sessions apart.
2. **Window gate.** Find the Electron main process (`/usr/lib/claude-desktop/claude-desktop`,
   the one whose parent is not itself) → `amon-hypr::resolve` → follow
   `movewindow`/`closewindow` with the existing `WindowWatch`. No window, no
   rows — same rule as herdr.
3. **Liveness.** A session is Running iff a `claude` process
   (`~/.config/Claude/claude-code/<ver>/claude`) descended from that Electron
   main has the session's cwd — or, better, iff the hook in (4) said so.
   Otherwise Idle. This is the only signal that needs no cooperation.
4. **State, the honest channel: extend the existing Claude Code hook.**
   amon already owns `~/.claude/settings.json`
   (`crates/amon-integration/.../assets/claude/amon-agent-state.sh`), the
   desktop loads user settings (§2), and Claude Code hooks carry
   `session_id` and `cwd`. Relaxing the script's `AMON_ENV` / `AMON_AGENT_ID`
   guards into a second, wrapper-less mode — report `session_id` + `cwd` +
   event to `AMON_SOCKET_PATH` discovered from the well-known daemon socket
   rather than the environment — gives Working/Blocked/Idle for desktop
   sessions from the same events that already work in the terminal, and the
   `cliSessionId` in the record joins the hook's `session_id` to the row.
   This is a change to amon's own hook, needs no cooperation from Anthropic,
   and is the part worth prototyping first.
5. **Seen: no source, again.** Same conclusion as the Codex note. Either
   never claim Done for desktop rows, or treat "the `com.anthropic.Claude`
   window received compositor focus" as the seen signal (Hyprland
   `activewindow`, which `amon-hypr::follow` does not parse yet). One window
   for all sessions makes this coarser here than it would have been for
   Codex; leaning on `claude://code/needs-input` as the jump for Blocked rows
   is probably better than pretending to know Done.
6. **Overlap with the wrapper.** A terminal `amon claude` is screen-detected
   and writes into `~/.claude/projects`; a desktop session may write into the
   same tree (unknown, experiment 3). The dedupe rule has to be *by
   `cliSessionId`*, with the desktop module yielding to the wrapper whenever
   a wrapped row already claims that id — not by cwd, which will collide.

### Decision

**Not parked, but not `Hosted` either — and gated on one experiment.**

Unlike Codex desktop, nothing here is broken or waiting on an upstream fix:
the enumeration surface is stable, documented-by-artifact, and read by the
vendor's own shipped code. What is missing is *state*, and the route to state
does not run through the desktop app at all — it runs through Claude Code's
user-level hooks, which amon already installs and which the desktop's
sessions load.

So the recommendation is: **do not build a Claude Desktop module. Build the
wrapper-less hook mode** (step 4), which pays off for bare `claude` in a
terminal too, and let desktop rows fall out of it, joined to
`local_*.json` for title/cwd/kind and to the Electron window for the focus
hop. The residual risk is narrower than "do hooks work": that user settings
are loaded is verified in the spawn options themselves (§2), and no hook kill
switch exists anywhere in the package, so the only way this fails is if the
CCD spawn points `CLAUDE_CONFIG_DIR` somewhere other than `~/.claude` — which
would leave amon's hook sitting in a file those sessions never read.
Experiment 3 settles that from `/proc` in a single run. If it falls that way
the fix is to follow the app's configured root instead of assuming
`~/.claude`; only if the root turns out to be *per-session* does this collapse
to existence-plus-liveness rows with no Working/Blocked, and then the honest
call is to park it — a bar row that cannot go Blocked is not worth a row.

## What does not work, and was considered

- **Joining the Cowork helper socket.** Same-uid clients are accepted, but
  it is the VM's control plane (`spawn`, `kill`, `writeStdin`), has no
  session vocabulary, and exists only while Cowork has a VM. Reading it would
  be indistinguishable from driving it. No.
- **CDP / `--remote-debugging-port`.** Hard `process.exit(1)` unless
  Anthropic signs a 5-minute token. Not a gap to work around; a decision.
- **Calling the GNOME search provider over D-Bus.** It would answer, but it
  is a shell integration that self-exits after 120 s idle, returns at most 5
  results, and gates on `os-entry-points.json`. Read the same files it reads
  instead — that is what it does.
- **Tailing the JSONL transcripts for state.** Possible and precise, but it
  is polling private files whose schema Anthropic changes freely, and it
  duplicates what hooks report at the source. Same verdict the Codex note
  reached about `state_5.sqlite`.
- **Screen detection.** `crates/amon-detect/src/vendor/detect/manifests/claude.toml`
  is entirely TUI shape — `osc_title_working`, prompt-box regexes,
  "do you want to proceed?". The desktop is not a terminal; none of it
  applies. Same as Codex desktop.
- **Remote Control as a channel.** It is a relay to claude.ai, opt-in per
  session, and reports to the account, not to the box. Orthogonal.

## Next experiments

Each of these needs Claude Desktop installed and run once, which this
research deliberately did not do.

1. **Does the app leave a readable session tree?** Install
   `claude-desktop` (AUR, 1.40609.0-1), sign in, start one Claude Code
   session, then, from a *different* process:
   `ls ~/.config/Claude/{claude-code,local-agent-mode}-sessions/*/*/local_*.json`
   and `jq . ` one of them. Confirms the layout and pins the real field set
   (the code only proves which fields are *read*). Also
   `cat ~/.config/Claude/os-entry-points.json`.
2. **Is state really absent?** With a session mid-turn and again sitting on a
   permission prompt, diff the record file. If `isRunning` or a status field
   ever lands on disk, the whole design gets simpler. Expected: only
   `lastActivityAt` moves.
3. **The one that decides the verdict: which config root do desktop sessions
   use?** That user settings are loaded at all is settled in the spawn options
   (§2), and no hook kill switch exists, so what is left is only the path. Put
   a marker
   hook in `~/.claude/settings.json` (`SessionStart` → `date >> /tmp/marker`),
   start a Claude Code session *from the desktop app*, and check the marker.
   In the same run: `ps -ef --forest | grep claude-desktop` to confirm a
   `claude` child, and `tr '\0' '\n' < /proc/<claude pid>/environ | grep CLAUDE_CONFIG_DIR`
   to settle whether it is overridden. Then check whether
   `~/.claude/projects/` gained a transcript or whether it landed under the
   session storage dir.
4. **Where the Cowork line falls.** Start a Cowork task; `ps` for a host
   `claude` child and `ss -xp | grep claude-cowork-vm.sock` for the helper's
   connections. Establishes whether the process signal covers Cowork at all,
   and how many clients the helper holds.
5. **The focus hop.** With the app running,
   `xdg-open "claude://code/continue?session=local_<uuid>&source=amon"` and
   `xdg-open "claude://code/needs-input"`. Confirms the second hop works from
   an unrelated process and that `needs-input` picks the blocked session —
   the two things a Claude Desktop row would rely on.
6. **The window, on Hyprland.** `hyprctl -j clients` for class /
   `initialClass` / `xwayland`. The package ships no launcher script, so the
   app almost certainly starts under XWayland with WM_CLASS
   `com.anthropic.Claude`; `amon-hypr` should be pointed at the pid, not the
   class, either way. **Unknown** until observed.
7. **Speak the helper socket read-only** (optional, only if the projection
   above ever needs it): a ~40-line client that connects to
   `$XDG_RUNTIME_DIR/claude-cowork-vm.sock`, writes
   `4-byte BE length || {"method":"isRunning","id":1}`, and reads the reply.
   Confirms the framing and that a non-app same-uid client is accepted.
   Do not call anything but `isRunning` / `isGuestConnected`.

## Side effects of this research on this machine

Kept for honesty; nothing in the repo changed besides this file.

- `claude-desktop_1.40609.0_amd64.deb` (166 MB) was downloaded from
  Anthropic's apt pool into `~/.cache/claude-research/claude-desktop/` and
  extracted there (`x/data`, 558 MB; `asar/`, the unpacked `app.asar`).
  Nothing was installed, no service started, no file outside that cache
  directory and this document was written or modified.
- The AUR `PKGBUILD`s for `claude-desktop` and `claude-desktop-extra` and the
  repository's `Packages.gz` were fetched into the same directory.
- Two helper scripts (an `asar` extractor and a context-grep) live in this
  session's scratchpad, not in the repo.
- The cache directory can be deleted with
  `rm -rf ~/.cache/claude-research/claude-desktop`.

## Sources

Primary, in order of weight:

- **The package itself**, sha256
  `a96e96ff8eb4d4d7ffa785aba7fc23f8684b12ac83ed2ef4060f0f09f4177a98`, from
  `https://downloads.claude.ai/claude-desktop/apt/stable/pool/main/c/claude-desktop/claude-desktop_1.40609.0_amd64.deb`
  — `control`, `postinst`, `postrm`;
  `usr/share/applications/com.anthropic.Claude.desktop`;
  `usr/lib/claude-desktop/version`;
  `usr/lib/claude-desktop/resources/gnome-search-provider/{searchProvider.js,
  com.anthropic.Claude.search-provider.ini,
  com.anthropic.Claude.SearchProvider.service}` (unminified, commented);
  `usr/lib/claude-desktop/resources/cowork-linux-helper` (Go, stripped —
  read via `strings`: `github.com/mdlayher/vsock` v1.2.1,
  `github.com/mdlayher/socket` v0.4.1, `main.startVirtiofsd`,
  `[server] listening on %s`, `peer uid %d != our uid %d`,
  `vhost-user-fs-pci,chardev=virtiofs0,tag=claudeshared`,
  `vhost-vsock-pci,guest-cid=%d`, `qmp_capabilities`);
  `usr/lib/claude-desktop/resources/app.asar`, unpacked and hand-deminified —
  `package.json`, `.vite/build/index.pre.js` (switch blocklist, CDP-auth
  verifier, userData entry list), `.vite/build/index.chunk-Ci77nY41.js`
  (Cowork VM client and framing, QEMU/firmware/virtiofsd probing, session
  storage paths `oDt`/`eD`/`EPr`, the import scanner `[claude-ai-import:local-scan]`
  `oPr`/`bPr`/`yPr`/`_Pr`, `UPr` persisted projection, `[CCD]` binary manager,
  `Iyr()` OS-entry/deep-link table, `nz()` = `$CLAUDE_CONFIG_DIR ?? ~/.claude`,
  `ttr()` status_category folding, `vm_bundles/claudevm.bundle`),
  `.vite/build/index.chunk-Clv3sLGS.js` (bundled Claude Code CLI settings
  schema: `bwrapPath`, `socatPath`, `sandbox_deps_missing`; agent-SDK
  `pathToClaudeCodeExecutable`), `.vite/build/index2.chunk-lz5Un0_t.js` (CCD
  session spawn, `settingSources: ['user','project','local']`),
  `.vite/build/index2.chunk-CGh0k4sB.js` (Cowork host loop,
  `settingSources: ['user']`, `CLAUDE_CONFIG_DIR` override),
  `.vite/build/index.chunk-ISElr9gv.js` (`claude-ssh` deployment,
  `--serve --socket`, `https://downloads.claude.ai/claude-ssh-releases`).
- **Anthropic's apt repository metadata**:
  `https://downloads.claude.ai/claude-desktop/apt/stable/dists/stable/Release`
  and `.../main/binary-amd64/Packages.gz` (version history, `Depends` /
  `Recommends` — no `socat`, no hard QEMU).
- **Anthropic docs**:
  https://code.claude.com/docs/en/desktop-linux (Linux beta scope, apt repo,
  key fingerprint, Cowork's KVM / `vhost_vsock` / `virtiofsd` requirements,
  what is not in the beta);
  https://code.claude.com/docs/en/sandboxing (bubblewrap + socat as the Linux
  sandbox dependencies — the socat answer);
  https://code.claude.com/docs/en/remote-control (Remote Control is a relay
  to claude.ai / mobile; the desktop's "Enable remote control by default"
  toggle);
  https://support.claude.com/en/articles/10065433-install-claude-desktop
  (apt install steps, Cowork prerequisites, `usermod -aG kvm`, disk/RAM);
  https://claude.com/download (Linux beta; Chat + Cowork + Claude Code at
  launch; Computer Use and Dictation not available).
- **AUR packaging** (packager's work, not upstream's manifest, but it
  documents the Arch-specific shims):
  `https://aur.archlinux.org/cgit/aur.git/plain/PKGBUILD?h=claude-desktop`
  (repackages the same `.deb`; promotes `qemu-system-x86`/`edk2-ovmf`/
  `virtiofsd` to hard deps, adds `socat`, symlinks
  `/usr/bin/virtiofsd` and the Debian OVMF names, notes `vhost_vsock`
  auto-loads via its devname alias);
  `https://aur.archlinux.org/cgit/aur.git/plain/PKGBUILD?h=claude-desktop-extra`
  (third-party patched fork; its `socat: Faster Quick Entry toggle via socket`
  optdep is that fork's own launcher patch and does **not** describe upstream).
- **amon**: `crates/amon-daemon/src/runtime/mod.rs` (the `Hosted` contract,
  `HostedAgent`, `HostedEvent`), `crates/amon-daemon/src/runtime/discover.rs`
  (attached-client rule), `crates/amon-cli/src/focus.rs` (the focus hops),
  `crates/amon-detect/src/vendor/detect/manifests/claude.toml` (TUI-only
  detection), `crates/amon-integration/src/vendor/integration/assets/claude/amon-agent-state.sh`
  (the `AMON_ENV` / `AMON_SOCKET_PATH` / `AMON_AGENT_ID` guards),
  `docs/research/herdr-live-integration.md` and
  `docs/research/codex-desktop-integration.md` (the shape this note is
  measured against).

Secondary (used for orientation only; no technical claim above rests on
them):

- https://github.com/anthropics/claude-code/issues/40347 — "Official Claude
  Desktop Linux support — Cowork, Dispatch, and remote session integration".
- https://github.com/anthropics/claude-code/issues/86105 — Linux desktop
  leaves an orphaned process on window close (relevant to a `/proc`-based
  liveness signal; **unverified**).
- https://github.com/anthropics/claude-code/issues/32345, #82431, #48299,
  #76307 — other Linux desktop bug reports.

Not verified, listed so the gap is explicit: the Linux beta's exact
announcement date. The brief gives 2026-06-30; the package's own `postinst`
comment says the apt repo has served a signed `InRelease` "since the 2026-07
launch", and the oldest published package is 1.17180.0 with no date in the
index. Nothing in this note depends on the date.
