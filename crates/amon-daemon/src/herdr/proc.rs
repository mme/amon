//! Finding herdr on this machine by reading /proc.
//!
//! herdr never rewrites its process title and `--session` survives verbatim
//! in kernel-visible argv, which is what makes classification by argv safe.
//! The server is a session leader with no controlling tty; an attached
//! client is the `herdr` the user typed, tty and all. A session is shown
//! exactly while it has an attached client (a spec decision — a headless
//! server's agents have no window to jump to), and of several clients the
//! lowest pid wins: "first, or something" was the whole requirement.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    Client { name: Option<String> },
    Server { name: Option<String> },
    Other,
}

/// One herdr session with an attached client — the only kind amon shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrSession {
    pub name: Option<String>,
    /// The session's JSON API socket.
    pub socket: PathBuf,
    /// The attached client whose window the session's agents borrow.
    pub client_pid: u32,
}

/// herdr's top-level options that take a value, so what follows them is the
/// value and not a subcommand.
const VALUE_OPTIONS: &[&str] = &["--session", "--remote", "--remote-keybindings"];

/// Top-level flags that make herdr print something and exit rather than
/// attach to anything.
const PRINT_AND_EXIT: &[&str] = &["--default-config", "--skill", "--version", "-V"];

/// What one `herdr` process is, judged from its kernel-visible facts.
///
/// The server is recognized by argv alone — its own `setsid` races this
/// probe, so the session-leader test would misjudge a server that has not
/// daemonized yet. Clients need their tty: a `herdr` without one is a
/// detached leftover, not an attached view.
pub fn classify(argv: &[String], is_session_leader: bool, has_tty: bool, environ: &str) -> Role {
    let args: Vec<&str> = argv.iter().skip(1).map(String::as_str).collect();
    let positional = positionals(&args);
    if positional.first() == Some(&"server") {
        let _ = is_session_leader;
        return Role::Server {
            name: environ_session(environ),
        };
    }
    if !has_tty {
        return Role::Other;
    }
    // `--remote` is a wrapper around a child `herdr client` that owns the
    // tty; the child is the client, and counting both would invent a session
    // at a socket the parent never speaks to.
    if args
        .iter()
        .any(|arg| *arg == "--remote" || arg.starts_with("--remote="))
    {
        return Role::Other;
    }
    if args.iter().any(|arg| PRINT_AND_EXIT.contains(arg)) {
        return Role::Other;
    }
    // Flags come before the subcommand, and there may be no subcommand at
    // all — `herdr`, `herdr --handoff`, `herdr --session work` are all the
    // plain client. Reading only the first argument would miss every one
    // that opted into a flag, and miss the whole session with it.
    let is_client = match positional.split_first() {
        None => true,
        Some((&"client", _)) => true,
        Some((&"session", rest)) | Some((&"agent", rest)) | Some((&"terminal", rest)) => {
            rest.first() == Some(&"attach")
        }
        Some(_) => false,
    };
    if is_client {
        Role::Client {
            name: argv_session(&args).or_else(|| environ_session(environ)),
        }
    } else {
        Role::Other
    }
}

/// The arguments that are not flags, nor the values flags consume.
fn positionals<'a>(args: &[&'a str]) -> Vec<&'a str> {
    let mut found = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg.starts_with('-') {
            // `--opt=value` carries its own value; `--opt value` eats the
            // next argument, which is otherwise mistaken for a subcommand.
            if !arg.contains('=') && VALUE_OPTIONS.contains(arg) {
                iter.next();
            }
            continue;
        }
        found.push(*arg);
    }
    found
}

fn argv_session(args: &[&str]) -> Option<String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if *arg == "--session" {
            return iter.next().map(|name| (*name).to_owned());
        }
        if let Some(name) = arg.strip_prefix("--session=") {
            return Some(name.to_owned());
        }
    }
    let positional = positionals(args);
    if positional.first() == Some(&"session") && positional.get(1) == Some(&"attach") {
        return positional.get(2).map(|name| (*name).to_owned());
    }
    None
}

/// One variable out of a NUL-separated environ blob.
pub fn env_value<'a>(environ: &'a str, key: &str) -> Option<&'a str> {
    environ
        .split('\0')
        .filter_map(|pair| pair.split_once('='))
        .find(|(name, _)| *name == key)
        .map(|(_, value)| value)
        .filter(|value| !value.is_empty())
}

/// `HERDR_SESSION` out of an environ blob. Usually absent on clients — herdr
/// applies `--session` with setenv after exec, which relocates the
/// environment off the stack `/proc` reads — so argv is the primary source
/// and this the fallback for a shell-exported session.
fn environ_session(environ: &str) -> Option<String> {
    env_value(environ, "HERDR_SESSION").map(str::to_owned)
}

