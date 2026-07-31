# Work Louder Creator Micro 2 — Linux support and key remapping

Research notes, 2026-07-31. Sources are primary where possible (worklouder.cc, the
QMK/Vial/VIA GitHub repos, Espressif's USB PID registry, Work Louder's own GitHub
org). Confirmed facts are stated plainly with a source; anything marked
**inference** is a conclusion drawn from those facts, not directly documented.

## TL;DR

The **original** Creator Micro (v1) is a QMK/VIA device (ATmega32U4, VID
`0x574C`). The **Creator Micro 2 is not QMK**: it is an ESP32‑S3 device with
proprietary firmware, configured through Work Louder's own Electron app
("Input"). It still types as a standard USB/BLE HID keyboard, so it works on
Linux out of the box for *using* it; for *remapping* it you either run the
community Linux port of Input (`github.com/worklouder/input-linux`, AppImage +
udev rules) or remap at the OS level with evdev/uinput tools (keyd, xremap,
input-remapper, kmonad). VIA/Vial do **not** apply to the Micro 2.

---

## 1. Firmware

### Creator Micro 2: proprietary, not QMK, not VIA, not Vial

- Work Louder's product page says configuration happens through their **Input**
  app ("Create Actions, Multi Action or simply remap your Creator Micro 2 with
  Input") and, for the Codex edition, through an integration in OpenAI Codex.
  No mention of QMK/VIA/Vial anywhere on the v2 product or setup pages.
  Sources: <https://worklouder.cc/creator-micro-2>,
  <https://worklouder.cc/micro-setup>.
- There is **no `micro_2` (or similar) keyboard in QMK**. `qmk_firmware`
  contains only `keyboards/work_louder/{loop,micro,nano,numpad,work_board}` —
  all v1-generation AVR boards.
  Source: <https://github.com/qmk/qmk_firmware/tree/master/keyboards/work_louder>.
- A question on Work Louder's own feedback board about the Creator Micro 2
  exposing a QMK-style raw HID interface notes that **"the v2 isn't in QMK
  mainline"**; no answer documenting a public HID API was found.
  Source: <https://feedback.worklouder.cc/> (feature-request post asking whether
  the Creator Micro 2 exposes a raw HID / usage page `0xFF60` interface).
- The Micro 2 enumerates under **Espressif's USB vendor ID `0x303A`** with PIDs
  registered by Work Louder in Espressif's official PID registry (see §6) —
  i.e. the MCU is an **ESP32-S3-class Espressif chip**. The community
  `input-linux` udev script explicitly mentions "Serial devices (ESP32-S3,
  debug interfaces, DFU firmware)" for these VIDs.
  Sources:
  <https://github.com/espressif/usb-pids/blob/master/allocated-pids.txt>,
  <https://raw.githubusercontent.com/worklouder/input-linux/main/patch/dist-electron/scripts/install-udev-worklouder.sh>.
- **Inference:** QMK cannot run on the Micro 2 as shipped — QMK's supported
  platforms are AVR/ARM/RP2040 and do not include ESP32
  (<https://docs.qmk.fm/compatible_microcontrollers>). The move to ESP32‑S3 is
  what enables BLE, and it is why Work Louder moved off QMK for the whole v2
  lineup (Nomad E v2, Knob, Creator Micro 2 all sit under `0x303A`, §6).
- Device firmware is versioned separately from the Input app (community
  reports reference device firmware v0.4.0 alongside Input 0.17.x); firmware
  updates are delivered through the Input app / Work Louder support flows.
  Source: <https://feedback.worklouder.cc/> (support threads referencing
  "firmware v0.4.0", "Input 0.17.1").

### Original Creator Micro (v1), for contrast

- QMK mainline: `keyboards/work_louder/micro` — `"keyboard_name": "Micro Pad"`,
  `"manufacturer": "Work Louder"`, **`"processor": "atmega32u4"`**,
  `"bootloader": "atmel-dfu"`, dual encoders, RGB matrix + underglow.
  Source: <https://github.com/qmk/qmk_firmware/blob/master/keyboards/work_louder/micro/keyboard.json>.
