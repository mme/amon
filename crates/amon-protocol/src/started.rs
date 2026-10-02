//! The agents you started: what the start agent panel offers back (ADR-0026).
//!
//! One file, `started.toml` in amon's state directory, with an `[[agent]]`
//! table per agent and folder (and host, for one on another machine), the
//! most recent first and never more than [`CAP`]. TOML because people edit it
//! by hand, as they do the config; everything that reads it re-reads it, so
//! an edit applies the next time the panel opens.
//!
//! Writers are wrappers, and two agents can start in the same instant, so a
//! write takes an exclusive lock, reads, changes and replaces the file whole.
//! A file that will not parse is treated as empty rather than as an error: a
//! hand edit gone wrong costs the history, never an agent its start.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use toml::value::{Date, Datetime, Offset, Time};

/// How many entries the file keeps, and the panel shows.
pub const CAP: usize = 25;

/// One agent you started, in one place.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Started {
    /// The command it was started with, bare: `claude`, never its arguments.
    pub agent: String,
    /// The folder, absolute - on the host's own file system for a remote one.
    pub dir: String,
    /// The repository's name, when the folder is inside one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Where in the repository, when not at its root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subpath: Option<String>,
    /// The branch the folder was on when it was last started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// For an agent on another machine: its host name, as that machine
    /// reports it. Absent for one on this machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// For an agent on another machine: the arguments the ssh session was
    /// opened with, which is how it is reached again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ssh: Vec<String>,
    /// When it was last started, to the second.
    pub last_started: Datetime,
}

impl Started {
    /// What makes two entries the same row: agent, folder, and host.
    pub fn key(&self) -> (&str, &str, Option<&str>) {
        (&self.agent, &self.dir, self.host.as_deref())
    }

    /// `last_started` as unix seconds, or 0 when it is not a full timestamp -
    /// a hand-written date with no time sorts last rather than failing.
    pub fn last_started_secs(&self) -> i64 {
        unix_from_datetime(&self.last_started).unwrap_or(0)
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    agent: Vec<Started>,
}

/// `started.toml` in amon's state directory: `$XDG_STATE_HOME/amon`, else
/// `~/.local/state/amon` - the same directory the detection state lives in.
pub fn path() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state")
        });
    base.join(crate::paths::app_dir_name()).join("started.toml")
}

/// Every entry, most recent first. Empty when there is no file, or one that
/// does not parse.
pub fn load() -> Vec<Started> {
    load_from(&path())
}

pub fn load_from(path: &Path) -> Vec<Started> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut entries = toml::from_str::<File>(&text)
        .map(|file| file.agent)
        .unwrap_or_default();
    sort(&mut entries);
    entries
}

/// Records a start: the entry moves to the top, replacing the one with the
/// same key, and the file is cut back to [`CAP`].
pub fn record(entry: Started) -> io::Result<()> {
    record_at(&path(), entry)
}

pub fn record_at(path: &Path, entry: Started) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _lock = Lock::take(&path.with_extension("lock"))?;

    let mut entries = load_from(path);
    entries.retain(|existing| existing.key() != entry.key());
    entries.push(entry);
    sort(&mut entries);
    entries.truncate(CAP);

    let text = toml::to_string(&File { agent: entries }).map_err(io::Error::other)?;
    let partial = path.with_extension("toml.new");
    {
        let mut file = std::fs::File::create(&partial)?;
        file.write_all(HEADER.as_bytes())?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
    }
    std::fs::rename(&partial, path)
}

const HEADER: &str = "\
# The agents you started, most recent first: what the start agent panel
# (Super+Alt+A) offers. Written by amon each time an agent starts; edit or
# delete entries freely - the panel re-reads this file every time it opens.

";

fn sort(entries: &mut [Started]) {
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.last_started_secs()));
}

/// An exclusive `flock` held for as long as the value lives.
struct Lock(std::fs::File);