/// The socket a client with this argv and environment is talking to.
///
/// herdr's own resolution order, applied to *that process's* environment
/// rather than the daemon's: an explicit `--session`, then
/// `HERDR_SOCKET_PATH`, then `HERDR_SESSION`, then the default session. A
/// herdr started with a config home of its own is a session amon would
/// otherwise knock at the wrong door for, forever.
pub fn socket_for(argv: &[String], environ: &str) -> Option<PathBuf> {
    let args: Vec<&str> = argv.iter().skip(1).map(String::as_str).collect();
    if let Some(name) = argv_session(&args) {
        return Some(session_socket_in(&config_dir(environ), Some(&name)));
    }
    // An override this daemon cannot resolve makes the client unknowable,
    // not ordinary. A relative path is herdr's to resolve against the
    // client's working directory, and it would travel on to the panel and
    // the CLI as an `HerdrInfo::socket` each resolves against its own —
    // three processes, three different sockets. Falling back to the default
    // would be worse than saying nothing: that socket may well exist, and
    // then another session's agents are shown in this client's window, and
    // focus takes you to the wrong one.
    if let Some(path) = env_value(environ, "HERDR_SOCKET_PATH") {
        let path = Path::new(path);
        return path.is_absolute().then(|| path.to_path_buf());
    }
    Some(session_socket_in(
        &config_dir(environ),
        environ_session(environ).as_deref(),
    ))
}

/// herdr's config directory for a process with this environment, falling
/// back to the daemon's own when the environ could not be read.
fn config_dir(environ: &str) -> PathBuf {
    let base = env_value(environ, "XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env_value(environ, "HOME").map(|home| PathBuf::from(home).join(".config")))
        .or_else(|| {
            std::env::var_os("XDG_CONFIG_HOME")
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from)
        })
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_default();
    base.join("herdr")
}

/// The session's JSON API socket under a given config directory.
fn session_socket_in(config: &Path, name: Option<&str>) -> PathBuf {
    match name {
        None => config.join("herdr.sock"),
        Some(name) => config.join("sessions").join(name).join("herdr.sock"),
    }
}

/// Records a client against its socket, keeping the lowest pid.
///
/// Sessions are told apart by socket, not by name: two nameless clients
/// pointed at different sockets are two sessions, and two clients on one
/// socket are one session with a shared view — of which amon follows the
/// first, which is all the tie-breaking a shared view deserves.
fn claim(found: &mut Vec<(PathBuf, u32)>, socket: PathBuf, pid: u32) {
    match found.iter_mut().find(|(known, _)| *known == socket) {
        Some((_, client)) => *client = (*client).min(pid),
        None => found.push((socket, pid)),
    }
}

/// `(session id, tty_nr)` from a `/proc/<pid>/stat` line — fields 6 and 7,
/// counted after the last `)` because comm is arbitrary bytes.
pub fn stat_fields(stat: &str) -> Option<(u32, u32)> {
    let after_comm = &stat[stat.rfind(')')? + 1..];
    let mut fields = after_comm.split_whitespace();
    let session = fields.nth(3)?.parse().ok()?;
    let tty_nr = fields.next()?.parse().ok()?;
    Some((session, tty_nr))
}

