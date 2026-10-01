# macOS is a headless target, Apple Silicon only

Amon builds and releases for `aarch64-apple-darwin` so a Mac can be the far
end of an ssh session — wrapper, daemon, and the full CLI (`status`, `setup`
for agent hooks and aliases, `doctor`, `remove`), with no desktop surface.
The desktop is Omarchy's: no bar, no panel, no sounds, no `amon focus`
(hidden from help and answered with a plain error, rather than removed —
clap would otherwise read `amon focus 3` as an agent named `focus`). A
remote Mac agent's sounds and indicators happen on the local Omarchy
desktop, via the mirrored entry, which is the point of having amon there at
all.

The port is small on purpose, because the crate boundary already was the
platform boundary: the wrapper is portable-pty and POSIX termios, the shadow
terminal is Ghostty's VT library (native Mac software), and every desktop
integration was already gated at runtime on what it finds — `omarchy` on
PATH, WirePlumber present, `HYPRLAND_INSTANCE_SIGNATURE` set — so on a Mac
each reports its own absence instead of needing a compile-time fork. The
compile-time differences that do exist are three: aliases go to `~/.zshrc`
(zsh has been the Mac default since Catalina; the alias syntax and
interactive-only expansion are the same as bash's), `amon focus` is Linux
gated, and the installer picks the `aarch64-darwin` tarball and verifies it
with `shasum`, the tool a Mac actually ships.

Apple Silicon only, deliberately: one Mac target to build, test, and
support, on hardware every remaining Mac is. An Intel Mac is turned away by
the installer with directions to build from source, which still works —
nothing in the code is arm-specific.

## Consequences

CI builds and tests on a macOS runner per pull request, and the release
attaches a second tarball from one, after the Linux release — the release
Omarchy installs from is never blocked by a Mac build. The release-contract
tests hold the darwin tarball's name between `publish.yml` and `install.sh`
the same way they hold the Linux one's.