- **VIA:** the v1 boards got VIA v3 support in QMK PR #19555 ("[Keyboard] Work
  Louder updates for via v3", by maintainer drashna); the `via` keymaps now
  live in QMK's userspace-VIA repo under
  `keyboards/work_louder/{loop,micro,nano,numpad,work_board}`.
  Sources: <https://github.com/qmk/qmk_firmware/pull/19555>,
  <https://github.com/qmk/qmk_userspace_via/tree/main/keyboards/work_louder>.
  Work Louder's legacy setup page tells v1 owners to remap with **VIA at
  usevia.app ("Only available for Chrome")** and to flash factory firmware with
  QMK Toolbox. Source: <https://worklouder.cc/setup>.
  (A GitHub code search for `work_louder` in `the-via/keyboards` returned 0
  hits, so the VIA definitions appear to be distributed by Work Louder/QMK
  userspace rather than the VIA definitions repo; GitHub code search can be
  incomplete, treat as weak evidence.)
- **Vial:** `vial-kb/vial-qmk` contains the `work_louder` directories only
  because it periodically merges QMK master; they hold just the `default`
  keymap and **no `vial.json` / `vial` keymap**, so there is **no official
  Vial port** for any Work Louder board.
  Source: <https://github.com/vial-kb/vial-qmk/tree/master/keyboards/work_louder>
  (micro contains only `keymaps/default`).
- Community QMK forks for v1 exist, e.g.
  <https://github.com/ForsakenRei/qmk-worklouder-micro>.

## 2. Connectivity

Confirmed from <https://worklouder.cc/creator-micro-2>:

- **BASE model:** wired only, USB‑C. CNC polycarbonate frame.
- **PRO model:** **Bluetooth (BLE) + USB‑C**, 2100 mAh battery, CNC aluminium
  frame.
- BLE pairing (PRO): hold the touch button 3 s until the underglow turns blue,
  then tap to cycle **BLE channels 1/2/3**; a 4th tap returns to wired mode.
  Source: <https://worklouder.cc/micro-setup>.

What stack handles BLE:

- **Confirmed:** the Espressif registry lists a distinct PID for the BLE
  personality — `0x8298 "Work Louder - Creator Micro v2 - BLE"` vs `0x8297`
  for USB — plus a `0x8299 "Work Louder - USB Dongle v1"`. All on the ESP32
  vendor ID. Source:
  <https://github.com/espressif/usb-pids/blob/master/allocated-pids.txt>.
