//! Command construction and shell discovery for PTY sessions.
//!
//! [`CommandBuilder`] assembles an executable, argv, working directory and a
//! *deterministic* environment overlay (backed by a [`BTreeMap`], never a
//! `HashMap`) plus `TERM`/`COLORTERM` handling, and materializes it into a
//! spawn-ready [`SpawnCommand`].
//!
//! Environment semantics (see [`CommandBuilder::build_envs`]):
//!
//! * When `clear_envs` is false the parent process environment is inherited;
//!   non-UTF-8 parent entries are skipped by `std::env::vars` (documented in
//!   std) and therefore do not reach the child.
//! * The configured overlay is applied on top of the inherited environment.
//! * `TERM` is always present in the result: the explicit `term` value wins,
//!   otherwise an inherited/overlay `TERM` is kept, otherwise the default
//!   [`DEFAULT_TERM`] is injected.
//! * `COLORTERM` is only injected when explicitly configured via
//!   [`CommandBuilder::colorterm`]/[`CommandBuilder::colorterm_opt`]; an
//!   inherited or overlay `COLORTERM` passes through untouched.
//!
//! Shell discovery precedence (see [`CommandBuilder::discover_shell`]):
//! an explicit path (returned unchanged so spawn errors are not hidden), then
//! `$SHELL` (non-empty, absolute), then the user's `pw_shell` from the passwd
//! database, then [`DEFAULT_SHELL`].

use std::collections::BTreeMap;
use std::ffi::{CStr, OsString};
use std::path::{Path, PathBuf};

/// `TERM` value injected when neither the builder nor the environment
/// specifies one.
pub const DEFAULT_TERM: &str = "xterm-256color";

/// Final fallback shell when no other discovery source yields one.
pub const DEFAULT_SHELL: &str = "/bin/sh";

/// POSIX launcher used to run a startup fragment before replacing itself with
/// the user's interactive shell on the same PTY.
pub const STARTUP_SHELL_LAUNCHER: &str = "/bin/sh";

/// Positional-argument script for [`startup_shell_argv`]. The fragment is
/// evaluated in a subshell so failure never prevents the interactive shell
/// from starting; `exec` then leaves interactive programs attached directly
/// to the original PTY.
pub const STARTUP_SHELL_SCRIPT: &str = "( eval \"$1\" ); exec \"$0\"";

/// Build the reusable argv contract for a pre-shell startup fragment.
///
/// The returned vector includes the executable at index zero and preserves
/// non-UTF-8 shell paths and fragments as [`OsString`] values. PTY hosts own
/// the actual spawn and may adapt this argv to their process API.
pub fn startup_shell_argv(
    shell: impl Into<OsString>,
    fragment: impl Into<OsString>,
) -> Vec<OsString> {
    vec![
        OsString::from(STARTUP_SHELL_LAUNCHER),
        OsString::from("-c"),
        OsString::from(STARTUP_SHELL_SCRIPT),
        shell.into(),
        fragment.into(),
    ]
}

/// A fully materialized command ready to be spawned by the platform layer.
///
/// `envs` is the complete child environment (merged parent env when
/// inheritance is enabled, then overlay, then `TERM`/`COLORTERM` handling).
/// `term` is the resolved `TERM` value and always equals `envs["TERM"]`;
/// `colorterm` is `envs.get("COLORTERM")` when a `COLORTERM` entry exists.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpawnCommand {
    /// Executable path to spawn.
    pub exe: PathBuf,
    /// Argument vector (not including the executable itself).
    pub args: Vec<OsString>,
    /// Working directory for the child, if any.
    pub cwd: Option<PathBuf>,
    /// Complete child environment, deterministically ordered.
    pub envs: BTreeMap<String, String>,
    /// Resolved terminal type; always equals `envs["TERM"]`.
    pub term: String,
    /// Resolved color terminal mode, if a `COLORTERM` entry is present.
    pub colorterm: Option<String>,
}

/// Builder for [`SpawnCommand`].
///
/// All mutations take `&mut self` and return `&mut Self` so calls chain.
/// The environment overlay is a [`BTreeMap`] so overlay iteration and the
/// final child environment are deterministic for identical inputs.
#[derive(Clone, Debug)]
pub struct CommandBuilder {
    exe: PathBuf,
    args: Vec<OsString>,
    cwd: Option<PathBuf>,
    env: BTreeMap<String, String>,
    clear_envs: bool,
    term: Option<String>,
    colorterm: Option<String>,
}