/// Every herdr session that has an attached client right now.
pub fn sessions() -> Vec<HerdrSession> {
    use std::os::unix::fs::MetadataExt;

    let mut found: Vec<(PathBuf, u32)> = Vec::new();
    let mut names: HashMap<PathBuf, Option<String>> = HashMap::new();
    let user = unsafe { libc::geteuid() };
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let path = entry.path();
        // Somebody else's herdr is none of this daemon's business — amon is
        // per-user. It matters beyond tidiness: `comm`, `cmdline` and `stat`
        // are world-readable while `environ` is not, so another user's client
        // would be classified from an empty environment, fall back to *this*
        // user's default socket, and claim the session with a pid whose
        // window lives in another login entirely.
        let Ok(owner) = std::fs::metadata(&path) else {
            continue;
        };
        if owner.uid() != user {
            continue;
        }
        // comm first: cheap, and filters out everything that is not herdr
        // before argv and environ are read at all.
        let Ok(comm) = std::fs::read_to_string(path.join("comm")) else {
            continue;
        };
        if comm.trim_end() != "herdr" {
            continue;
        }
        let Ok(cmdline) = std::fs::read(path.join("cmdline")) else {
            continue;
        };
        let argv: Vec<String> = cmdline
            .split(|byte| *byte == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf8_lossy(part).into_owned())
            .collect();
        let Ok(stat) = std::fs::read_to_string(path.join("stat")) else {
            continue;
        };
        let Some((session_id, tty_nr)) = stat_fields(&stat) else {
            continue;
        };
        let environ = std::fs::read(path.join("environ")).unwrap_or_default();
        let environ = String::from_utf8_lossy(&environ).into_owned();
        if let Role::Client { name } = classify(&argv, session_id == pid, tty_nr != 0, &environ) {
            let Some(socket) = socket_for(&argv, &environ) else {
                continue;
            };
            names.entry(socket.clone()).or_insert(name);
            claim(&mut found, socket, pid);
        }
    }
    found
        .into_iter()
        .map(|(socket, client_pid)| HerdrSession {
            name: names.get(&socket).cloned().unwrap_or_default(),
            socket,
            client_pid,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_bare_herdr_with_a_tty_is_a_client_of_the_default_session() {
        assert_eq!(
            classify(&argv(&["/usr/bin/herdr"]), false, true, ""),
            Role::Client { name: None }
        );
    }

    #[test]
    fn the_session_flag_names_the_session_in_both_spellings() {
        assert_eq!(
            classify(&argv(&["herdr", "--session", "work"]), false, true, ""),
            Role::Client {
                name: Some("work".into())
            }
        );
        assert_eq!(
            classify(&argv(&["herdr", "--session=work"]), false, true, ""),
            Role::Client {
                name: Some("work".into())
            }
        );
        assert_eq!(
            classify(
                &argv(&["herdr", "session", "attach", "work"]),
                false,
                true,
                ""
            ),
            Role::Client {
                name: Some("work".into())
            }
        );
    }

    #[test]
    fn attach_forms_and_the_explicit_client_are_clients_too() {
        for form in [
            vec!["herdr", "client"],
            vec!["herdr", "agent", "attach", "reviewer"],
            vec!["herdr", "terminal", "attach", "term_1"],
            vec!["herdr", "--no-session"],
        ] {
            assert!(
                matches!(classify(&argv(&form), false, true, ""), Role::Client { .. }),
                "{form:?} should be a client"
            );
        }
    }

    #[test]
    fn the_server_is_a_server_even_though_it_has_no_tty() {
        assert_eq!(
            classify(
                &argv(&["/home/me/.local/bin/herdr", "server"]),
                true,
                false,
                "HERDR_SESSION=work\0HOME=/home/me\0",
            ),
            Role::Server {
                name: Some("work".into())
            }
        );
    }

    #[test]
    fn flags_may_come_before_the_session_and_before_nothing_at_all() {
        // herdr's top-level flags precede any subcommand, and a client is
        // often *only* flags. Reading the first argument alone would leave
        // every session started with one invisible.
        for form in [
            vec!["herdr", "--handoff"],
            vec!["herdr", "--handoff", "--session", "work"],
            vec!["herdr", "--session=work", "--handoff"],
            // The value of a value-taking option is not a subcommand.
            vec!["herdr", "--remote-keybindings", "local"],
        ] {
            assert!(
                matches!(classify(&argv(&form), false, true, ""), Role::Client { .. }),
                "{form:?} should be a client"
            );
        }
        assert_eq!(
            classify(
                &argv(&["herdr", "--handoff", "--session", "work"]),
                false,
                true,
                ""
            ),
            Role::Client {
                name: Some("work".into())
            }
        );
        // A subcommand behind flags is still a subcommand.
        assert_eq!(
            classify(&argv(&["herdr", "--handoff", "status"]), false, true, ""),
            Role::Other
        );
        assert!(matches!(
            classify(&argv(&["herdr", "--handoff", "server"]), true, false, ""),
            Role::Server { .. }
        ));
    }

    #[test]
    fn what_is_neither_is_other() {
        // No tty: a client that lost its terminal is not attached.
        assert_eq!(classify(&argv(&["herdr"]), false, false, ""), Role::Other);
        // The remote wrapper's tty owner is its `herdr client` child.
        assert_eq!(
            classify(&argv(&["herdr", "--remote", "box"]), false, true, ""),
            Role::Other
        );
        // Every other subcommand is a one-shot CLI call.
        assert_eq!(
            classify(&argv(&["herdr", "agent", "list"]), false, true, ""),
            Role::Other
        );
        assert_eq!(
            classify(&argv(&["herdr", "status"]), false, true, ""),
            Role::Other
        );
        // Print-and-exit flags attach to nothing.
        assert_eq!(
            classify(&argv(&["herdr", "--skill"]), false, true, ""),
            Role::Other
        );
    }

    #[test]
    fn environ_names_the_session_when_argv_does_not() {
        assert_eq!(
            classify(&argv(&["herdr"]), false, true, "HERDR_SESSION=work\0"),
            Role::Client {
                name: Some("work".into())
            }
        );
    }

    #[test]
    fn the_stat_line_yields_sid_and_tty_past_a_hostile_comm() {
        // comm may contain spaces and parens; fields count from after the
        // last ')'. Layout: pid (comm) state ppid pgrp session tty_nr …
        let stat = "17 (h (x) dr) S 1 17 17 34816 1234";
        assert_eq!(stat_fields(stat), Some((17, 34816)));
        assert_eq!(stat_fields("garbage"), None);
    }

    #[test]
    fn the_socket_follows_herdrs_own_resolution_order() {
        // Resolved against the *client's* environment, not the daemon's: a
        // herdr started with its own XDG_CONFIG_HOME or HERDR_SOCKET_PATH
        // keeps its socket wherever the daemon happens to have been started
        // from, and a watcher reading its own environment would knock on a
        // door that is not there.
        let home = "HOME=/h\0";

        // 1. an explicit --session wins over everything
        assert_eq!(
            socket_for(
                &argv(&["herdr", "--session", "work"]),
                "HERDR_SOCKET_PATH=/x/y.sock\0HERDR_SESSION=other\0HOME=/h\0"
            ),
            Some(PathBuf::from("/h/.config/herdr/sessions/work/herdr.sock"))
        );
        // 2. then HERDR_SOCKET_PATH, taken literally
        assert_eq!(
            socket_for(
                &argv(&["herdr"]),
                "HERDR_SOCKET_PATH=/x/y.sock\0HERDR_SESSION=other\0"
            ),
            Some(PathBuf::from("/x/y.sock"))
        );
        // 3. then HERDR_SESSION
        assert_eq!(
            socket_for(&argv(&["herdr"]), "HERDR_SESSION=work\0HOME=/h\0"),
            Some(PathBuf::from("/h/.config/herdr/sessions/work/herdr.sock"))
        );
        // 4. then the default session
        assert_eq!(
            socket_for(&argv(&["herdr"]), home),
            Some(PathBuf::from("/h/.config/herdr/herdr.sock"))
        );
        // The client's own XDG_CONFIG_HOME is what its socket hangs off.
        assert_eq!(
            socket_for(&argv(&["herdr"]), "XDG_CONFIG_HOME=/xdg\0HOME=/h\0"),
            Some(PathBuf::from("/xdg/herdr/herdr.sock"))
        );
    }

    #[test]
    fn a_client_whose_socket_cannot_be_resolved_is_left_alone() {
        // herdr resolves a relative override against the client's working
        // directory; the daemon, the panel and the CLI would each resolve it
        // against their own. Falling back to the default would be worse than
        // saying nothing — that socket may well exist, and then another
        // session's agents show up in this client's window and focus takes
        // you to the wrong one.
        assert_eq!(
            socket_for(&argv(&["herdr"]), "HERDR_SOCKET_PATH=herdr.sock\0HOME=/h\0"),
            None
        );
    }

    #[test]
    fn two_clients_on_one_socket_are_one_session_and_the_first_wins() {
        // Same socket, two attached clients: a shared view, so amon follows
        // one of them — the lowest pid, deterministically. Sessions are kept
        // apart by socket rather than by name, because two nameless clients
        // pointed at different sockets are two sessions, not one.
        let both = [
            ("HERDR_SOCKET_PATH=/a.sock\0", 40u32),
            ("HERDR_SOCKET_PATH=/a.sock\0", 12u32),
            ("HERDR_SOCKET_PATH=/b.sock\0", 30u32),
        ];
        let mut found: Vec<(PathBuf, u32)> = Vec::new();
        for (environ, pid) in both {
            claim(
                &mut found,
                socket_for(&argv(&["herdr"]), environ).expect("an absolute override resolves"),
                pid,
            );
        }
        found.sort();
        assert_eq!(
            found,
            vec![
                (PathBuf::from("/a.sock"), 12),
                (PathBuf::from("/b.sock"), 30)
            ]
        );
    }

    #[test]
    fn an_unreadable_environ_falls_back_to_the_daemons_own() {
        // /proc/<pid>/environ can be unreadable; the client is still a
        // client, and the overwhelmingly likely answer is that it shares the
        // daemon's config home. Env-mutating, which is why the suite runs
        // under nextest: one process per test (see justfile).
        std::env::remove_var("XDG_CONFIG_HOME");
        let home = std::env::var("HOME").unwrap();
        assert_eq!(
            socket_for(&argv(&["herdr"]), ""),
            Some(PathBuf::from(&home).join(".config/herdr/herdr.sock"))
        );
        assert_eq!(
            socket_for(&argv(&["herdr", "--session", "work"]), ""),
            Some(PathBuf::from(&home).join(".config/herdr/sessions/work/herdr.sock"))
        );

        std::env::set_var("XDG_CONFIG_HOME", "/xdg");
        assert_eq!(
            socket_for(&argv(&["herdr"]), ""),
            Some(PathBuf::from("/xdg/herdr/herdr.sock"))
        );
    }
}
