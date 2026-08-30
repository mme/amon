//! Telling luvus's processes apart, and where their socket is.
//!
//! luvus's process model matches herdr's: the process a user types is the
//! tty-owning TUI client, `luvus server` is spawned detached under setsid,
//! and argv survives verbatim.

use std::path::PathBuf;

use crate::runtime::discover::env_value;
use crate::runtime::Role;

/// luvus's top-level options that take a value, so what follows them is the
/// value and not a subcommand.
const VALUE_OPTIONS: &[&str] = &["--session", "--remote"];

/// Top-level flags that make luvus print something and exit rather than
/// attach to anything.
const PRINT_AND_EXIT: &[&str] = &["--version", "-V", "--help", "-h"];

/// luvus's threshold for aliasing a socket path: macOS allows 103 bytes of
/// `sun_path`, and luvus applies the rule on every platform.
const ALIAS_AT: usize = 100;

pub fn classify(argv: &[String], _is_session_leader: bool, has_tty: bool, environ: &str) -> Role {
    let args: Vec<&str> = argv.iter().skip(1).map(String::as_str).collect();
    let positional = positionals(&args);
    if positional.first() == Some(&"server") {
        return Role::Server {
            name: environ_session(environ),
        };
    }
    if !has_tty {
        return Role::Other;
    }
    // `--remote` is a wrapper around an ssh that runs the bridge on the far
    // side; nothing local owns a session through it.
    if args
        .iter()
        .any(|arg| *arg == "--remote" || arg.starts_with("--remote="))
    {
        return Role::Other;
    }
    if args.iter().any(|arg| PRINT_AND_EXIT.contains(arg)) {
        return Role::Other;
    }
    // `luvus`, `luvus --session X` and `luvus --local` attach the TUI
    // in-process; so do `attach <id>`, `client`, and `session attach X`.
    // Every other subcommand is the CLI: one request and out.
    let is_client = match positional.split_first() {
        None => true,
        Some((&"client", _)) | Some((&"attach", _)) => true,
        Some((&"session", rest)) => rest.first() == Some(&"attach"),
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
            if !arg.contains('=') && VALUE_OPTIONS.contains(arg) {
                iter.next();
            }
            continue;
        }
        found.push(*arg);
    }
    found
}

/// luvus's name for the unnamed session. Asking for it by name is the same
/// as not asking — luvus normalizes it away before anything is resolved,
/// and so must this, or the socket is looked for under `sessions/default/`.
const DEFAULT_SESSION: &str = "default";

fn named(name: &str) -> Option<String> {
    (name != DEFAULT_SESSION).then(|| name.to_owned())
}

fn argv_session(args: &[&str]) -> Option<String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if *arg == "--session" {
            return iter.next().and_then(|name| named(name));
        }
        if let Some(name) = arg.strip_prefix("--session=") {
            return named(name);
        }
    }
    let positional = positionals(args);
    if positional.first() == Some(&"session") && positional.get(1) == Some(&"attach") {
        return positional.get(2).and_then(|name| named(name));
    }
    None
}

/// `LUVUS_SESSION` out of an environ blob: what a client resolved its
/// `--session` into before spawning the server, so the server carries it
/// even though its argv does not.
fn environ_session(environ: &str) -> Option<String> {
    env_value(environ, "LUVUS_SESSION").and_then(named)
}

/// The socket a client with this argv and environment is talking to —
/// resolved against *that process's* environment, not the daemon's.
pub fn socket_for(argv: &[String], environ: &str) -> Option<PathBuf> {
    let args: Vec<&str> = argv.iter().skip(1).map(String::as_str).collect();
    let name = argv_session(&args).or_else(|| environ_session(environ));
    let home = luvus_home(environ)?;
    let logical = match name.as_deref() {
        None => home.join("luvus.sock"),
        Some(name) => home.join("sessions").join(name).join("luvus.sock"),
    };
    Some(alias(logical))
}

