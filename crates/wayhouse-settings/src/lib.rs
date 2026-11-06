//! Process flags from a settings file (G1, #477).
//!
//! Every program (proxy, controller, aggregator, UI) can be started with `--settings-dir DIR`.
//! It then reads `DIR/<program>.json` and passes the values it holds as flags the command line
//! did not give, so the command line always wins (the programs read no environment variables for
//! their flags). The UI writes these files; the flag list it offers comes from each program's own
//! clap definition ([`schema`]) and a new value is checked by running that same parser
//! ([`check`]), so there is no second list to keep in step.
//!
//! **Trial applies.** Changing a flag restarts the program through its normal restart policy
//! (systemd `Restart=on-failure`, a compose `restart:` policy). A change is never final until
//! someone confirms it: the file keeps the previous values and a `pending` mark, and unless the
//! mark is cleared within [`CONFIRM_SECS`] of the start the program puts the previous values
//! back and restarts again ([`Guard`]). A value that stops the program from starting at all is
//! undone as well, at the next start after [`MAX_ATTEMPTS`] tries or [`HARD_DEADLINE_SECS`]
//! ([`layer`]). No supervisor process is involved: the file and the restart policy are enough.
//!
//! Token, password and PSK flags are never read from or written to the file (they are the
//! bootstrap of the thing that edits them); flags that run once and exit are left out too.

use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::{ArgAction, Command};
use serde::{Deserialize, Serialize};

/// How long a freshly applied change may run unconfirmed before it is undone.
pub const CONFIRM_SECS: u64 = 60;
/// Starts a pending change gets before it is undone (a crash loop after an apply).
pub const MAX_ATTEMPTS: u32 = 3;
/// A pending change older than this is undone at the next start, whatever the attempts.
pub const HARD_DEADLINE_SECS: u64 = 300;
/// The exit status a program leaves with to be restarted (`EX_TEMPFAIL`).
pub const RESTART_EXIT: i32 = 75;

/// Flags that cannot be set from the file: the file's own location, and flags that act once.
const NOT_EDITABLE: &[&str] = &[
    "settings-dir",
    "help",
    "version",
    "check",
    "hash-password",
    "secret-set",
    "secret-delete",
    "secret-list",
];

/// Flags that make a run something other than a start.
const DRY_FLAGS: &[&str] = &[
    "--check",
    "--help",
    "-h",
    "--version",
    "-V",
    "--hash-password",
    "--secret-set",
    "--secret-delete",
    "--secret-list",
];

/// One flag's value in the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Bool(bool),
    One(String),
    Many(Vec<String>),
}

pub type Values = BTreeMap<String, Value>;

/// A change that is not confirmed yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    /// Unix seconds when it was applied.
    pub applied_at: u64,
    /// Starts since then.
    #[serde(default)]
    pub attempts: u32,
}

/// `DIR/<program>.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct File {
    /// Counts every write, so a writer can tell that the file moved under it.
    #[serde(default)]
    pub rev: u64,
    #[serde(default)]
    pub values: Values,
    /// What to go back to; present only while `pending` is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<Values>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<Pending>,
    /// Why the last change was refused by the program itself (it could not run with those
    /// values, so they were put back without a restart). Cleared by the next write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub fn path(dir: &Path, program: &str) -> PathBuf {
    dir.join(format!("{program}.json"))
}

/// The file, or `None` when there is none yet.
pub fn read(dir: &Path, program: &str) -> Result<Option<File>, String> {
    let p = path(dir, program);
    let text = match std::fs::read_to_string(&p) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", p.display())),
    };
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|e| format!("{}: {e}", p.display()))
}

/// Writes the file whole: a temporary file in the same directory (its name is this process's
/// own, so the UI backend and the program cannot tear each other's write), synced, then renamed
/// over the old one, owner-only. The writer becomes the owner, so the UI backend and the programs
/// must run as the same user.
pub fn write(dir: &Path, program: &str, file: &File) -> Result<(), String> {
    let body = serde_json::to_vec_pretty(file).map_err(|e| e.to_string())?;
    write_atomic(dir, &format!("{program}.json"), &body)
}

fn write_atomic(dir: &Path, name: &str, body: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let p = dir.join(name);
    let tmp = dir.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let result = opts
        .open(&tmp)
        .and_then(|mut f| f.write_all(body).and_then(|()| f.sync_all()))
        .and_then(|()| std::fs::rename(&tmp, &p));
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("{}: {e}", p.display()));
    }
    Ok(())
}

/// `DIR/<program>.schema.json`: the flags the running program offers, written by the program at
/// each start so the UI backend (which does not link the other programs) can show and check them.
pub fn schema_path(dir: &Path, program: &str) -> PathBuf {
    dir.join(format!("{program}.schema.json"))
}

#[derive(Serialize, Deserialize)]
struct SchemaFile {
    flags: Vec<Flag>,
}

/// The flags the program last wrote, or `None` when it never started with this directory.
pub fn read_schema(dir: &Path, program: &str) -> Option<Vec<Flag>> {
    let text = std::fs::read_to_string(schema_path(dir, program)).ok()?;
    serde_json::from_str::<SchemaFile>(&text)
        .ok()
        .map(|f| f.flags)
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

// ---- schema -------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Bool,
    Text,
    List,
}