- **Inference:** BLE is handled by the same proprietary firmware on the
  ESP32‑S3 (which has native BLE 5). It is **not ZMK** and not a separate BT
  module bolted onto QMK — there is no QMK build for this hardware at all, and
  no ZMK shield/board definition for it exists (nothing Work Louder-related in
  ZMK's supported hardware, <https://zmk.dev/docs/hardware>).
- Over BLE it appears as a **HID-over-GATT (HOG) device** — the `input-linux`
  udev script explicitly matches "Bluetooth HID (HID-over-GATT) devices" with
  bus `0005` hidraw parents. Source:
  <https://raw.githubusercontent.com/worklouder/input-linux/main/patch/dist-electron/scripts/install-udev-worklouder.sh>.

## 3. Linux: does it work out of the box?

- **Typing/using it: yes (inference, high confidence).** It enumerates as a
  standard USB HID keyboard (wired) / BLE HID (HOG) device (PRO), so the
  kernel's `hid-generic` handles it with no driver. Work Louder's site lists
  only "Mac / Windows" compatibility (<https://worklouder.cc/creator-micro-2>),
  but that refers to their configurator app, not HID itself; the existence and
  wording of the community Linux port ("use your Work Louder device on Linux"
  with the *configurator*) confirms the gap is tooling, not HID.
  Source: <https://github.com/worklouder/input-linux>.
- **Configuring it: needs the Input app + udev rules.** Official Input builds
  are macOS/Windows; Linux is served by an **unofficial, Work
  Louder-acknowledged community port hosted in Work Louder's own GitHub org**:
  <https://github.com/worklouder/input-linux> — an Electron app using
  `node-hid`, distributed as an AppImage (requires FUSE / `libfuse2`), or
  buildable from source. The Input download page itself links a "Linux
  (unofficial)" build. Source: <https://worklouder.cc/input>.
- **Known Linux gotcha (documented in the udev script):** a **BLE hidraw node
  has no USB parent**, so udev rules matching `ATTRS{idVendor}` never match it
  — configuring over Bluetooth fails unless you also match the Bluetooth HID
  parent (`KERNELS=="0005:<VID>:*"`, bus 0005 = BUS_BLUETOOTH, uppercase hex).
  The script handles this. Source:
  <https://raw.githubusercontent.com/worklouder/input-linux/main/patch/dist-electron/scripts/install-udev-worklouder.sh>.

### The udev rules (from the official install script)

Installed to `/etc/udev/rules.d/99-worklouder.rules` by
`patch/dist-electron/scripts/install-udev-worklouder.sh` (the app can also
install them itself). Vendor IDs: `303a` (current, Espressif) and `574c`
(legacy QMK boards). Essentials:

```udev
# wired hidraw
KERNEL=="hidraw*", SUBSYSTEM=="hidraw", ATTRS{idVendor}=="303a", MODE="0666", GROUP="plugdev", TAG+="uaccess"
KERNEL=="hidraw*", SUBSYSTEM=="hidraw", ATTRS{idVendor}=="574c", MODE="0666", GROUP="plugdev", TAG+="uaccess"
# BLE hidraw (no USB parent — match the bluetooth HID parent kernel name)
KERNEL=="hidraw*", SUBSYSTEM=="hidraw", KERNELS=="0005:303A:*", MODE="0666", GROUP="plugdev", TAG+="uaccess"
KERNEL=="hidraw*", SUBSYSTEM=="hidraw", KERNELS=="0005:574C:*", MODE="0666", GROUP="plugdev", TAG+="uaccess"
# generic USB + serial (ESP32-S3 DFU/debug)
SUBSYSTEM=="usb", ATTR{idVendor}=="303a", MODE="0666", GROUP="plugdev"
SUBSYSTEM=="tty", ATTRS{idVendor}=="303a", MODE="0666", GROUP="plugdev", TAG+="uaccess"
```

(The script picks group `input` on Arch/Fedora-family and `plugdev` on
Debian-family.) Source:
<https://raw.githubusercontent.com/worklouder/input-linux/main/patch/dist-electron/scripts/install-udev-worklouder.sh>.

For **v1 (QMK/VIA) boards** the standard VIA/Vial guidance applies instead:
usevia.app in Chrome needs hidraw access, e.g. Vial's rule
`KERNEL=="hidraw*", SUBSYSTEM=="hidraw", ATTRS{serial}=="*vial:f64c2b3c*", MODE="0660", GROUP="users"`
(Vial boards only) or the broader
`KERNEL=="hidraw*", SUBSYSTEM=="hidraw", MODE="0660", GROUP="users", TAG+="uaccess"`
for VIA. Source: <https://get.vial.today/manual/linux-udev.html>.

## 4. Remapping options on Linux

For the **Creator Micro 2** specifically:

| Option | Applies? | Notes |
|---|---|---|
| Input (on-device) via `input-linux` | **Yes** | Community AppImage; remap, Actions/Multi-Action, multi-tap/hold; stored on device, works across hosts. <https://github.com/worklouder/input-linux>, <https://worklouder.cc/input> |
| VIA (usevia.app) | **No** | Not QMK/VIA firmware; no VIA definition exists for v2. (v1 Micro: yes, per <https://worklouder.cc/setup>) |
| Vial | **No** | No Vial port even for v1 (see §1). <https://github.com/vial-kb/vial-qmk/tree/master/keyboards/work_louder> |
| Flashing QMK/ZMK | **No** | ESP32-S3 is unsupported by QMK (<https://docs.qmk.fm/compatible_microcontrollers>); no ZMK board definition exists. |
| OS-level evdev/uinput remappers (keyd, kmonad, input-remapper, xremap) | **Yes (inference)** | The device presents as an ordinary HID keyboard, which is all these tools require; they can match it per-device by its VID/PID (`303a:8297`/`8298`) or name. Host-local only — remaps don't travel with the device and can't drive its RGB/BLE-channel features. |

**Inference / practical note:** the Micro 2's keys are freely assignable from
Input, so the usual pattern is to assign obscure keycodes (e.g. F13–F24) on the
device and bind them in the OS/WM — that works on Linux with zero extra
software. OS-level tools are the fallback when you can't or don't want to run
the Input port.

## 5. Extra controls and how they're exposed

Confirmed hardware (v2): **13 mechanical switches, 1 touch sensor, 1 rotary
encoder, 1 planar joystick, RGB underglow** (used for status "color cues",
e.g. blue = BLE mode). Source: <https://worklouder.cc/creator-micro-2>,
<https://worklouder.cc/micro-setup>.

- **How controls are reported:** not publicly documented for v2. **Inference:**
  since every control is user-assignable via Input and the device works on
  OSes without drivers, keys/dial/joystick emit standard HID keyboard and/or
  consumer-control usages as configured (on v1/QMK, encoders likewise sent
  whatever keycode was mapped — commonly consumer-page volume).
- **Raw/vendor HID API:** an open feature request on Work Louder's feedback
  board asks whether the Micro 2 exposes a raw HID interface (QMK-style usage
  page `0xFF60` or vendor-specific) that third-party apps could use, e.g. for
  per-key RGB from a host app; no documented public API was found, so
  **treat host-driven RGB/status control as unsupported today**. The Input app
  itself talks to the device over hidraw (`node-hid`), i.e. a vendor HID
  config protocol exists but is undocumented. Sources:
  <https://feedback.worklouder.cc/>, <https://github.com/worklouder/input-linux>.
- **Open-source Linux tooling for Work Louder devices:**
  - <https://github.com/worklouder/input-linux> — the community Linux port of
    Input (Electron + node-hid + udev script). The only Linux-specific project
    found.
  - <https://github.com/ForsakenRei/qmk-worklouder-micro> — custom QMK firmware
    for the v1 Micro (not v2).
  - No standalone open-source daemon/driver for the v2's vendor protocol was
    found.

## 6. USB VID/PID

**Creator Micro 2 (and v2-generation lineup)** — vendor ID `0x303A`
(Espressif); PIDs allocated to Work Louder in Espressif's official registry
(<https://github.com/espressif/usb-pids/blob/master/allocated-pids.txt>):

| PID | Device |
|---|---|
| `0x8297` | Work Louder – Creator Micro v2 |
| `0x8298` | Work Louder – Creator Micro v2 – BLE |
| `0x8299` | Work Louder – USB Dongle v1 |
| `0x8360` | Work Louder – Creator Micro v2 – Project 2077 (special edition) |
| `0x8294` / `0x8295` | Nomad [E] v1 – ANSI / ISO |
| `0x8370` / `0x8371` | Nomad [E] v2 – ISO / ANSI |
| `0x8296` / `0x82E3` | Knob v1 – ANSI / ISO |
| `0x8396` / `0x8397` | Knob F1 v1 – ANSI / ISO |
| `0x8346` | XYZ Work Board r2 |

**Original Creator Micro (v1, QMK)** — VID **`0x574C`** ("WL"), PID
**`0xE6E3`**, device version 1.0.0. Source:
<https://github.com/qmk/qmk_firmware/blob/master/keyboards/work_louder/micro/keyboard.json>.

## 7. The vendor protocol (from the shipped app bundle)

Follow-up findings from extracting the official app out of the `input-linux`
AppImage (`v0.18.0-rc8-Devbuild4`, `Input-0.18.0-rc.8`): the bundle contains
`node_modules/@worklouder/wl-device-kit` (v0.1.28, `UNLICENSED`, published to
Work Louder's private GitHub npm registry), and its `dist/*.js.map` source maps
embed the **complete original TypeScript sources** with doc comments. The
protocol is therefore fully knowable, though the SDK code itself is marked
proprietary/confidential — fine to learn the protocol from for interop, not
fine to vendor.

**Transport.** Vendor HID interface on usage page `0xFF00` (the keyboard's
regular keys are a separate standard HID interface). 64-byte reports framed as
`[0x06, channel, payload_len, payload…]` with a 61-byte payload cap; channels
are RPC and DEBUG, payloads are newline-delimited UTF-8 text reassembled
across reports. The same protocol also runs over USB CDC serial at 115200
baud. Source: `patch/dist-electron/main/index.js` in
<https://github.com/worklouder/input-linux> (`sendDataHID`, `parseHIDReport`).

**RPC layer.** JSON-RPC 2.0, request ids constrained to `[0, 999)` by
firmware, one request in flight at a time. Method vocabulary (from
`wl_rpc_api.ts`): `sys.version`, `sys.bootloader`, `sys.selftest`,
`device.status` (firmware, profile, layer, battery), `fs.list` / `fs.read` /
`fs.write` / `fs.readbin` / `fs.writebin` / `fs.delete` / `fs.rmdir` /
`fs.txbegin` / `fs.txcommit` (device flash filesystem — this is how configs
and wallpapers are stored), `mp.write_info` / `mp.write_artwork` (media
metadata + album art to the display), `appmgr.list_active` /
`appmgr.list_installed`, `ui.active_screen`, `ui.home_accent_color`,
**`lights.preview`**, and **`host.focused_app`** (host tells the device the
focused desktop app, for per-app profiles).

**Host-driven lighting is supported.** `lights.preview` applies a lighting
config immediately **without persisting to flash** (per the SDK doc comment) —
ideal for status signalling with no flash-wear concern. Params:
`{ backlight: {...}, underglow: {...} }` where each side is
`{ effect, brightness, speed, magic, color }`, color `#RRGGBB`, effect one of
`off | solid | snake | rainbow | breath | gradient`.

**Device→host notifications.** The device also *initiates* JSON-RPC toward
the host (id-less notifications on the same channel; the Input app runs
resident to serve them): `kb.sa.exec`, `kb.sa.inserttext`, `kb.sa.openapp`,
`kb.sa.openurl` (a "smart action" key asks the host to run a command / type
text / open an app or URL), `kb.cs.show`/`hide`/`toggle` (cheat sheet),
`kb.radial` (radial menu), `mp.fetch_data` (request media metadata),
`alert.generic`, `diag.report`. Consequence: plain-keycode mappings work with
no host software, but "smart action" keys are inert unless a listener is
running. Abbreviated JSON-RPC keys `m`/`p`/`i` are accepted alongside
`method`/`params`/`id`.

**Per-key RGB: not exposed.** The key LEDs are individually driven by
firmware effects (`snake`/`gradient` animate across the key "strip"), and
lighting is configurable per layer, but both the stored config and
`lights.preview` are zone-level only — one `{effect, brightness, speed,
magic, color}` for backlight and one for underglow. No per-key color concept
exists anywhere in the renderer bundle or SDK, matching the open
feedback-board request for host-driven per-key RGB (§5). Open lead: dump the
on-device config files via `fs.list`/`fs.read` to check whether the firmware's
stored format has per-key fields the app never sets.

**Device registry (SDK `device_registry.ts`):** Creator Micro V2 = `0x8297`
(USB) and `0x8298` (BLE), Codex Micro = `0x8360`, plus Nomad E v1/v2, Knob,
Knob F1, XYZ — one protocol across the whole v2 lineup.

## 8. Per-key "Agent Key" lighting — the Codex protocol

§7's "per-key RGB: not exposed" holds for the Input app's surface, but the
firmware does support per-key lighting for the six **Agent Keys** — via vendor
RPC methods that live only in a second private SDK,
`@worklouder/device-kit-oai` (v0.1.11), bundled inside the **official Codex
desktop app** (extracted from
<https://persistent.oaistatic.com/codex-app-prod/Codex.dmg>; the open-source
`openai/codex` CLI contains no device code — the integration is
desktop-app-only). Same 64-byte vendor-HID JSON-RPC transport as §7.

Host→device methods (`rpc_api_oai.js`):

- **`v.oai.thstatus`** — per-thread (= per Agent Key, slots `id` 0–5)
  lighting. Params: array of `{id, c, b, e, s, sk, sa}` = color (packed 24-bit
  RGB int), brightness (0–1), effect, speed (0–1), sync-keys / sync-ambient
  flags (0|1); only `id` required, omitted fields stay unchanged. Effects:
  `off=0, solid=1, snake=2, rainbow=3, breath=4, gradient=5, shallowBreath=6`.
- **`v.oai.rgbcfg`** — zone lighting: `{ambient: {e,b,s,m,c}, keys: {...}}`
  (ambient ring + keys backlight).

Device→host notifications: **`v.oai.hid`** `{k: "AG00"…"AG05", act, ag}` for
agent/command key presses, and **`v.oai.rad`** `{a, d}` (joystick angle /
distance, 0–1). Notifications are id-less; abbreviated `m`/`p`/`i` keys are
accepted alongside `method`/`params`/`id`.

Codex's status→color map: working `0x304FFE` (blue), unread `0x00FF4C`
(green), idle `0xFFFFFF`, awaiting-approval/response `0xFF6D00` (amber),
error `0xFF0033`, off `0`. Selected thread's key uses `breath` (speed 0.4),
others `solid`; keys-backlight zone is normally `off`, the ambient ring
mirrors the selected slot (snake while working). Battery via `device.status`
every 60 s; lighting writes deferred 100 ms after input; auto-off default
3 min.

### Exact per-state parameters (traced from `CodexMicroService`)

From `Codex.app` `.vite/build/service-4uQDVZZZ.js` + renderer
`codex-micro-slot-signals-*.js` + `@worklouder/device-kit-oai` (README
documents the full API):

**Effect codes:** `off=0, solid=1, snake=2, rainbow=3, breath=4, gradient=5,
shallowBreath=6` (shallowBreath breathes 0.5→1.0 brightness; unused by Codex's
service, only in the SDK).

**Status → color (packed RGB int):** `working` `0x304FFE`, `unread`
`0x00FF4C`, `idle` `0xFFFFFF`, `awaiting-approval` / `awaiting-response`
`0xFF6D00`, `error` `0xFF0033`, `off` `0`. Status derivation: no thread →
`off`; error → `error`; pending approval/response chip →
`awaiting-approval`/`awaiting-response`; loading/in-progress → `working`;
unread → `unread`; else `idle`. A selected thread's `unread` renders as
`idle` while the app window is focused.

**`v.oai.thstatus`** — always an array of 6 entries (slots 0–5), each:

- `off` slot: `{id, c:0, b:0, e:0, s:0, sk:0, sa:0}`
- otherwise: `{id, c:<status color>, b:<brightness>, e:<4 if selected||pulsing
  else 1>, s:<0.4 if selected||pulsing else 0>, sk:0, sa:0}`

i.e. unselected keys are `solid` at speed 0; the selected slot's key runs
`breath` at speed 0.4. Brightness = the `codex-micro-lighting-brightness`
setting (0–100, default 100) divided by 100 → default `1`. The sync flags are
always sent as `0`. Wire example:

```json
{"method":"v.oai.thstatus","params":[
  {"id":0,"c":3166206,"b":1,"e":4,"s":0.4,"sk":0,"sa":0},
  {"id":1,"c":16739584,"b":1,"e":1,"s":0,"sk":0,"sa":0},
  {"id":2,"c":0,"b":0,"e":0,"s":0,"sk":0,"sa":0},
  {"id":3,"c":0,"b":0,"e":0,"s":0,"sk":0,"sa":0},
  {"id":4,"c":0,"b":0,"e":0,"s":0,"sk":0,"sa":0},
  {"id":5,"c":0,"b":0,"e":0,"s":0,"sk":0,"sa":0}],"id":42}
```

(slot 0 = selected + working, slot 1 = awaiting approval, rest unassigned.)
Omitted fields leave that attribute unchanged on-device; Codex always sends
all seven.

**`v.oai.rgbcfg`** — sent *before* `thstatus` on every update, shape
`{keys: {e,b,s,m,c}, ambient: {e,b,s,m,c}}`:

- `keys` (under-key zone): normally all-zero (off — thstatus owns the keys);
  for 4 s after the selected slot changes it becomes
  `{e:1, b:<brightness>, s:0, m:0, c:<ambient color>}`.
- `ambient` (ring): mirrors the selected slot — `working` →
  `{e:2 (snake), b, s:0.4, m:0, c:0x304FFE}`, other statuses → solid at
  speed 0; no selection → off. Voice overrides: recording → snake
  `0x2E8B57`, processing → snake `0xFFFFFF`, completed → solid `0xFFFFFF`.

**Housekeeping (service constants):** 6 slots; selection lighting visible for
4000 ms; lighting writes deferred 100 ms after key/joystick input; battery
polled via `device.status` every 60 s; inactivity auto-off (default 3 min,
setting `codex-micro-lighting-auto-off`) re-sends the all-off configs;
redundant writes deduped by JSON comparison of the last-applied payload;
reconnect backoff 1/2/5/10 s; lighting-write retries at 250/1000/3000 ms.
On shutdown the service sends all-off `rgbcfg` + `thstatus` (brightness 0).

### The Codex/OAI layer

- The agent keys are **firmware-native vendor keycodes**, not lighting magic:
  the Codex default profile (embedded in the Input app bundle,
  `device_config_data-*.js`) maps the key grid to `KV_OAI_AG00`–`KV_OAI_AG05`
  (six agent keys), `KV_OAI_ACT06`–`KV_OAI_ACT12` (action keys),
  `KV_OAI_ENC_CC`/`_CW`/`_CLK` (encoder), and joystick `type: "VENDOR"`.
  Keys mapped to these emit **no HID keystroke**; they produce `v.oai.hid`
  notifications instead. Renderer action-key labels include FAST, APPR, REJ,
  SPLIT, MIC, CODEX, BUG, TERM, DIFF, GIT, BRCH, MRG, PR, YOLO, ….
- The layer is **persistent, stored in device flash** like any layer. On the
  Codex Micro it is the factory default profile; on a stock Creator Micro 2
  the Input app offers an **"Add a new Codex layer"** button, gated by
  `canCreateCodexLayer`: device type `CreatorMicroV2` **and firmware ≥
  0.6.0** (so stock Micro 2 firmware ships the OAI feature set). The Codex
  desktop app itself never writes or deletes layers — quitting Codex leaves
  the layer mapped; its keys are simply inert (events with no listener). On
  shutdown Codex sends all-off lighting and the device reverts to its stored
  lighting config.
- Layers are an ordered array in the on-flash profile; whoever writes the
  profile controls the order (an "amon layer" could be index 0 = boot
  default). There is **no set-active-layer RPC** — `device.status` only
  reports `selectedLayerIndex`; switching is physical only. Note the profile
  file has two writers (Input app rewrites the whole profile on save).
- Codex desktop discovery: macOS/Windows use a private IOKit addon
  (`hid-topology-watcher.node`); **Linux falls back to plain `node-hid`**
  enumeration (VID `0x303A`). USB-vs-BLE heuristic: `transport === "usb" ||
  release % 4 == 0` (BCD release parity encodes transport); preference order
  USB first, then PID `0x8360` > `0x8297` > `0x8298`. Reconnect backoff
  1/2/5/10 s. Slot→thread assignment sources: `pinned` / `recent` /
  `priority` / `custom` (settings `codex-micro-agent-source`,
  `codex-micro-custom-agent-assignments` keyed `AG00`–`AG05`).

Notes: the Codex service drives the **stock Creator Micro 2 PIDs
(`0x8297`/`0x8298`) as well as the Codex Micro (`0x8360`)**, so `v.oai.*`
should work on a regular Micro 2 with current firmware. Scope caveat: this is
per-agent-slot (6 keys), not arbitrary per-key RGB across all 13 keys.
Firmware-side handling is closed (inference beyond the shipped host code).

### Local research artifacts (not in this repo; re-derivable)

- `~/.cache/chatgpt-app/` — Codex.dmg + extracted asar (`codex-asar/`):
  `@worklouder/device-kit-oai` SDK (dist + **396-line README documenting the
  full API**), `CodexMicroService` (`.vite/build/service-4uQDVZZZ.js`),
  slot-signals renderer.
- `~/.cache/wl-input-app/` — Input AppImage + extracted asar (`asar-out/`),
  plus `wl-device-kit-src/` (**original TypeScript of wl-device-kit v0.1.28,
  recovered from shipped source maps**) and `codex_layers.js` (the Codex
  default layer JSON).
- `~/.cache/codex-research/` — clone of `openai/codex` (confirmed: zero
  device code). `~/.cache/wl-buddyh/` — community skill repo.
- All SDK/app code is proprietary (`UNLICENSED`, Work Louder Inc.) — usable
  as protocol reference for interop, **never vendor or copy into amon**.

## 9. amon integration — design notes (2026-07-31)

Decisions and open questions from the design discussion; not yet implemented.

- **Shape:** build into `amond` as an optional, best-effort module (own task,
  panics caught, config-gated) — amond already owns the agent registry,
  event stream, and the right lifecycle (daemon death ⇒ device reverts to
  stored lighting). Detection via udev netlink monitor on the `hidraw`
  subsystem filtered to VID `303a` (BLE reconnects surface identically);
  reconnect with backoff; handshake `sys.version` + `device.status`; repaint
  only on state transitions, dedupe identical payloads (Codex-style).
- **Lighting mapping (proposed):** working → blue solid, blocked → amber,
  idle → white, focused agent → breath 0.4 — mirror Codex's exact params
  (§8). Lighting needs no remap and no exclusive access; keys keep typing
  whatever they're mapped to.
- **Input direction options:** (1) evdev `EVIOCGRAB` of the factory mapping —
  works today, all-or-nothing, per-host; (2) remap keys to F13–F24
  on-device, listen non-exclusively; (3) **preferred:** OAI-layer route —
  keys emit `v.oai.hid {k:"AG00"…}` with agent index, invisible to the OS,
  listener-agnostic (nothing binds it to Codex).
- **Permissions:** one udev rule per machine (root once; ship in package /
  `amon setup` subcommand / EACCES hint). Same story for `/dev/input` if
  option 1/2 is used.
- **Gate (first hardware session):** dump factory FS (`fs.list`/`fs.read`),
  back up, then send one `v.oai.thstatus` frame on the factory profile — does
  per-key lighting render without an OAI layer? Then optionally let Input
  write the Codex layer and diff the FS before/after to learn the on-disk
  profile format (safer than reverse-engineering Input's serializer).
- **Resolved (fw 0.6.0-rc.13 release notes, 2026-07-31):** thstatus lighting
  renders **only while a Codex-enabled layer is active**; switching layers
  restores normal lighting. So amon must watch `device.status.layer_index`
  and fall back to `lights.preview` zone lighting when the agent layer is
  not active. The 0.6.0 gate is confirmed by the notes themselves ("adds
  firmware support for the upcoming OpenAI Codex integration … Lighting can
  also display live Codex activity and status"; 0.5.0 added only Cheat
  Sheet/Smart Actions) and by Input's `canCreateCodexLayer` constant
  `V0_6_0 = "0.6.0"`. The notes also say the Codex host-side integration for
  stock Micro 2 arrives "with a future Codex host update".
- **Still unverified:** whether firmware identifies the Codex layer by
  position, keycodes, or a flag.

## 10. Hardware verification (2026-07-31, device fw v0.4.0)

Live results against the real device (serial `1020BA734D08`, wired,
enumerates as `303a:8298` — the "BLE" PID even in wired mode, with
`bcdDevice 0.b0`; the Codex heuristic `release % 4 == 0` classifies it USB):

- **One HID interface only** — keyboard/mouse/consumer *and* the vendor
  usage share a single hidraw node; the leading `0x06` of each frame is the
  HID report ID. Smoke test in Python (raw `os.write`/`os.read` of 64-byte
  frames on `/dev/hidraw*`) worked exactly per §7 framing.
- **Confirmed working on fw v0.4.0:** `sys.version` →
  `{"version":"v0.4.0"}`; `device.status` → `{version, profile_index,
  layer_index, battery, is_charging}` (snake_case on the wire!);
  `lights.preview` (§7 zone params) → applies immediately;
  `fs.list` → `[{"name":"keymap.json","size":1714}]`;
  `fs.read {"file": name}` → `{"data": "<json string>"}` (param is `file`,
  not `name`; response is stringified JSON in `data`).
- **`v.oai.thstatus` → `{"error":{"code":404,"message":"Method not
  found"}}` on v0.4.0.** Consistent with the Input app's `canCreateCodexLayer`
  gate: the OAI feature set ships in **firmware 0.6.0**, which as of
  2026-07-31 exists only as pre-releases (`v0.6.0-rc.13`, released
  2026-07-31) in the public repo **`worklouder/cm-v2-fw-releases`** (`.bin`
  assets; sibling repos: `knob-fw-releases`, `nomad-e-v2-fw-releases`, …;
  the Input app filters `rc` releases unless beta is enabled).
- **On-disk config format confirmed** — `keymap.json` is the whole config:
  `{version: 1, activeProfileId, profiles: [{id, name, layers: [...],
  macrosUsed, multiActionsUsed}], multiActions, macros, macrosGroups,
  multiActionsGroups, linkedApps}`. Factory profile has **3 layers**
  (Layer 1: keys `KC_A`–`KC_M`, encoder `KC_VOLU/KC_VOLD/KC_MPLY`, joystick
  `type:"RADIAL"` with 8 sectors `{k, a1, a2}` (angles 0–1); Layers 2–3 all
  `KC_NONE`, joystick `type:"JOYSTICK"`). Each layer may carry `lights:
  {backlight, underglow}` with **effect as a string** (`"solid"`,
  `"rainbow"`) — note the RPC lighting params use numeric effects, the file
  uses names. Device was on `layer_index: 1` (the empty Layer 2) during the
  test. Backup of the full factory config:
  `~/.cache/wl-input-app/device_keymap_backup_20260731.json`.
- **Conclusion:** everything amon needs is proven except `v.oai.*`, which
  requires a firmware update to 0.6.0 (currently RC-only). Fallback available
  today on 0.4.0: zone lighting via `lights.preview`.

## Context

- The **OpenAI Codex Micro** (launched July 2026) is a rebranded Creator
  Micro 2 built with Work Louder; its setup page lives on worklouder.cc.
  Sources: <https://worklouder.cc/openai-micro-setup>,
  <https://www.techtimes.com/articles/319389/20260630/openai-codex-micro-launches-july-15-macro-pad-built-work-louder.htm>,
  <https://news.ycombinator.com/item?id=48923550>.
- Work Louder feedback/support board (firmware versions, Linux/BLE issue
  reports, raw-HID request): <https://feedback.worklouder.cc/>.