impl Lock {
    fn take(path: &Path) -> io::Result<Self> {
        use std::os::fd::AsRawFd;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)?;
        // SAFETY: a valid descriptor owned by `file`, which outlives the lock.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(file))
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        // SAFETY: as above; closing would release it anyway.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

/// Unix seconds as a TOML UTC datetime, which is what a person reading the
/// file sees: `2026-10-02T09:45:27Z`.
pub fn datetime_from_unix(secs: i64) -> Datetime {
    let days = secs.div_euclid(86_400);
    let rest = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    Datetime {
        date: Some(Date {
            year: year as u16,
            month: month as u8,
            day: day as u8,
        }),
        time: Some(Time {
            hour: (rest / 3600) as u8,
            minute: ((rest % 3600) / 60) as u8,
            second: (rest % 60) as u8,
            nanosecond: 0,
        }),
        offset: Some(Offset::Z),
    }
}

/// The reverse, honouring any offset; `None` without both a date and a time.
pub fn unix_from_datetime(datetime: &Datetime) -> Option<i64> {
    let date = datetime.date?;
    let time = datetime.time?;
    let days = days_from_civil(date.year as i64, date.month as i64, date.day as i64);
    let mut secs =
        days * 86_400 + time.hour as i64 * 3600 + time.minute as i64 * 60 + time.second as i64;
    if let Some(Offset::Custom { minutes }) = datetime.offset {
        secs -= minutes as i64 * 60;
    }
    Some(secs)
}

// Howard Hinnant's civil-from-days and days-from-civil, proleptic Gregorian.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year.rem_euclid(400);
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// A moment as the calendar shows it where the user is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalDay {
    pub year: i32,
    /// 0-based day of the year.
    pub yday: i32,
    /// 0-based month.
    pub month: u32,
    pub mday: u32,
}

