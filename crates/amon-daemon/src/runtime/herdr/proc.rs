//! Telling herdr's processes apart, and where their socket is.
//!
//! herdr never rewrites its process title and `--session` survives verbatim
//! in kernel-visible argv, which is what makes classification by argv safe.
//! The server is a session leader with no controlling tty; an attached
//! client is the `herdr` the user typed, tty and all.

use std::path::{Path, PathBuf};

use crate::runtime::discover::env_value;
use crate::runtime::Role;

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