/// One flag, as the UI shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Flag {
    pub name: String,
    pub help: String,
    pub kind: Kind,
    pub value_name: Option<String>,
    pub default: Option<String>,
    pub editable: bool,
    /// Why it cannot be edited here, when it cannot.
    pub reason: Option<String>,
}

/// Token, password and PSK flags: the credentials that guard the thing doing the editing.
fn is_secret(name: &str) -> bool {
    name.ends_with("-token") || name == "token" || name.contains("password") || name.contains("psk")
}

/// Flags that say where the program sends its tokens or what it trusts: another program sharing
/// the settings directory could otherwise redirect them. Set these on the command line.
fn is_destination(name: &str) -> bool {
    name.ends_with("-url")
        || name.ends_with("-file")
        || matches!(name, "controller" | "aggregator")
        || name.contains("tls-cert")
        || name.contains("tls-key")
}

fn not_editable(name: &str) -> Option<&'static str> {
    if NOT_EDITABLE.contains(&name) {
        Some("acts once or locates this file")
    } else if name.starts_with("insecure-") || name.starts_with("allow-insecure-") {
        Some("a safety override: set it on the command line or in the service unit")
    } else if is_secret(name) {
        Some("a credential: set it on the command line or in the service unit")
    } else if is_destination(name) {
        Some("says where tokens go or what is trusted: set it on the command line or in the service unit")
    } else {
        None
    }
}

fn kind(a: &clap::Arg) -> Kind {
    match a.get_action() {
        ArgAction::SetTrue | ArgAction::SetFalse => Kind::Bool,
        ArgAction::Append => Kind::List,
        _ => Kind::Text,
    }
}