impl CommandBuilder {
    /// Starts a builder for the given executable.
    pub fn new(exe: impl Into<PathBuf>) -> Self {
        Self {
            exe: exe.into(),
            args: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            clear_envs: false,
            term: None,
            colorterm: None,
        }
    }

    /// Appends a single argument.
    pub fn arg(&mut self, arg: impl Into<OsString>) -> &mut Self {
        self.args.push(arg.into());
        self
    }

    /// Appends many arguments.
    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Sets the child working directory.
    pub fn cwd(&mut self, cwd: impl Into<PathBuf>) -> &mut Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Adds one overlay environment entry, overriding any inherited value.
    pub fn env(&mut self, key: impl Into<String>, value: impl Into<String>) -> &mut Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Adds many overlay environment entries.
    pub fn envs<K, V, I>(&mut self, envs: I) -> &mut Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        self.env
            .extend(envs.into_iter().map(|(k, v)| (k.into(), v.into())));
        self
    }

    /// Removes one key from the overlay. With inheritance enabled the parent
    /// value (if any) still reaches the child; combine with
    /// [`CommandBuilder::clear_envs`] to strip inherited entries.
    pub fn env_remove(&mut self, key: impl AsRef<str>) -> &mut Self {
        self.env.remove(key.as_ref());
        self
    }

    /// Controls whether the parent environment is inherited (`false`,
    /// default) or the child starts with only the overlay (`true`).
    pub fn clear_envs(&mut self, clear: bool) -> &mut Self {
        self.clear_envs = clear;
        self
    }

    /// Sets the explicit `TERM` value; overrides any inherited or overlay
    /// `TERM`.
    pub fn term(&mut self, term: impl Into<String>) -> &mut Self {
        self.term = Some(term.into());
        self
    }

    /// Sets or clears the explicit `TERM` value.
    pub fn term_opt(&mut self, term: Option<impl Into<String>>) -> &mut Self {
        self.term = term.map(Into::into);
        self
    }

    /// Sets the explicit `COLORTERM` value; injected into the child
    /// environment. Inherited or overlay `COLORTERM` is never removed.
    pub fn colorterm(&mut self, colorterm: impl Into<String>) -> &mut Self {
        self.colorterm = Some(colorterm.into());
        self
    }

    /// Sets or clears the explicit `COLORTERM` value.
    pub fn colorterm_opt(&mut self, colorterm: Option<impl Into<String>>) -> &mut Self {
        self.colorterm = colorterm.map(Into::into);
        self
    }

    /// Immutable view of the deterministic overlay map.
    pub fn envs_overlay(&self) -> &BTreeMap<String, String> {
        &self.env
    }

    /// Resolves the shell executable to run, using the documented precedence:
    ///
    /// 1. `explicit`, returned unchanged so an invalid requested shell is
    ///    rejected by spawn rather than silently replaced;
    /// 2. `$SHELL`, when non-empty and absolute;
    /// 3. the user's `pw_shell` from the passwd database, when non-empty;
    /// 4. [`DEFAULT_SHELL`].
    ///
    pub fn discover_shell(explicit: Option<&Path>) -> PathBuf {
        let shell_env = std::env::var("SHELL").ok();
        Self::discover_shell_from(explicit, shell_env.as_deref(), passwd_shell().as_deref())
    }

    /// Pure precedence core of [`CommandBuilder::discover_shell`], separated
    /// so the precedence contract is unit-testable without touching the
    /// ambient environment or the passwd database.
    fn discover_shell_from(
        explicit: Option<&Path>,
        shell_env: Option<&str>,
        passwd_shell: Option<&Path>,
    ) -> PathBuf {
        if let Some(path) = explicit {
            return path.to_path_buf();
        }
        if let Some(shell) = shell_env {
            if !shell.is_empty() {
                let path = Path::new(shell);
                if path.is_absolute() {
                    return path.to_path_buf();
                }
            }
        }
        if let Some(shell) = passwd_shell {
            if !shell.as_os_str().is_empty() {
                return shell.to_path_buf();
            }
        }
        PathBuf::from(DEFAULT_SHELL)
    }

    /// Sets the executable to the shell resolved by
    /// [`CommandBuilder::discover_shell`].
    pub fn shell(&mut self, explicit: Option<&Path>) -> &mut Self {
        self.exe = Self::discover_shell(explicit);
        self
    }

    /// Builds the complete child environment as a deterministic
    /// [`BTreeMap`]: inherited parent env (unless `clear_envs`), then the
    /// overlay, then `TERM`/`COLORTERM` handling as documented in the module
    /// docs. `TERM` is guaranteed present in the result.
    pub fn build_envs(&self) -> BTreeMap<String, String> {
        let mut envs: BTreeMap<String, String> = if self.clear_envs {
            BTreeMap::new()
        } else {
            std::env::vars().collect()
        };
        for (key, value) in &self.env {
            envs.insert(key.clone(), value.clone());
        }
        if let Some(term) = &self.term {
            envs.insert("TERM".to_string(), term.clone());
        } else {
            envs.entry("TERM".to_string())
                .or_insert_with(|| DEFAULT_TERM.to_string());
        }
        if let Some(colorterm) = &self.colorterm {
            envs.insert("COLORTERM".to_string(), colorterm.clone());
        }
        envs
    }

    /// Materializes this builder into a spawn-ready [`SpawnCommand`].
    ///
    /// When inheritance is enabled the parent environment is snapshotted at
    /// this call, so the result is a deterministic value; two calls with
    /// identical inputs and an unchanged parent env produce equal commands.
    pub fn to_spawn_command(&self) -> SpawnCommand {
        let envs = self.build_envs();
        SpawnCommand {
            exe: self.exe.clone(),
            args: self.args.clone(),
            cwd: self.cwd.clone(),
            term: envs
                .get("TERM")
                .cloned()
                .unwrap_or_else(|| DEFAULT_TERM.to_string()),
            colorterm: envs.get("COLORTERM").cloned(),
            envs,
        }
    }
}