/// The local calendar day of a unix time, from the system's time zone.
pub fn local_day(secs: i64) -> LocalDay {
    let time = secs as libc::time_t;
    // SAFETY: localtime_r writes into the zeroed struct we own.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&time, &mut tm) };
    LocalDay {
        year: tm.tm_year + 1900,
        yday: tm.tm_yday,
        month: tm.tm_mon as u32,
        mday: tm.tm_mday as u32,
    }
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// When something was started, said the way a person would: "just now",
/// "5 min ago", "2 h ago", "yesterday", then the date - "Sep 28", with the
/// year once it is not this one.
pub fn when(then: i64, now: i64) -> String {
    when_with(then, now, local_day)
}

pub fn when_with(then: i64, now: i64, day_of: impl Fn(i64) -> LocalDay) -> String {
    let ago = (now - then).max(0);
    if ago < 60 {
        return "just now".into();
    }
    if ago < 3600 {
        return format!("{} min ago", ago / 60);
    }
    let (start, today) = (day_of(then), day_of(now));
    if start.year == today.year && start.yday == today.yday {
        return format!("{} h ago", ago / 3600);
    }
    if day_of(now - 86_400).yday == start.yday && day_of(now - 86_400).year == start.year {
        return "yesterday".into();
    }
    let month = MONTHS[start.month.min(11) as usize];
    if start.year == today.year {
        format!("{month} {}", start.mday)
    } else {
        format!("{month} {} {}", start.mday, start.year)
    }
}

/// A folder as the panel writes it: the home folder as `~`. For a remote
/// entry the remote home is not known here, so a path under `/home/<user>` or
/// `/Users/<user>` - where Linux and macOS put it - is taken for it.
pub fn display_dir(dir: &str, remote: bool, local_home: &str) -> String {
    if !remote {
        if !local_home.is_empty() {
            if let Some(rest) = dir.strip_prefix(local_home) {
                if rest.is_empty() || rest.starts_with('/') {
                    return format!("~{rest}");
                }
            }
        }
        return dir.to_string();
    }
    for base in ["/home/", "/Users/"] {
        if let Some(after) = dir.strip_prefix(base) {
            let rest = after.find('/').map(|cut| &after[cut..]).unwrap_or("");
            if !after.is_empty() {
                return format!("~{rest}");
            }
        }
    }
    dir.to_string()
}

/// The part of an ssh command line that says how to connect: its options and
/// the destination, without a remote command after it - the start panel adds
/// its own - and without `-t`/`-T`, which it sets itself. Options that take a
/// value keep it, whether written apart (`-p 2222`) or joined (`-p2222`).
pub fn ssh_connection_args(args: &[String]) -> Vec<String> {
    const TAKES_VALUE: &str = "BbcDEeFIiJLlmOoPpQRSWw";
    let mut kept = Vec::new();
    let mut words = args.iter();
    while let Some(word) = words.next() {
        if word == "--" {
            if let Some(destination) = words.next() {
                kept.push(destination.clone());
            }
            break;
        }
        if let Some(flags) = word.strip_prefix('-').filter(|flags| !flags.is_empty()) {
            if flags.chars().all(|flag| flag == 't' || flag == 'T') {
                continue;
            }
            kept.push(word.clone());
            // `-p 2222`: a value-taking option as the last letter, value apart.
            let last = flags.chars().last().unwrap_or(' ');
            let joined =
                flags.len() > 1 && TAKES_VALUE.contains(flags.chars().next().unwrap_or(' '));
            if TAKES_VALUE.contains(last) && !joined {
                if let Some(value) = words.next() {
                    kept.push(value.clone());
                }
            }
            continue;
        }
        // The first word that is not an option is the destination; everything
        // after it is the remote command.
        kept.push(word.clone());
        break;
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started(agent: &str, dir: &str, host: Option<&str>, secs: i64) -> Started {
        Started {
            agent: agent.into(),
            dir: dir.into(),
            project: None,
            subpath: None,
            branch: None,
            host: host.map(str::to_owned),
            ssh: Vec::new(),
            last_started: datetime_from_unix(secs),
        }
    }

    #[test]
    fn a_start_moves_its_row_to_the_top_and_replaces_the_old_one() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("started.toml");
        record_at(&file, started("claude", "/w/a", None, 100)).unwrap();
        record_at(&file, started("codex", "/w/b", None, 200)).unwrap();
        record_at(&file, started("claude", "/w/a", None, 300)).unwrap();

        let entries = load_from(&file);
        let keys: Vec<_> = entries
            .iter()
            .map(|e| (e.agent.as_str(), e.last_started_secs()))
            .collect();
        assert_eq!(
            keys,
            [("claude", 300), ("codex", 200)],
            "one row per agent and folder"
        );
    }

    #[test]
    fn the_same_folder_on_another_host_is_another_row() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("started.toml");
        record_at(&file, started("claude", "/w/a", None, 100)).unwrap();
        record_at(&file, started("claude", "/w/a", Some("mac"), 200)).unwrap();
        assert_eq!(load_from(&file).len(), 2);
    }

    #[test]
    fn the_file_keeps_the_most_recent_twenty_five() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("started.toml");
        for i in 0..30 {
            record_at(&file, started("claude", &format!("/w/{i}"), None, i)).unwrap();
        }
        let entries = load_from(&file);
        assert_eq!(entries.len(), CAP);
        assert_eq!(entries[0].dir, "/w/29");
        assert_eq!(entries[CAP - 1].dir, "/w/5");
    }

    #[test]
    fn the_file_is_readable_toml_a_person_can_edit() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("started.toml");
        let mut entry = started(
            "claude",
            "/home/me/src/amon",
            Some("macbookpro"),
            1_790_850_000,
        );
        entry.project = Some("amon".into());
        entry.branch = Some("main".into());
        entry.ssh = vec!["mme@macbookpro".into()];
        record_at(&file, entry).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("[[agent]]"), "{text}");
        assert!(text.contains("agent = \"claude\""), "{text}");
        assert!(text.contains("branch = \"main\""), "{text}");
        assert!(
            text.contains("last_started = 2026-10-01T10:20:00Z"),
            "{text}"
        );
        assert!(text.contains("ssh = [\"mme@macbookpro\"]"), "{text}");
    }

    #[test]
    fn a_broken_file_is_an_empty_history_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("started.toml");
        std::fs::write(&file, "this is [not toml").unwrap();
        assert!(load_from(&file).is_empty());
        record_at(&file, started("claude", "/w/a", None, 1)).unwrap();
        assert_eq!(
            load_from(&file).len(),
            1,
            "and the next start writes a good one"
        );
    }

    #[test]
    fn datetimes_round_trip_through_unix_seconds() {
        for secs in [0, 86_399, 951_782_400, 1_790_850_000, 4_102_444_800] {
            assert_eq!(
                unix_from_datetime(&datetime_from_unix(secs)),
                Some(secs),
                "{secs}"
            );
        }
        // A hand-written offset is honoured.
        let parsed: Datetime = "2026-10-02T11:45:27+02:00".parse().unwrap();
        assert_eq!(
            unix_from_datetime(&parsed),
            unix_from_datetime(&"2026-10-02T09:45:27Z".parse().unwrap())
        );
    }

    fn utc_day(secs: i64) -> LocalDay {
        let days = secs.div_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        let yday = days - days_from_civil(year, 1, 1);
        LocalDay {
            year: year as i32,
            yday: yday as i32,
            month: (month - 1) as u32,
            mday: day as u32,
        }
    }

    #[test]
    fn when_reads_like_a_person_would_say_it() {
        // 2026-10-02 12:00:00 UTC
        let now = unix_from_datetime(&"2026-10-02T12:00:00Z".parse().unwrap()).unwrap();
        let at = |s: &str| unix_from_datetime(&s.parse().unwrap()).unwrap();
        let say = |then: i64| when_with(then, now, utc_day);
        assert_eq!(say(now - 20), "just now");
        assert_eq!(say(now - 5 * 60), "5 min ago");
        assert_eq!(say(now - 2 * 3600), "2 h ago");
        assert_eq!(say(at("2026-10-01T23:30:00Z")), "yesterday");
        assert_eq!(say(at("2026-09-28T10:00:00Z")), "Sep 28");
        assert_eq!(say(at("2025-12-31T10:00:00Z")), "Dec 31 2025");
    }

    #[test]
    fn folders_are_written_with_the_home_as_a_tilde() {
        assert_eq!(
            display_dir("/home/me/src/amon", false, "/home/me"),
            "~/src/amon"
        );
        assert_eq!(display_dir("/home/me", false, "/home/me"), "~");
        assert_eq!(
            display_dir("/home/meow/x", false, "/home/me"),
            "/home/meow/x"
        );
        assert_eq!(display_dir("/opt/x", false, "/home/me"), "/opt/x");
        // Remote: the usual homes on Linux and macOS.
        assert_eq!(
            display_dir("/Users/mme/src/amon", true, "/home/me"),
            "~/src/amon"
        );
        assert_eq!(display_dir("/home/mme", true, "/home/me"), "~");
        assert_eq!(display_dir("/srv/app", true, "/home/me"), "/srv/app");
    }

    #[test]
    fn only_how_to_connect_is_kept_of_an_ssh_command_line() {
        let args = |line: &str| line.split(' ').map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(ssh_connection_args(&args("mac")), args("mac"));
        assert_eq!(
            ssh_connection_args(&args(
                "-t -o BatchMode=yes mme@mac cd x && exec zsh -ic claude"
            )),
            args("-o BatchMode=yes mme@mac")
        );
        assert_eq!(
            ssh_connection_args(&args("-p 2222 -A host ls")),
            args("-p 2222 -A host")
        );
        assert_eq!(
            ssh_connection_args(&args("-p2222 -tt host")),
            args("-p2222 host")
        );
        assert_eq!(
            ssh_connection_args(&args("-J jump -- host uptime")),
            args("-J jump host")
        );
    }
}