/// Every long flag of `cmd`.
pub fn schema(cmd: &Command) -> Vec<Flag> {
    let mut out: Vec<Flag> = cmd
        .get_arguments()
        .filter_map(|a| {
            let name = a.get_long()?.to_string();
            let reason = not_editable(&name);
            Some(Flag {
                help: a.get_help().map(ToString::to_string).unwrap_or_default(),
                kind: kind(a),
                value_name: a
                    .get_value_names()
                    .and_then(|v| v.first())
                    .map(ToString::to_string),
                default: a
                    .get_default_values()
                    .first()
                    .map(|v| v.to_string_lossy().into_owned()),
                editable: reason.is_none(),
                reason: reason.map(str::to_string),
                name,
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The arguments `values` stand for. Flags the program does not know, or that cannot be set
/// from the file, are returned in the second list (a newer file read by an older build must not
/// stop it from starting).
fn to_args(
    cmd: &Command,
    values: &Values,
    given: &HashSet<String>,
) -> (Vec<OsString>, Vec<String>) {
    let mut args = Vec::new();
    let mut skipped = Vec::new();
    for (name, value) in values {
        let known = cmd
            .get_arguments()
            .find(|a| a.get_long() == Some(name.as_str()));
        let Some(arg) = known.filter(|_| not_editable(name).is_none()) else {
            skipped.push(name.clone());
            continue;
        };
        if given.contains(arg.get_id().as_str()) {
            continue;
        }
        let flag = format!("--{name}");
        match (kind(arg), value) {
            (Kind::Bool, Value::Bool(true)) => args.push(flag.into()),
            (Kind::Bool, Value::Bool(false)) => {}
            (Kind::Text, Value::One(v)) => args.push(format!("{flag}={v}").into()),
            (Kind::List, Value::One(v)) => args.push(format!("{flag}={v}").into()),
            (Kind::List, Value::Many(vs)) => {
                args.extend(vs.iter().map(|v| OsString::from(format!("{flag}={v}"))));
            }
            _ => skipped.push(name.clone()),
        }
    }
    (args, skipped)
}

/// The shape check the UI backend can make without the program's parser: every key is an
/// editable flag in `flags` and its value has the flag's shape. The program itself runs the
/// full parser on the change ([`Guard`]).
pub fn check_flags(flags: &[Flag], values: &Values) -> Result<(), String> {
    for (name, value) in values {
        let flag = flags
            .iter()
            .find(|f| &f.name == name)
            .ok_or_else(|| format!("no such flag: --{name}"))?;
        if let Some(why) = &flag.reason {
            return Err(format!("--{name} cannot be set here: {why}"));
        }
        let fits = matches!(
            (flag.kind, value),
            (Kind::Bool, Value::Bool(_))
                | (Kind::Text, Value::One(_))
                | (Kind::List, Value::One(_) | Value::Many(_))
        );
        if !fits {
            return Err(format!("--{name}: the value has the wrong shape"));
        }
        let texts: Vec<&String> = match value {
            Value::One(v) => vec![v],
            Value::Many(vs) => vs.iter().collect(),
            Value::Bool(_) => vec![],
        };
        if texts.iter().any(|t| t.is_empty() || t.contains('\n')) {
            return Err(format!(
                "--{name}: empty or multi-line values are not accepted"
            ));
        }
    }
    Ok(())
}

/// Checks `values` for `program`: every key is an editable flag of `cmd`, has the right shape,
/// and the parser accepts the lot (so a bad number or a conflict is refused here, not at the
/// restart). Required flags are not demanded: they come from the command line.
pub fn check(cmd: &Command, values: &Values) -> Result<(), String> {
    for name in values.keys() {
        let Some(_) = cmd
            .get_arguments()
            .find(|a| a.get_long() == Some(name.as_str()))
        else {
            return Err(format!("no such flag: --{name}"));
        };
        if let Some(why) = not_editable(name) {
            return Err(format!("--{name} cannot be set here: {why}"));
        }
    }
    let (args, skipped) = to_args(cmd, values, &HashSet::new());
    if let Some(name) = skipped.first() {
        return Err(format!("--{name}: the value has the wrong shape"));
    }
    let relaxed = cmd.clone().mut_args(|a| a.required(false));
    let argv = std::iter::once(OsString::from("program")).chain(args);
    relaxed
        .try_get_matches_from(argv)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

// ---- layering and the boot-time undo --------------------------------------------------------

/// The value of `--name` in `argv`, either `--name=v` or `--name v`.
fn flag_value(argv: &[OsString], name: &str) -> Option<PathBuf> {
    let long = format!("--{name}");
    let mut it = argv.iter().skip(1);
    while let Some(a) = it.next() {
        let s = a.to_string_lossy();
        if let Some(v) = s.strip_prefix(&format!("{long}=")) {
            return Some(PathBuf::from(v));
        }
        if s == long {
            return it.next().map(PathBuf::from);
        }
    }
    None
}

/// The ids of the arguments the command line gives, by clap's own reckoning (so aliases and
/// combined short flags count). A command line clap cannot parse gives none: the real parse
/// then reports the error.
fn given_on_command_line(cmd: &Command, argv: &[OsString]) -> HashSet<String> {
    let relaxed = cmd
        .clone()
        .mut_args(|a| a.required(false))
        .ignore_errors(true);
    let Ok(matches) = relaxed.try_get_matches_from(argv) else {
        return HashSet::new();
    };
    cmd.get_arguments()
        .filter(|a| {
            matches.value_source(a.get_id().as_str())
                == Some(clap::parser::ValueSource::CommandLine)
        })
        .map(|a| a.get_id().to_string())
        .collect()
}

/// What to do with a file at a start.
#[derive(Debug, PartialEq, Eq)]
enum Boot {
    /// Nothing is pending.
    Plain,
    /// A pending change gets another try; this is the file to store.
    Try(File),
    /// A pending change is undone; this is the file to store.
    Undo(File),
}

fn decide_boot(file: &File, now: u64) -> Boot {
    let Some(p) = &file.pending else {
        return Boot::Plain;
    };
    if p.attempts >= MAX_ATTEMPTS || now > p.applied_at.saturating_add(HARD_DEADLINE_SECS) {
        return Boot::Undo(undone(file));
    }
    let mut next = file.clone();
    if let Some(p) = next.pending.as_mut() {
        p.attempts += 1;
    }
    Boot::Try(next)
}

/// `file` with the previous values back and nothing pending.
pub fn undone(file: &File) -> File {
    File {
        rev: file.rev + 1,
        values: file.previous.clone().unwrap_or_default(),
        previous: None,
        pending: None,
        error: None,
    }
}

/// The result of [`layer`].
pub struct Layered {
    /// `argv` with the file's flags added.
    pub argv: Vec<OsString>,
    /// Watches the file while the program runs; `None` without `--settings-dir`.
    pub guard: Option<Guard>,
    /// What happened while layering. Logging is not set up yet at that point, so
    /// [`Layered::start`] says it once it is.
    notes: Vec<Note>,
}

struct Note {
    warn: bool,
    text: String,
}

impl Layered {
    /// Call once logging is up and inside the runtime: logs what layering did and starts the
    /// guard, which ends the process with [`RESTART_EXIT`] when the file asks for a restart.
    pub fn start(self) {
        for n in &self.notes {
            if n.warn {
                tracing::warn!("settings: {}", n.text);
            } else {
                tracing::info!("settings: {}", n.text);
            }
        }
        if let Some(g) = self.guard {
            tokio::spawn(g.run(|| std::process::exit(RESTART_EXIT)));
        }
    }
}

/// Adds the settings file's flags to `argv` (the real command line, program name first), after
/// settling a pending change at this start. Without `--settings-dir`, or with a file that
/// cannot be read, `argv` is returned as it came (the program starts as before).
pub fn layer(program: &str, cmd: &Command, argv: Vec<OsString>) -> Layered {
    let none = |argv, notes| Layered {
        argv,
        guard: None,
        notes,
    };
    let mut notes = Vec::new();
    let mut warn = |text: String| notes.push(Note { warn: true, text });
    let Some(dir) = flag_value(&argv, "settings-dir") else {
        return none(argv, notes);
    };
    let mut file = match read(&dir, program) {
        Ok(Some(f)) => f,
        Ok(None) => File::default(),
        Err(e) => {
            warn(format!("file unreadable, starting without it: {e}"));
            return none(argv, notes);
        }
    };
    let at = now();
    // A dry run (`--check`, `--help`, `--version`, and the run-once modes) is not a start: it
    // must not use up a try.
    let dry = argv.iter().skip(1).any(|a| {
        let s = a.to_string_lossy();
        DRY_FLAGS
            .iter()
            .any(|f| s == *f || s.starts_with(&format!("{f}=")))
    });
    // Whether the file can be written. When a pending change cannot be counted or undone on
    // disk, nothing could ever end the trial: use the previous values and watch nothing,
    // rather than restart in a loop against a file that cannot be changed.
    let mut writable = true;
    match decide_boot(&file, at) {
        Boot::Plain => {}
        Boot::Try(next) => {
            if !dry {
                if let Err(e) = write(&dir, program, &next) {
                    warn(format!(
                        "a settings change is pending but this directory cannot be written ({e}); using the previous values"
                    ));
                    writable = false;
                    file = undone(&file);
                } else {
                    file = next;
                }
            } else {
                file = next;
            }
        }
        Boot::Undo(next) => {
            warn("the last change did not hold; going back to the previous values".into());
            if !dry {
                if let Err(e) = write(&dir, program, &next) {
                    warn(format!("could not store the undo ({e}); the settings directory must be writable by this program"));
                    writable = false;
                }
            }
            file = next;
        }
    }
    if !dry {
        let body =
            serde_json::to_vec_pretty(&SchemaFile { flags: schema(cmd) }).unwrap_or_default();
        if let Err(e) = write_atomic(&dir, &format!("{program}.schema.json"), &body) {
            warn(format!("could not publish the flag list: {e}"));
        }
    }
    let given = given_on_command_line(cmd, &argv);
    let (args, skipped) = to_args(cmd, &file.values, &given);
    for name in skipped {
        warn(format!(
            "--{name} ignored (unknown to this build, or not settable here)"
        ));
    }
    let mut out = argv;
    out.extend(args);
    Layered {
        argv: out,
        guard: writable.then(|| Guard {
            dir,
            program: program.to_string(),
            booted: file,
            started: at,
            cmd: cmd.clone(),
        }),
        notes,
    }
}

// ---- the running program's side ---------------------------------------------------------------

/// What the guard found.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Nothing,
    /// The file holds other values than this run started with: restart to take them.
    Restart,
    /// Still unconfirmed after [`CONFIRM_SECS`]: put the previous values back, then restart.
    Undo,
    /// The new values do not parse for this program: put the previous values back, say why,
    /// and keep running.
    Reject(String),
}

/// Watches the settings file of a running program.
pub struct Guard {
    dir: PathBuf,
    program: String,
    booted: File,
    started: u64,
    cmd: Command,
}

impl Guard {
    fn judge(&self, current: Option<&File>, now: u64) -> Action {
        let Some(cur) = current else {
            return Action::Nothing;
        };
        if cur.values != self.booted.values {
            return match check(&self.cmd, &cur.values) {
                Ok(()) => Action::Restart,
                Err(e) => Action::Reject(e),
            };
        }
        if cur.pending.is_some() && now >= self.started + CONFIRM_SECS {
            return Action::Undo;
        }
        Action::Nothing
    }

    /// Looks every second and calls `restart` (which should end the process with
    /// [`RESTART_EXIT`]) when the file asks for it.
    pub async fn run(self, restart: impl Fn()) {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        // A refused file that cannot be undone (no pending mark) is logged once per revision.
        let mut refused_rev = None;
        loop {
            tick.tick().await;
            let current = read(&self.dir, &self.program).ok().flatten();
            match self.judge(current.as_ref(), now()) {
                Action::Nothing => {}
                Action::Restart => {
                    tracing::info!(program = %self.program, "settings changed; restarting to apply them");
                    restart();
                }
                Action::Reject(why) => {
                    let rev = current.as_ref().map(|c| c.rev);
                    if refused_rev == rev {
                        continue;
                    }
                    refused_rev = rev;
                    tracing::warn!(program = %self.program, error = %why, "settings change refused; undoing it if it is a trial");
                    if let Err(e) = self.undo_if_unchanged(current.as_ref(), Some(why)) {
                        tracing::error!(program = %self.program, error = %e,
                            "could not undo the settings change; not watching any more");
                        return;
                    }
                }
                Action::Undo => {
                    // The file is read again right before the write: a confirm that landed in
                    // between must win.
                    match self.undo_if_unchanged(current.as_ref(), None) {
                        Ok(true) => {
                            tracing::warn!(program = %self.program, "settings change not confirmed in time; undone");
                            restart();
                        }
                        Ok(false) => {}
                        Err(e) => {
                            // Restarting would only find the same values again.
                            tracing::error!(program = %self.program, error = %e,
                                "could not undo the settings change; not restarting");
                            return;
                        }
                    }
                }
            }
        }
    }

    /// Puts the previous values back if the file is still exactly as `seen` (same revision,
    /// still pending). `Ok(false)` when it moved meanwhile.
    fn undo_if_unchanged(
        &self,
        seen: Option<&File>,
        error: Option<String>,
    ) -> Result<bool, String> {
        let Some(seen) = seen else {
            return Ok(false);
        };
        let latest = read(&self.dir, &self.program)?;
        match latest {
            Some(l) if l.rev == seen.rev && l.pending.is_some() => {
                let mut back = undone(&l);
                back.error = error;
                write(&self.dir, &self.program, &back)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

// ---- the writer's side (the UI backend) --------------------------------------------------------

/// Why a writer's request was not carried out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyError {
    /// The values are not acceptable (unknown flag, wrong shape, refused by the parser).
    Invalid(String),
    /// The file is in a state that does not allow it (changed since read, a change is waiting,
    /// nothing to do).
    Conflict(String),
    /// The file could not be read or written.
    Io(String),
}

impl std::fmt::Display for ApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (Self::Invalid(m) | Self::Conflict(m) | Self::Io(m)) = self;
        f.write_str(m)
    }
}

impl std::error::Error for ApplyError {}

fn conflict(m: &str) -> ApplyError {
    ApplyError::Conflict(m.into())
}

/// Starts a trial apply of `values`: they become the file's values, the old ones are kept as
/// `previous`, and the change is pending. `expect_rev` is the revision the caller read; a file
/// that moved since is refused. A change that is still pending cannot be stacked on.
pub fn apply(
    dir: &Path,
    program: &str,
    cmd: &Command,
    values: Values,
    expect_rev: u64,
    now: u64,
) -> Result<File, ApplyError> {
    let checked = check(cmd, &values).map_err(ApplyError::Invalid);
    apply_with(dir, program, checked, values, expect_rev, now)
}

/// [`apply`] for a writer that has only the program's published flag list (the UI backend):
/// the shapes are checked here, the full parser runs in the program ([`Guard`]).
pub fn apply_flags(
    dir: &Path,
    program: &str,
    flags: &[Flag],
    values: Values,
    expect_rev: u64,
    now: u64,
) -> Result<File, ApplyError> {
    let checked = check_flags(flags, &values).map_err(ApplyError::Invalid);
    apply_with(dir, program, checked, values, expect_rev, now)
}

fn apply_with(
    dir: &Path,
    program: &str,
    checked: Result<(), ApplyError>,
    values: Values,
    expect_rev: u64,
    now: u64,
) -> Result<File, ApplyError> {
    checked?;
    let cur = read(dir, program)
        .map_err(ApplyError::Io)?
        .unwrap_or_default();
    if cur.rev != expect_rev {
        return Err(conflict(
            "the settings changed since you loaded them; reload and try again",
        ));
    }
    if cur.pending.is_some() {
        return Err(conflict(
            "a change is waiting for confirmation; confirm or undo it first",
        ));
    }
    if cur.values == values {
        return Err(conflict("nothing changed"));
    }
    let next = File {
        rev: cur.rev + 1,
        previous: Some(cur.values),
        values,
        pending: Some(Pending {
            applied_at: now,
            attempts: 0,
        }),
        error: None,
    };
    write(dir, program, &next).map_err(ApplyError::Io)?;
    Ok(next)
}

/// Makes the pending change final.
pub fn confirm(dir: &Path, program: &str) -> Result<File, ApplyError> {
    let mut cur = read(dir, program)
        .map_err(ApplyError::Io)?
        .ok_or_else(|| conflict("no settings file"))?;
    if cur.pending.take().is_none() {
        return Err(conflict("nothing is waiting for confirmation"));
    }
    cur.previous = None;
    cur.error = None;
    cur.rev += 1;
    write(dir, program, &cur).map_err(ApplyError::Io)?;
    Ok(cur)
}

/// Puts the previous values back (the running program notices and restarts).
pub fn undo(dir: &Path, program: &str) -> Result<File, ApplyError> {
    let cur = read(dir, program)
        .map_err(ApplyError::Io)?
        .ok_or_else(|| conflict("no settings file"))?;
    if cur.pending.is_none() {
        return Err(conflict("nothing is waiting for confirmation"));
    }
    let next = undone(&cur);
    write(dir, program, &next).map_err(ApplyError::Io)?;
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser, Debug)]
    struct Demo {
        #[arg(long)]
        settings_dir: Option<PathBuf>,
        #[arg(short, long, default_value = "config.yaml")]
        config: PathBuf,
        #[arg(long)]
        listen: String,
        #[arg(long)]
        auth_token: Option<String>,
        #[arg(long)]
        plugins: bool,
        #[arg(long)]
        tag: Vec<String>,
        #[arg(long)]
        workers: Option<u32>,
        #[arg(long)]
        check: bool,
        #[arg(long, alias = "old-name")]
        renamed: Option<String>,
        #[arg(long)]
        insecure_no_auth: bool,
        #[arg(long)]
        allow_insecure_secrets: bool,
    }

    fn cmd() -> Command {
        <Demo as clap::CommandFactory>::command()
    }

    fn argv(parts: &[&str]) -> Vec<OsString> {
        std::iter::once("demo")
            .chain(parts.iter().copied())
            .map(OsString::from)
            .collect()
    }

    fn vals(pairs: &[(&str, Value)]) -> Values {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect()
    }

    fn one(s: &str) -> Value {
        Value::One(s.into())
    }

    fn parse(layered: &Layered) -> Demo {
        Demo::try_parse_from(&layered.argv).unwrap()
    }

    #[test]
    fn the_schema_comes_from_the_clap_definition_and_marks_what_cannot_be_edited() {
        let flags = schema(&cmd());
        let by = |n: &str| flags.iter().find(|f| f.name == n).unwrap();
        assert_eq!(by("plugins").kind, Kind::Bool);
        assert_eq!(by("tag").kind, Kind::List);
        assert_eq!(by("config").default.as_deref(), Some("config.yaml"));
        assert!(by("listen").editable);
        for locked in [
            "auth-token",
            "settings-dir",
            "check",
            "insecure-no-auth",
            "allow-insecure-secrets",
        ] {
            assert!(!by(locked).editable, "{locked}");
            assert!(by(locked).reason.is_some());
        }
    }

    #[test]
    fn check_runs_the_real_parser_and_refuses_what_it_would_refuse() {
        let ok = vals(&[("workers", one("4")), ("plugins", Value::Bool(true))]);
        assert_eq!(check(&cmd(), &ok), Ok(()));
        let e = check(&cmd(), &vals(&[("workers", one("many"))])).unwrap_err();
        assert!(e.contains("workers"), "{e}");
        assert!(check(&cmd(), &vals(&[("nope", one("x"))]))
            .unwrap_err()
            .contains("no such flag"));
        assert!(check(&cmd(), &vals(&[("auth-token", one("x"))]))
            .unwrap_err()
            .contains("credential"));
        assert!(check(&cmd(), &vals(&[("plugins", one("yes"))]))
            .unwrap_err()
            .contains("wrong shape"));
    }

    #[test]
    fn file_values_fill_in_and_the_command_line_wins() {
        let dir = tempfile::tempdir().unwrap();
        let file = File {
            values: vals(&[
                ("listen", one("0.0.0.0:1")),
                ("workers", one("8")),
                ("plugins", Value::Bool(true)),
                ("tag", Value::Many(vec!["a".into(), "b".into()])),
                ("config", one("/etc/from-file.yaml")),
            ]),
            ..File::default()
        };
        write(dir.path(), "demo", &file).unwrap();
        let dir_arg = dir.path().to_str().unwrap().to_string();
        let l = layer(
            "demo",
            &cmd(),
            argv(&[
                "--settings-dir",
                &dir_arg,
                "--listen=127.0.0.1:2",
                "-c",
                "/etc/cli.yaml",
            ]),
        );
        let d = parse(&l);
        assert_eq!(d.listen, "127.0.0.1:2", "the command line wins");
        assert_eq!(d.config, PathBuf::from("/etc/cli.yaml"), "short form too");
        assert_eq!(d.workers, Some(8));
        assert!(d.plugins);
        assert_eq!(d.tag, ["a", "b"]);
        assert!(l.guard.is_some());
    }

    #[test]
    fn without_a_settings_dir_or_with_a_broken_file_the_program_starts_as_before() {
        let l = layer("demo", &cmd(), argv(&["--listen=x"]));
        assert_eq!(l.argv, argv(&["--listen=x"]));
        assert!(l.guard.is_none());
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(path(dir.path(), "demo"), "{ not json").unwrap();
        let d = dir.path().to_str().unwrap();
        let l = layer("demo", &cmd(), argv(&["--settings-dir", d, "--listen=x"]));
        assert!(l.guard.is_none());
        assert_eq!(parse(&l).listen, "x");
    }

    #[test]
    fn a_secret_or_unknown_key_in_the_file_is_ignored_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let file = File {
            values: vals(&[
                ("auth-token", one("leaked")),
                ("from-the-future", one("x")),
                ("workers", one("2")),
            ]),
            ..File::default()
        };
        write(dir.path(), "demo", &file).unwrap();
        let d = dir.path().to_str().unwrap();
        let l = layer("demo", &cmd(), argv(&["--settings-dir", d, "--listen=x"]));
        let parsed = parse(&l);
        assert_eq!(parsed.auth_token, None);
        assert_eq!(parsed.workers, Some(2));
    }

    #[test]
    fn a_pending_change_is_counted_per_start_and_undone_after_the_attempts() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().to_str().unwrap();
        let start = || layer("demo", &cmd(), argv(&["--settings-dir", d, "--listen=x"]));
        let applied = apply(
            dir.path(),
            "demo",
            &cmd(),
            vals(&[("workers", one("9"))]),
            0,
            now(),
        )
        .unwrap();
        assert_eq!(applied.rev, 1);
        for n in 1..=MAX_ATTEMPTS {
            assert_eq!(parse(&start()).workers, Some(9), "start {n} tries it");
            assert_eq!(
                read(dir.path(), "demo")
                    .unwrap()
                    .unwrap()
                    .pending
                    .unwrap()
                    .attempts,
                n
            );
        }
        // The next start finds it used up and goes back.
        assert_eq!(parse(&start()).workers, None);
        let f = read(dir.path(), "demo").unwrap().unwrap();
        assert!(f.pending.is_none() && f.previous.is_none() && f.values.is_empty());
    }

    #[test]
    fn a_change_older_than_the_deadline_is_undone_at_once() {
        let file = File {
            rev: 3,
            values: vals(&[("workers", one("9"))]),
            previous: Some(vals(&[("workers", one("1"))])),
            pending: Some(Pending {
                applied_at: 1_000,
                attempts: 0,
            }),
            error: None,
        };
        assert!(matches!(
            decide_boot(&file, 1_000 + HARD_DEADLINE_SECS),
            Boot::Try(_)
        ));
        let Boot::Undo(back) = decide_boot(&file, 1_000 + HARD_DEADLINE_SECS + 1) else {
            panic!("expected an undo");
        };
        assert_eq!(back.values, vals(&[("workers", one("1"))]));
        assert_eq!(back.rev, 4);
        assert_eq!(decide_boot(&back, 5_000), Boot::Plain);
    }

    #[test]
    fn apply_refuses_stale_stacked_and_empty_changes_and_confirm_makes_it_final() {
        let dir = tempfile::tempdir().unwrap();
        let go = |v: Values, rev| apply(dir.path(), "demo", &cmd(), v, rev, 10);
        let v1 = vals(&[("workers", one("2"))]);
        assert!(go(v1.clone(), 7)
            .unwrap_err()
            .to_string()
            .contains("changed since"));
        go(v1.clone(), 0).unwrap();
        let again = go(vals(&[("workers", one("3"))]), 1)
            .unwrap_err()
            .to_string();
        assert!(again.contains("waiting for confirmation"), "{again}");
        let f = confirm(dir.path(), "demo").unwrap();
        assert!(f.pending.is_none() && f.previous.is_none());
        assert_eq!(f.values, v1);
        assert!(go(v1, f.rev)
            .unwrap_err()
            .to_string()
            .contains("nothing changed"));
        assert!(confirm(dir.path(), "demo")
            .unwrap_err()
            .to_string()
            .contains("nothing"));
        assert!(go(vals(&[("workers", one("many"))]), f.rev).is_err());
    }

    #[test]
    fn the_guard_restarts_on_new_values_and_undoes_an_unconfirmed_change() {
        let booted = File {
            rev: 1,
            values: vals(&[("workers", one("2"))]),
            previous: Some(Values::new()),
            pending: Some(Pending {
                applied_at: 100,
                attempts: 1,
            }),
            error: None,
        };
        let g = Guard {
            dir: PathBuf::new(),
            program: "demo".into(),
            booted: booted.clone(),
            started: 100,
            cmd: cmd(),
        };
        assert_eq!(g.judge(None, 500), Action::Nothing);
        assert_eq!(
            g.judge(Some(&booted), 100 + CONFIRM_SECS - 1),
            Action::Nothing
        );
        assert_eq!(g.judge(Some(&booted), 100 + CONFIRM_SECS), Action::Undo);
        let confirmed = File {
            pending: None,
            previous: None,
            rev: 2,
            ..booted.clone()
        };
        assert_eq!(g.judge(Some(&confirmed), 9_999), Action::Nothing);
        let newer = File {
            values: vals(&[("workers", one("5"))]),
            ..booted
        };
        assert_eq!(g.judge(Some(&newer), 101), Action::Restart);
        // Values the program's own parser refuses are put back without a restart.
        let bad = File {
            values: vals(&[("workers", one("many"))]),
            ..newer
        };
        let Action::Reject(why) = g.judge(Some(&bad), 101) else {
            panic!("expected a rejection");
        };
        assert!(why.contains("workers"), "{why}");
    }

    #[test]
    fn a_refused_trial_is_put_back_with_the_reason_and_a_confirmed_file_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let prev = vals(&[("workers", one("2"))]);
        let trial = File {
            rev: 7,
            values: vals(&[("workers", one("many"))]),
            previous: Some(prev.clone()),
            pending: Some(Pending {
                applied_at: 1,
                attempts: 0,
            }),
            error: None,
        };
        write(dir.path(), "demo", &trial).unwrap();
        let g = Guard {
            dir: dir.path().to_path_buf(),
            program: "demo".into(),
            booted: File {
                rev: 6,
                values: prev.clone(),
                ..File::default()
            },
            started: 100,
            cmd: cmd(),
        };
        assert_eq!(
            g.undo_if_unchanged(Some(&trial), Some("bad".into())),
            Ok(true)
        );
        let back = read(dir.path(), "demo").unwrap().unwrap();
        assert_eq!(
            (&back.values, &back.pending, back.error.as_deref()),
            (&prev, &None, Some("bad"))
        );
        // Now confirmed (no pending mark): nothing to undo.
        assert_eq!(
            g.undo_if_unchanged(Some(&back), Some("again".into())),
            Ok(false)
        );
    }

    #[test]
    fn flags_that_say_where_tokens_go_or_what_is_trusted_are_not_editable() {
        for n in [
            "controller-url",
            "aggregator-admin-url",
            "controller",
            "aggregator",
            "ca-file",
            "users-file",
            "tls-cert",
            "tls-key",
            "plugin-webhook-tls-cert",
        ] {
            assert!(not_editable(n).is_some(), "{n} must be locked");
        }
        assert!(not_editable("workers").is_none());
    }

    #[test]
    fn the_program_publishes_its_flag_list_and_the_writer_checks_shapes_against_it() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().to_str().unwrap();
        assert_eq!(read_schema(dir.path(), "demo"), None);
        // A dry run publishes nothing; a real start does.
        layer("demo", &cmd(), argv(&["--settings-dir", d, "--check"]));
        assert_eq!(read_schema(dir.path(), "demo"), None);
        layer("demo", &cmd(), argv(&["--settings-dir", d, "--listen=x"]));
        let flags = read_schema(dir.path(), "demo").unwrap();
        assert_eq!(flags, schema(&cmd()));
        let ok = vals(&[
            ("workers", one("4")),
            ("tag", Value::Many(vec!["a".into()])),
        ]);
        assert_eq!(check_flags(&flags, &ok), Ok(()));
        for (bad, why) in [
            (vals(&[("nope", one("x"))]), "no such flag"),
            (vals(&[("auth-token", one("x"))]), "credential"),
            (vals(&[("plugins", one("yes"))]), "wrong shape"),
            (vals(&[("workers", one(""))]), "empty"),
            (vals(&[("workers", one("1\n--check"))]), "multi-line"),
        ] {
            let e = check_flags(&flags, &bad).unwrap_err();
            assert!(e.contains(why), "{e}");
        }
    }

    /// Whether this process can write into a directory made read-only (root cannot be stopped).
    fn read_only(dir: &Path) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555)).unwrap();
            let blocked = std::fs::write(dir.join("probe"), "x").is_err();
            if !blocked {
                let _ = std::fs::remove_file(dir.join("probe"));
                eprintln!("skipped: this user can write into a read-only directory");
            }
            blocked
        }
        #[cfg(not(unix))]
        {
            let _ = dir;
            false
        }
    }

    #[test]
    fn an_alias_on_the_command_line_beats_the_file_instead_of_clashing_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let file = File {
            values: vals(&[("renamed", one("from-file"))]),
            ..File::default()
        };
        write(dir.path(), "demo", &file).unwrap();
        let d = dir.path().to_str().unwrap();
        let l = layer(
            "demo",
            &cmd(),
            argv(&["--settings-dir", d, "--listen=x", "--old-name=from-cli"]),
        );
        assert_eq!(parse(&l).renamed.as_deref(), Some("from-cli"));
        let l = layer("demo", &cmd(), argv(&["--settings-dir", d, "--listen=x"]));
        assert_eq!(parse(&l).renamed.as_deref(), Some("from-file"));
    }

    #[test]
    fn run_once_modes_and_checks_do_not_use_up_a_try() {
        let dir = tempfile::tempdir().unwrap();
        apply(
            dir.path(),
            "demo",
            &cmd(),
            vals(&[("workers", one("9"))]),
            0,
            now(),
        )
        .unwrap();
        let d = dir.path().to_str().unwrap();
        for flag in [
            "--check",
            "--secret-list",
            "--hash-password",
            "--secret-set=a",
        ] {
            layer(
                "demo",
                &cmd(),
                argv(&["--settings-dir", d, "--listen=x", flag]),
            );
        }
        let f = read(dir.path(), "demo").unwrap().unwrap();
        assert_eq!(f.pending.unwrap().attempts, 0);
    }

    #[test]
    fn a_hostile_applied_at_does_not_overflow() {
        let file = File {
            values: vals(&[("workers", one("9"))]),
            pending: Some(Pending {
                applied_at: u64::MAX,
                attempts: 0,
            }),
            ..File::default()
        };
        assert!(matches!(decide_boot(&file, 5), Boot::Try(_)));
    }

    #[test]
    fn a_pending_change_in_a_directory_that_cannot_be_written_is_not_tried_and_not_watched() {
        let dir = tempfile::tempdir().unwrap();
        apply(
            dir.path(),
            "demo",
            &cmd(),
            vals(&[("workers", one("9"))]),
            0,
            now(),
        )
        .unwrap();
        if !read_only(dir.path()) {
            return;
        }
        let d = dir.path().to_str().unwrap();
        let l = layer("demo", &cmd(), argv(&["--settings-dir", d, "--listen=x"]));
        // The previous values (none) are used, and no guard is left to restart in a loop.
        assert_eq!(parse(&l).workers, None);
        assert!(l.guard.is_none());
    }

    #[test]
    fn the_late_undo_does_not_overwrite_a_confirm_and_a_failed_undo_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let applied = apply(
            dir.path(),
            "demo",
            &cmd(),
            vals(&[("workers", one("9"))]),
            0,
            now(),
        )
        .unwrap();
        let guard = Guard {
            dir: dir.path().to_path_buf(),
            program: "demo".into(),
            booted: applied.clone(),
            started: 0,
            cmd: cmd(),
        };
        // The guard saw `applied`; the operator confirms a moment later.
        confirm(dir.path(), "demo").unwrap();
        assert_eq!(guard.undo_if_unchanged(Some(&applied), None), Ok(false));
        let f = read(dir.path(), "demo").unwrap().unwrap();
        assert_eq!(
            f.values,
            vals(&[("workers", one("9"))]),
            "the confirmed value stays"
        );
        // Unchanged and pending: undone.
        let again = apply(
            dir.path(),
            "demo",
            &cmd(),
            vals(&[("workers", one("5"))]),
            f.rev,
            now(),
        )
        .unwrap();
        assert_eq!(guard.undo_if_unchanged(Some(&again), None), Ok(true));
        assert_eq!(
            read(dir.path(), "demo").unwrap().unwrap().values,
            vals(&[("workers", one("9"))])
        );
    }

    #[test]
    fn a_manual_undo_puts_the_previous_values_back() {
        let dir = tempfile::tempdir().unwrap();
        apply(
            dir.path(),
            "demo",
            &cmd(),
            vals(&[("workers", one("2"))]),
            0,
            10,
        )
        .unwrap();
        let back = undo(dir.path(), "demo").unwrap();
        assert!(back.values.is_empty() && back.pending.is_none());
        assert!(undo(dir.path(), "demo").is_err());
    }
}