/// Reads the invoking user's login shell from the passwd database.
///
/// Returns `None` when the database has no record, the record has no shell,
/// or the shell is not valid UTF-8.
fn passwd_shell() -> Option<PathBuf> {
    unsafe {
        // SAFETY: all operations in this block are raw libc interactions, and
        // each invariant is stated here:
        // - `getuid` and `getpwuid` are plain C calls with no Rust
        //   preconditions.
        // - `getpwuid` returns either NULL (checked before any dereference) or
        //   a pointer to a `passwd` record stored in libc-managed,
        //   thread-specific storage that stays valid until the next passwd
        //   call in this thread. This crate makes no other passwd calls, and
        //   the record is only read (never written) before its contents are
        //   copied into owned storage below, so the record pointer cannot
        //   dangle while it is in use.
        // - After the NULL check, `(*pwd).pw_shell` reads a field of a valid
        //   record. For a valid record POSIX guarantees `pw_shell` is either
        //   NULL or a pointer to a NUL-terminated C string; the NULL case is
        //   checked before `CStr::from_ptr` is called.
        // - `CStr::from_ptr(shell_ptr)` therefore reads a valid,
        //   NUL-terminated string, and its borrow ends before this function
        //   returns, so no use-after-free is possible.
        let pwd = libc::getpwuid(libc::getuid());
        if pwd.is_null() {
            return None;
        }
        let shell_ptr = (*pwd).pw_shell;
        if shell_ptr.is_null() {
            return None;
        }
        match CStr::from_ptr(shell_ptr).to_str() {
            Ok(shell) if !shell.is_empty() => Some(PathBuf::from(shell)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    /// Creates a scratch file that certainly exists, with a caller-unique
    /// name so parallel test runs cannot collide.
    fn scratch_file(name: &str) -> PathBuf {
        let file = format!("mr-crabs-pty-{}-{name}", std::process::id());
        std::fs::write(std::env::temp_dir().join(&file), b"").expect("write scratch file");
        std::env::temp_dir().join(file)
    }

    #[test]
    fn startup_shell_argv_has_exact_same_pty_contract() {
        let argv = startup_shell_argv("/bin/zsh", "rustfetch");
        assert_eq!(
            argv,
            vec![
                OsString::from("/bin/sh"),
                OsString::from("-c"),
                OsString::from("( eval \"$1\" ); exec \"$0\""),
                OsString::from("/bin/zsh"),
                OsString::from("rustfetch"),
            ]
        );
    }

    #[test]
    fn startup_shell_argv_preserves_non_utf8_values() {
        let shell = OsString::from_vec(vec![b'/', b'x', 0xff]);
        let fragment = OsString::from_vec(vec![b'f', 0xfe]);
        let argv = startup_shell_argv(shell.clone(), fragment.clone());
        assert_eq!(argv[3].as_bytes(), shell.as_bytes());
        assert_eq!(argv[4].as_bytes(), fragment.as_bytes());
    }

    #[test]
    fn discover_shell_precedence_explicit_existing_absolute_wins() {
        let explicit = scratch_file("explicit-shell");
        let resolved = CommandBuilder::discover_shell_from(
            Some(&explicit),
            Some("/bin/zsh"),
            Some(Path::new("/bin/bash")),
        );
        assert_eq!(resolved, explicit);
        let _ = std::fs::remove_file(&explicit);
    }

    #[test]
    fn discover_shell_preserves_relative_explicit_for_spawn_validation() {
        let explicit = Path::new("not-a-real-shell");
        let resolved = CommandBuilder::discover_shell_from(
            Some(explicit),
            Some("/bin/zsh"),
            Some(Path::new("/bin/bash")),
        );
        assert_eq!(resolved, explicit);
    }

    #[test]
    fn discover_shell_preserves_nonexistent_explicit_for_spawn_validation() {
        let explicit = Path::new("/definitely/not/a/real/shell");
        let resolved = CommandBuilder::discover_shell_from(
            Some(explicit),
            Some("/bin/zsh"),
            Some(Path::new("/bin/bash")),
        );
        assert_eq!(resolved, explicit);
    }

    #[test]
    fn discover_shell_precedence_shell_env_beats_passwd() {
        let resolved = CommandBuilder::discover_shell_from(
            None,
            Some("/bin/zsh"),
            Some(Path::new("/bin/bash")),
        );
        assert_eq!(resolved, PathBuf::from("/bin/zsh"));
    }

    #[test]
    fn discover_shell_precedence_relative_shell_env_ignored() {
        let resolved = CommandBuilder::discover_shell_from(
            None,
            Some("bin/zsh"),
            Some(Path::new("/bin/bash")),
        );
        assert_eq!(resolved, PathBuf::from("/bin/bash"));
    }

    #[test]
    fn discover_shell_precedence_empty_shell_env_ignored() {
        let resolved =
            CommandBuilder::discover_shell_from(None, Some(""), Some(Path::new("/bin/bash")));
        assert_eq!(resolved, PathBuf::from("/bin/bash"));
    }

    #[test]
    fn discover_shell_precedence_passwd_beats_default() {
        let resolved =
            CommandBuilder::discover_shell_from(None, None, Some(Path::new("/bin/bash")));
        assert_eq!(resolved, PathBuf::from("/bin/bash"));
    }

    #[test]
    fn discover_shell_precedence_default_fallback() {
        let resolved = CommandBuilder::discover_shell_from(None, None, None);
        assert_eq!(resolved, PathBuf::from(DEFAULT_SHELL));
    }

    #[test]
    fn discover_shell_public_explicit_wins_regardless_of_environment() {
        // The public entry point must honor an explicit absolute existing
        // path no matter what $SHELL or the passwd database contain.
        let explicit = scratch_file("public-explicit-shell");
        assert_eq!(CommandBuilder::discover_shell(Some(&explicit)), explicit);
        let _ = std::fs::remove_file(&explicit);
    }

    #[test]
    fn discover_shell_public_never_empty() {
        let resolved = CommandBuilder::discover_shell(None);
        assert!(!resolved.as_os_str().is_empty());
        assert!(resolved.is_absolute());
    }

    #[test]
    fn build_envs_clear_envs_overlay_and_term_default() {
        let envs = CommandBuilder::new("/bin/sh")
            .clear_envs(true)
            .env("FOO", "bar")
            .build_envs();
        assert_eq!(envs.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(envs.get("TERM").map(String::as_str), Some(DEFAULT_TERM));
        assert_eq!(envs.len(), 2);
    }

    #[test]
    fn build_envs_explicit_term_overrides_overlay() {
        let envs = CommandBuilder::new("/bin/sh")
            .clear_envs(true)
            .env("TERM", "vt100")
            .term("xterm-88color")
            .build_envs();
        assert_eq!(envs.get("TERM").map(String::as_str), Some("xterm-88color"));
    }

    #[test]
    fn build_envs_overlay_term_kept_without_explicit_term() {
        let envs = CommandBuilder::new("/bin/sh")
            .clear_envs(true)
            .env("TERM", "vt100")
            .build_envs();
        assert_eq!(envs.get("TERM").map(String::as_str), Some("vt100"));
    }

    #[test]
    fn build_envs_colorterm_injected_only_when_configured() {
        let without = CommandBuilder::new("/bin/sh").clear_envs(true).build_envs();
        assert!(!without.contains_key("COLORTERM"));

        let with = CommandBuilder::new("/bin/sh")
            .clear_envs(true)
            .colorterm("truecolor")
            .build_envs();
        assert_eq!(with.get("COLORTERM").map(String::as_str), Some("truecolor"));

        let via_opt = CommandBuilder::new("/bin/sh")
            .clear_envs(true)
            .colorterm_opt(Some("truecolor"))
            .build_envs();
        assert_eq!(
            via_opt.get("COLORTERM").map(String::as_str),
            Some("truecolor")
        );
    }

    #[test]
    fn build_envs_inherits_parent_when_not_cleared() {
        let envs = CommandBuilder::new("/bin/sh").build_envs();
        // Every parent entry must be present and unchanged.
        for (key, value) in std::env::vars() {
            assert_eq!(envs.get(&key), Some(&value));
        }
        // TERM is always injected.
        assert!(envs.contains_key("TERM"));
    }

    #[test]
    fn build_envs_overlay_overrides_parent() {
        let envs = CommandBuilder::new("/bin/sh")
            .env("MR_CRABS_PTY_TEST_KEY", "overlay-wins")
            .build_envs();
        assert_eq!(
            envs.get("MR_CRABS_PTY_TEST_KEY").map(String::as_str),
            Some("overlay-wins")
        );
    }

    #[test]
    fn env_remove_strips_overlay_entry() {
        let envs = CommandBuilder::new("/bin/sh")
            .clear_envs(true)
            .env("FOO", "bar")
            .env_remove("FOO")
            .build_envs();
        assert!(!envs.contains_key("FOO"));
    }

    #[test]
    fn args_and_envs_accumulate() {
        let mut builder = CommandBuilder::new("/bin/sh");
        builder
            .arg("-c")
            .args(["echo", "hi"])
            .envs([("A", "1"), ("B", "2")]);
        assert_eq!(
            builder.args,
            vec![
                OsString::from("-c"),
                OsString::from("echo"),
                OsString::from("hi")
            ]
        );
        assert_eq!(builder.envs_overlay().len(), 2);
    }

    #[test]
    fn to_spawn_command_materializes_all_fields() {
        let mut builder = CommandBuilder::new("/bin/sh");
        builder
            .args(["-c", "true"])
            .cwd("/tmp")
            .clear_envs(true)
            .env("FOO", "bar")
            .term("xterm-88color")
            .colorterm("truecolor");
        let cmd = builder.to_spawn_command();
        assert_eq!(cmd.exe, PathBuf::from("/bin/sh"));
        assert_eq!(cmd.args, vec![OsString::from("-c"), OsString::from("true")]);
        assert_eq!(cmd.cwd, Some(PathBuf::from("/tmp")));
        assert_eq!(cmd.term, "xterm-88color");
        assert_eq!(cmd.colorterm.as_deref(), Some("truecolor"));
        // SpawnCommand stays consistent with its own env map.
        assert_eq!(
            cmd.envs.get("TERM").map(String::as_str),
            Some(cmd.term.as_str())
        );
        assert_eq!(
            cmd.envs.get("COLORTERM").map(String::as_str),
            cmd.colorterm.as_deref()
        );
        assert_eq!(cmd.envs.get("FOO").map(String::as_str), Some("bar"));
    }

    #[test]
    fn identical_builders_produce_identical_results() {
        let mut a = CommandBuilder::new("/bin/zsh");
        a.args(["-l"]).env("X", "y").term("xterm-256color");
        let mut b = CommandBuilder::new("/bin/zsh");
        b.args(["-l"]).env("X", "y").term("xterm-256color");
        assert_eq!(a.build_envs(), b.build_envs());
        assert_eq!(a.to_spawn_command(), b.to_spawn_command());
    }
    #[test]
    fn argv_preserves_non_utf8_bytes_through_spawn_command() {
        use std::os::unix::ffi::OsStringExt;
        let raw = vec![0x66, 0x6f, 0x80, 0x6f]; // "fo\x80o"
        let non_utf8 = OsString::from_vec(raw.clone());
        let mut builder = CommandBuilder::new("/bin/sh");
        builder.arg(non_utf8.clone());
        let cmd = builder.to_spawn_command();
        assert_eq!(cmd.args.len(), 1);
        assert_eq!(cmd.args[0].as_bytes(), raw.as_slice());
        assert_eq!(cmd.args[0], non_utf8);
    }

    #[test]
    fn argv_round_trip_non_utf8_via_os_bytes() {
        use std::os::unix::ffi::OsStringExt;
        let bytes = vec![0xFF, 0xFE, 0xFD];
        let arg = OsString::from_vec(bytes.clone());
        let mut b = CommandBuilder::new("/bin/echo");
        b.args([arg.clone(), OsString::from("ok")]);
        let cmd = b.to_spawn_command();
        assert_eq!(cmd.args[0].as_bytes(), bytes.as_slice());
        assert_eq!(cmd.args[1], OsString::from("ok"));
    }
}