/// `$LUVUS_HOME`, else `~/.luvus`, for the client's environment, falling
/// back to the daemon's own HOME when the environ could not be read. A debug
/// build of luvus uses `~/.luvus-dev`; amon only knows the release name.
fn luvus_home(environ: &str) -> Option<PathBuf> {
    if let Some(home) = env_value(environ, "LUVUS_HOME") {
        return Some(PathBuf::from(home));
    }
    env_value(environ, "HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .map(|home| home.join(".luvus"))
}

/// luvus keeps sockets under its home unless the path would not fit
/// `sun_path`, in which case it hashes the logical path into
/// `/tmp/luvus-<uid>/`. Same hash, same threshold, or a long home is a
/// session amon never finds.
fn alias(logical: PathBuf) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    let bytes = logical.as_os_str().as_bytes();
    if bytes.len() < ALIAS_AT {
        return logical;
    }
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let uid = unsafe { libc::geteuid() };
    PathBuf::from(format!("/tmp/luvus-{uid}/{hash:016x}-api.sock"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_bare_luvus_with_a_tty_is_a_client_of_the_default_session() {
        assert_eq!(
            classify(&argv(&["/usr/bin/luvus"]), true, true, ""),
            Role::Client { name: None }
        );
    }

    #[test]
    fn the_session_flag_names_the_session_in_every_spelling() {
        for form in [
            vec!["luvus", "--session", "work"],
            vec!["luvus", "--session=work"],
            vec!["luvus", "session", "attach", "work"],
        ] {
            assert_eq!(
                classify(&argv(&form), true, true, ""),
                Role::Client {
                    name: Some("work".into())
                },
                "{form:?}"
            );
        }
    }

    #[test]
    fn attach_client_and_local_are_clients_too() {
        for form in [
            vec!["luvus", "attach", "7"],
            vec!["luvus", "client"],
            vec!["luvus", "--local"],
        ] {
            assert!(
                matches!(classify(&argv(&form), true, true, ""), Role::Client { .. }),
                "{form:?} should be a client"
            );
        }
    }

    #[test]
    fn the_server_is_a_server_and_names_its_session_from_environ() {
        assert_eq!(
            classify(
                &argv(&["luvus", "server"]),
                true,
                false,
                "LUVUS_SESSION=work\0HOME=/home/me\0"
            ),
            Role::Server {
                name: Some("work".into())
            }
        );
    }

    #[test]
    fn remote_forms_and_cli_subcommands_are_neither() {
        for form in [
            vec!["luvus", "--remote", "box"],
            vec!["luvus", "remote-client-bridge"],
            vec!["luvus", "agent", "list"],
            vec!["luvus", "--version"],
            vec!["luvus", "doctor"],
            vec!["luvus", "uhp", "schema"],
        ] {
            assert_eq!(
                classify(&argv(&form), true, true, ""),
                Role::Other,
                "{form:?}"
            );
        }
        // No tty: a client that lost its terminal is not attached.
        assert_eq!(classify(&argv(&["luvus"]), true, false, ""), Role::Other);
    }

    #[test]
    fn the_socket_follows_luvus_home_and_the_session() {
        assert_eq!(
            socket_for(&argv(&["luvus"]), "HOME=/home/me\0"),
            Some(PathBuf::from("/home/me/.luvus/luvus.sock"))
        );
        assert_eq!(
            socket_for(
                &argv(&["luvus", "--session", "work"]),
                "HOME=/home/me\0LUVUS_HOME=/x/luvus\0"
            ),
            Some(PathBuf::from("/x/luvus/sessions/work/luvus.sock"))
        );
        assert_eq!(
            socket_for(&argv(&["luvus"]), "HOME=/home/me\0LUVUS_SESSION=work\0"),
            Some(PathBuf::from("/home/me/.luvus/sessions/work/luvus.sock"))
        );
    }

    #[test]
    fn asking_for_the_default_session_by_name_is_the_unnamed_session() {
        for form in [
            vec!["luvus", "--session", "default"],
            vec!["luvus", "--session=default"],
            vec!["luvus", "session", "attach", "default"],
        ] {
            assert_eq!(
                classify(&argv(&form), true, true, ""),
                Role::Client { name: None },
                "{form:?}"
            );
            assert_eq!(
                socket_for(&argv(&form), "HOME=/home/me\0"),
                Some(PathBuf::from("/home/me/.luvus/luvus.sock")),
                "{form:?}"
            );
        }
        assert_eq!(
            classify(&argv(&["luvus"]), true, true, "LUVUS_SESSION=default\0"),
            Role::Client { name: None }
        );
        assert_eq!(
            socket_for(&argv(&["luvus"]), "HOME=/home/me\0LUVUS_SESSION=default\0"),
            Some(PathBuf::from("/home/me/.luvus/luvus.sock"))
        );
    }

    #[test]
    fn a_long_home_aliases_into_tmp_the_way_luvus_does() {
        let home = format!("/home/{}", "x".repeat(120));
        let socket = socket_for(&argv(&["luvus"]), &format!("HOME={home}\0")).unwrap();
        assert!(socket.starts_with("/tmp/"), "{socket:?}");
        assert!(socket.to_string_lossy().contains("/luvus-"));
        assert!(socket.to_string_lossy().ends_with("-api.sock"));
        assert!(socket.as_os_str().len() < ALIAS_AT);
    }

    #[test]
    fn an_unreadable_environ_falls_back_to_the_daemons_own_home() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(
            socket_for(&argv(&["luvus"]), ""),
            Some(PathBuf::from(&home).join(".luvus/luvus.sock"))
        );
    }
}
