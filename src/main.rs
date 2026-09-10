// jj-workspace: a Herdr plugin to create/remove Jujutsu (jj) workspaces,
// mirroring Herdr's own git-worktree flow and dialog.
//
// One binary, dispatched by subcommand (set in herdr-plugin.toml):
//   open <workspace|tab>  action: capture the caller, open the wizard pane
//   wizard                pane:   select a source + name, create the two-pane workspace
//   remove                action: `jj workspace forget` + delete dir + close tab
//
// The wizard renders the actual "new worktree" modal using the same TUI stack as
// Herdr (ratatui + crossterm), ported from herdr's src/ui/dialogs.rs and
// src/ui/widgets.rs so it looks and behaves like the built-in dialog.

use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{self, Command};
use std::thread;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    symbols,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame, Terminal,
};
use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Debug)]
struct WorkspaceChoice {
    id: String,
    label: String,
    path: String,
}

struct WizardResult {
    source: WorkspaceChoice,
    name: String,
    /// The base revision for workspace creation: the resolution-chain value
    /// evaluated for the finally selected source, or the user-edited revset
    /// (validated) when the base field was touched.
    base_rev: String,
}

#[derive(Clone, Copy)]
struct WizardView<'a> {
    choices: &'a [WorkspaceChoice],
    filtered: &'a [usize],
    selected: usize,
    field: WizardField,
    query: &'a str,
    name: &'a str,
    base: &'a str,
    root: &'a Path,
    error: Option<&'a str>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum WizardField {
    WorkspaceSearch,
    Name,
    Base,
}

/// Tab/BackTab cycle order of the wizard's editable fields.
fn next_wizard_field(field: WizardField) -> WizardField {
    match field {
        WizardField::WorkspaceSearch => WizardField::Name,
        WizardField::Name => WizardField::Base,
        WizardField::Base => WizardField::WorkspaceSearch,
    }
}

/// Name-field edit state: component-level operations apply only at anchor
/// states. `Fresh` means the name is still the auto-generated default
/// `workspace/<slug>`; `Prefixed` means the slug was dropped and only the
/// prefix remains; `Free` is per-character editing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum NameEditState {
    Fresh,
    Prefixed,
    Free,
}

/// A name-field keypress reducible through `apply_name_key`.
enum NameKey {
    Char(char),
    Backspace,
}

/// Apply a keypress to the name field (component-level state machine):
/// - Fresh + Char    → keep prefix, replace slug, then Free
/// - Fresh + Bksp    → drop slug, keep prefix (→ Prefixed)
/// - Prefixed + Char → append after prefix (→ Free)
/// - Prefixed + Bksp → clear whole name (→ Free)
/// - Free + Char     → append
/// - Free + Bksp     → pop one char
///
/// The prefix is derived from the current name via the last `/`, never
/// hardcoded, so user-typed multi-segment names edit per-character.
fn apply_name_key(name: &mut String, state: &mut NameEditState, key: NameKey) {
    match key {
        NameKey::Char(c) => {
            if *state == NameEditState::Fresh {
                // Replace the slug part, keep the prefix (everything up to
                // and including the last '/').
                match name.rfind('/') {
                    Some(slash) => name.truncate(slash + 1),
                    None => name.clear(),
                }
            }
            name.push(c);
            *state = NameEditState::Free;
        }
        NameKey::Backspace => match *state {
            NameEditState::Fresh => match name.rfind('/') {
                Some(slash) => {
                    name.truncate(slash + 1);
                    *state = NameEditState::Prefixed;
                }
                None => {
                    name.clear();
                    *state = NameEditState::Free;
                }
            },
            NameEditState::Prefixed => {
                name.clear();
                *state = NameEditState::Free;
            }
            NameEditState::Free => {
                name.pop();
            }
        },
    }
}

// Default repo-relative paths materialized in each new sparse checkout so
// the coding agent finds its startup instructions before the right pane's
// full materialization lands seconds later. Union of the startup-time
// (synchronous) reads of 15 mainstream coding agents, per official docs —
// evidence matrix and exclusion list in design.md of the
// agent-agnostic-bootstrap-paths openspec change.
// Paths missing from a repo are silently skipped by jj sparse and stay in
// the pattern list (auto-materialized if the repo later gains a match).
const DEFAULT_BOOTSTRAP_PATHS: [&str; 34] = [
    // Root instruction files.
    "AGENTS.md",
    "AGENT.md",
    "AGENTS.override.md",
    "CLAUDE.md",
    "CLAUDE.local.md",
    "GEMINI.md",
    "QWEN.md",
    "CRUSH.md",
    // Root tool-specific files.
    ".mcp.json",
    "opencode.json",
    "opencode.jsonc",
    ".cursorrules",
    ".windsurfrules",
    ".goosehints",
    ".augment-guidelines",
    ".github/copilot-instructions.md",
    // Directories.
    ".agents",
    ".claude",
    ".codex",
    ".cursor",
    ".gemini",
    ".qwen",
    ".opencode",
    ".windsurf",
    ".devin",
    ".clinerules",
    ".cline",
    ".kilo",
    ".kilocode",
    ".augment",
    ".continue",
    ".github/instructions",
    ".crush",
    ".goose",
];

// --- plugin config ---------------------------------------------------------
//
// Single config channel: `$HERDR_PLUGIN_CONFIG_DIR/config.toml`. herdr injects
// HERDR_PLUGIN_CONFIG_DIR and guarantees the directory exists before every
// plugin command/pane spawn, so the path is always read from that env var and
// never hardcoded. Process env vars and `.env` are NOT config sources. A
// missing file means all built-in defaults; any other problem (syntax error,
// wrong type, unknown key, empty string value) is a hard fail-fast error.

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct Config {
    jj: JjConfig,
    agent: AgentConfig,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct JjConfig {
    base_rev: String,
    workspace_root: String,
    /// The jj executable: a bare name (looked up on PATH) or an absolute path,
    /// or a full argv whose first element is the executable.
    command: JjCommandValue,
}

/// Value of `jj.command`: either a single string — a bare name or an absolute
/// (or `~`-prefixed) path, classified by `resolve_jj_command` — or a full argv
/// list whose first element is the executable and whose remaining elements are
/// leading arguments prepended to every jj invocation.
#[derive(Debug, Clone, PartialEq)]
enum JjCommandValue {
    Single(String),
    Argv(Vec<String>),
}

impl Default for JjCommandValue {
    fn default() -> Self {
        JjCommandValue::Single("jj".into())
    }
}

impl<'de> Deserialize<'de> for JjCommandValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct JjCommandVisitor;

        impl<'de> serde::de::Visitor<'de> for JjCommandVisitor {
            type Value = JjCommandValue;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(formatter, "a string or a list of strings for `jj.command`")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(JjCommandValue::Single(value.to_string()))
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element::<String>()? {
                    items.push(item);
                }
                Ok(JjCommandValue::Argv(items))
            }
        }

        deserializer.deserialize_any(JjCommandVisitor)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct AgentConfig {
    command: String,
    bootstrap_paths: Vec<String>,
    /// Additive extension to `bootstrap_paths`: entries are appended to the
    /// resolved base list (explicit value, or the 34-item built-in default),
    /// order-preserving deduplicated. Absent key = empty vec, so old configs
    /// keep parsing byte-for-byte identically.
    extend_bootstrap_paths: Vec<String>,
    /// `false` (default): never auto-answer any agent prompt; a blocked agent
    /// is surfaced as a toast. `true`: auto-press Enter ONLY for a codex
    /// trust prompt inside the startup window (see `trust_window_secs`).
    auto_trust: bool,
    /// Startup window (seconds, from finish-tab start) during which codex
    /// trust prompts may be auto-answered when `auto_trust` is on.
    trust_window_secs: u64,
    /// Total wait budget (seconds) for the agent to be detected.
    startup_timeout_secs: u64,
    /// `herdr agent list` polling interval (milliseconds).
    poll_interval_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            jj: JjConfig::default(),
            agent: AgentConfig::default(),
        }
    }
}

impl Default for JjConfig {
    fn default() -> Self {
        JjConfig {
            base_rev: "trunk()".into(),
            workspace_root: "~/.herdr/workspaces".into(),
            command: JjCommandValue::default(),
        }
    }
}

impl Default for AgentConfig {
    fn default() -> Self {
        AgentConfig {
            command: "opencode".into(),
            bootstrap_paths: DEFAULT_BOOTSTRAP_PATHS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            extend_bootstrap_paths: Vec::new(),
            auto_trust: false,
            trust_window_secs: 10,
            startup_timeout_secs: 20,
            poll_interval_ms: 200,
        }
    }
}

impl AgentConfig {
    /// The effective materialization list: the resolved `bootstrap_paths`
    /// (explicit value, or the 34-item built-in default) followed by
    /// `extend_bootstrap_paths` entries, deduplicated order-preserving —
    /// base items keep their position, already-present extend entries are
    /// skipped, and the remaining extend entries append in order.
    fn effective_bootstrap_paths(&self) -> Vec<String> {
        let mut effective = Vec::with_capacity(
            self.bootstrap_paths.len() + self.extend_bootstrap_paths.len(),
        );
        for path in self
            .bootstrap_paths
            .iter()
            .chain(self.extend_bootstrap_paths.iter())
        {
            if !effective.contains(path) {
                effective.push(path.clone());
            }
        }
        effective
    }
}

/// Why `load_config` failed. Every variant carries a user-facing message that
/// names the offending key or file.
#[derive(Debug)]
enum ConfigError {
    /// The config directory/file exists but could not be read.
    Io(String),
    /// TOML syntax error, type error, or unknown key (serde reports the key).
    Parse(String),
    /// An explicitly empty string value, which is invalid config.
    Empty(String),
    /// A numeric value below its required minimum.
    BelowMinimum {
        key: String,
        minimum: u64,
        actual: u64,
    },
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Io(message) => write!(f, "could not read config: {message}"),
            ConfigError::Parse(message) => write!(f, "invalid config: {message}"),
            ConfigError::Empty(key) => write!(f, "invalid config: `{key}` must not be empty"),
            ConfigError::BelowMinimum {
                key,
                minimum,
                actual,
            } => write!(
                f,
                "invalid config: `{key}` must be >= {minimum} (got {actual})"
            ),
        }
    }
}

/// Load and validate the plugin config. A missing `config.toml` is not an
/// error: it yields all built-in defaults. Anything else malformed is a hard
/// error (fail-fast).
fn load_config() -> Result<Config, ConfigError> {
    let Some(dir) = env::var("HERDR_PLUGIN_CONFIG_DIR").ok() else {
        return Ok(Config::default());
    };
    load_config_from(&Path::new(&dir).join("config.toml"))
}

fn load_config_from(path: &Path) -> Result<Config, ConfigError> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(err) => return Err(ConfigError::Io(err.to_string())),
    };
    let config: Config = toml::from_str(&content)
        .map_err(|err| ConfigError::Parse(err.to_string()))?;
    config.validate()
}

impl Config {
    /// Reject explicitly empty string values. This is a BREAKING change from
    /// the old `config_value` semantics, where an empty value meant "use the
    /// default".
    fn validate(self) -> Result<Config, ConfigError> {
        if self.jj.base_rev.is_empty() {
            return Err(ConfigError::Empty("jj.base_rev".into()));
        }
        if self.jj.workspace_root.is_empty() {
            return Err(ConfigError::Empty("jj.workspace_root".into()));
        }
        if self.agent.command.is_empty() {
            return Err(ConfigError::Empty("agent.command".into()));
        }
        match &self.jj.command {
            JjCommandValue::Single(value) if value.is_empty() => {
                return Err(ConfigError::Empty("jj.command".into()));
            }
            JjCommandValue::Argv(items) if items.is_empty() => {
                return Err(ConfigError::Empty("jj.command".into()));
            }
            JjCommandValue::Argv(items) if items[0].is_empty() => {
                return Err(ConfigError::Empty("jj.command (argv[0])".into()));
            }
            _ => {}
        }
        for (index, path) in self.agent.bootstrap_paths.iter().enumerate() {
            if path.is_empty() {
                return Err(ConfigError::Empty(format!("agent.bootstrap_paths[{index}]")));
            }
        }
        for (index, path) in self.agent.extend_bootstrap_paths.iter().enumerate() {
            if path.is_empty() {
                return Err(ConfigError::Empty(format!(
                    "agent.extend_bootstrap_paths[{index}]"
                )));
            }
        }
        for (key, value, minimum) in [
            ("agent.trust_window_secs", self.agent.trust_window_secs, 1),
            (
                "agent.startup_timeout_secs",
                self.agent.startup_timeout_secs,
                1,
            ),
            ("agent.poll_interval_ms", self.agent.poll_interval_ms, 10),
        ] {
            if value < minimum {
                return Err(ConfigError::BelowMinimum {
                    key: key.into(),
                    minimum,
                    actual: value,
                });
            }
        }
        Ok(self)
    }
}

// --- jj executable resolution ----------------------------------------------

/// The fully-resolved `jj` invocation: an absolute executable path validated
/// to exist and carry the executable bit, plus any leading arguments from an
/// argv-form `jj.command`. Resolution happens exactly once per plugin
/// invocation and the result is threaded through every jj call, including the
/// right-pane setup command (so the pane shell's PATH never participates).
#[derive(Debug, Clone, PartialEq)]
struct ResolvedJj {
    executable: PathBuf,
    extra_args: Vec<String>,
}

/// Why `jj.command` could not be resolved. `origin` is `"jj.command"` for the
/// string form and `"jj.command argv[0]"` for the argv form.
#[derive(Debug)]
enum ResolveError {
    /// A bare name that matched no executable in any PATH directory.
    NotOnPath {
        origin: &'static str,
        value: String,
        searched: Vec<PathBuf>,
    },
    /// An absolute path that does not exist, is not a regular file, or lacks
    /// the executable bit.
    BadAbsolutePath {
        origin: &'static str,
        path: String,
        reason: &'static str,
    },
    /// A value containing `/` that is neither absolute nor `~`-expanded to an
    /// absolute path.
    RelativePath { origin: &'static str, value: String },
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::NotOnPath {
                origin,
                value,
                searched,
            } => {
                let dirs = searched
                    .iter()
                    .map(|dir| dir.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(
                    f,
                    "{origin}: `{value}` was not found on PATH (searched: {dirs}). \
                     Run `which jj` in your shell and write the result into `jj.command` in config.toml"
                )
            }
            ResolveError::BadAbsolutePath { origin, path, reason } => {
                write!(f, "{origin}: {path} {reason} (expected an executable file)")
            }
            ResolveError::RelativePath { origin, value } => write!(
                f,
                "{origin}: `{value}` is a relative path; only a bare name (looked up on PATH) \
                 or an absolute path is supported"
            ),
        }
    }
}

fn path_dirs() -> Vec<PathBuf> {
    env::var_os("PATH")
        .map(|paths| {
            env::split_paths(&paths)
                .filter(|dir| !dir.as_os_str().is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Resolve `jj.command` once per process. A string form classifies as a bare
/// name (PATH lookup) or an absolute path (validated as-is); an argv form
/// resolves only `argv[0]` and keeps the rest as leading arguments. `~` is
/// expanded before classification. PATH directories come from the plugin
/// process environment; tests inject their own directory list instead.
fn resolve_jj_command(
    value: &JjCommandValue,
    path_dirs: &[PathBuf],
) -> Result<ResolvedJj, ResolveError> {
    let (head, extra_args, origin) = match value {
        JjCommandValue::Single(value) => (value.clone(), Vec::new(), "jj.command"),
        JjCommandValue::Argv(items) => (items[0].clone(), items[1..].to_vec(), "jj.command argv[0]"),
    };
    let home = env::var("HOME").ok();
    resolve_jj_head(&head, extra_args, origin, home.as_deref(), path_dirs)
}

fn resolve_jj_head(
    head: &str,
    extra_args: Vec<String>,
    origin: &'static str,
    home: Option<&str>,
    path_dirs: &[PathBuf],
) -> Result<ResolvedJj, ResolveError> {
    let head = expand_tilde_with_home(head, home);
    if head.contains('/') {
        if !head.starts_with('/') {
            return Err(ResolveError::RelativePath {
                origin,
                value: head,
            });
        }
        return match fs::metadata(&head) {
            Ok(meta) if meta.is_file() && is_executable(&meta) => Ok(ResolvedJj {
                executable: PathBuf::from(&head),
                extra_args,
            }),
            Ok(_) => Err(ResolveError::BadAbsolutePath {
                origin,
                path: head,
                reason: "is not an executable file",
            }),
            Err(_) => Err(ResolveError::BadAbsolutePath {
                origin,
                path: head,
                reason: "does not exist",
            }),
        };
    }
    for dir in path_dirs {
        let candidate = dir.join(&head);
        if is_executable_file(&candidate) {
            return Ok(ResolvedJj {
                executable: candidate,
                extra_args,
            });
        }
    }
    Err(ResolveError::NotOnPath {
        origin,
        value: head,
        searched: path_dirs.to_vec(),
    })
}

fn is_executable(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

fn is_executable_file(path: &Path) -> bool {
    fs::metadata(path)
        .map(|meta| meta.is_file() && is_executable(&meta))
        .unwrap_or(false)
}

fn main() {
    let args: Vec<String> = env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("open") => cmd_open(args.get(2).map(String::as_str).unwrap_or("workspace")),
        Some("wizard") => cmd_wizard(),
        Some("finish-tab") => cmd_finish_tab(&args),
        Some("remove") => cmd_remove(),
        other => {
            eprintln!("usage: jj-workspace <open [workspace|tab] | wizard | remove>");
            eprintln!("got: {other:?}");
            process::exit(2);
        }
    }
}

// --- base revision resolution ----------------------------------------------

/// Resolve the base revision for workspace creation. The chain: `jj config get
/// herdr.base-rev` (repo-level config wins; user-level config acts as a
/// personal global default) → `jj.base_rev` in config.toml → the built-in
/// `trunk()`. A missing key (`jj config get` exits non-zero) is never an
/// error — it falls through to the next layer. An explicitly EMPTY value
/// (exit 0, empty output) is invalid config and fails fast, mirroring the
/// config.toml layer's empty-value semantics.
fn resolve_base_rev(config: &Config, jj: &ResolvedJj, repo: &Path) -> Result<String, String> {
    let mut command = Command::new(&jj.executable);
    command
        .current_dir(repo)
        .args(&jj.extra_args)
        .args(["config", "get", "herdr.base-rev"]);
    if let Ok(output) = command.output() {
        if output.status.success() {
            let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if value.is_empty() {
                return Err(
                    "jj config reports `herdr.base-rev` as empty — empty values are invalid. \
                     Set it to a revset (`jj config set --repo herdr.base-rev <revset>`) \
                     or remove the key to fall back to config.toml / trunk()."
                        .into(),
                );
            }
            return Ok(value);
        }
    }
    Ok(config.jj.base_rev.clone())
}

/// Determine the final base revision at wizard submit time for a jj source:
/// an untouched base field (dirty = false) is re-evaluated from the
/// resolution chain so the *finally selected* source wins; a user-edited
/// value (dirty = true) is validated against the repo with
/// `jj log -r <expr>` (a cheap parse-only check with zero output). Failure
/// keeps the wizard open with jj's own error message.
fn wizard_final_base_rev(
    config: &Config,
    jj: &ResolvedJj,
    repo: &Path,
    entered: &str,
    dirty: bool,
) -> Result<String, String> {
    // The FINAL base value is pre-validated regardless of its source:
    // chain-resolved values come from the user's jj config and can reference
    // nonexistent revisions (falsified during acceptance: `herdr.base-rev =
    // <nonexistent change id>` used to slip through and fail only at
    // `jj workspace add`). An invalid value keeps the wizard open so the
    // user can edit the base field.
    let value = if dirty {
        entered.to_string()
    } else {
        resolve_base_rev(config, jj, repo)?
    };
    validate_revset(jj, repo, &value)?;
    Ok(value)
}

/// Pre-validate a revset against the source repo: `jj log -r <value>` parses
/// and resolves it; `--limit 0` keeps it fast and pager-free. Failure carries
/// jj's native message into the wizard error line.
fn validate_revset(jj: &ResolvedJj, repo: &Path, value: &str) -> Result<(), String> {
    let mut validate = Command::new(&jj.executable);
    validate
        .current_dir(repo)
        .args(&jj.extra_args)
        .args(["log", "-r", value, "--no-graph", "--limit", "0", "--no-pager"]);
    match validate.output() {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(if stderr.is_empty() {
                format!("invalid revset: {value}")
            } else {
                stderr
            })
        }
        Err(err) => Err(format!("could not validate revset: {err}")),
    }
}

/// Action (headless): capture the calling workspace, then open the wizard pane.
fn cmd_open(_mode: &str) -> ! {
    let config = load_config().unwrap_or_else(|err| die(&err.to_string()));
    // Resolve eagerly so a broken `jj.command` fails the action (and surfaces
    // as a toast) instead of surfacing only inside the wizard pane.
    if let Err(err) = resolve_jj_command(&config.jj.command, &path_dirs()) {
        die(&err.to_string());
    }
    let ctx = env::var("HERDR_PLUGIN_CONTEXT_JSON").unwrap_or_default();
    let workspace_id = env::var("HERDR_WORKSPACE_ID")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| json_string_field(&ctx, "workspace_id"))
        .unwrap_or_default();

    let mut cmd = Command::new(herdr_bin());
    cmd.args([
        "plugin",
        "pane",
        "open",
        "--plugin",
        &plugin_id(),
        "--entrypoint",
        "wizard",
    ])
    .arg("--env")
    .arg(format!("CURRENT_HERDR_WORKSPACE_ID={workspace_id}"))
    .arg("--focus");
    match cmd.status() {
        Ok(status) => process::exit(status.code().unwrap_or(0)),
        Err(err) => {
            eprintln!("error: failed to open wizard pane: {err}");
            process::exit(1);
        }
    }
}

/// Pane (interactive TTY): select a source workspace and name, then create the
/// agent-left / terminal-right Herdr workspace.
fn cmd_wizard() -> ! {
    let config = match load_config() {
        Ok(config) => config,
        Err(err) => show_config_error_and_exit(&err.to_string()),
    };
    // Resolve `jj.command` exactly once per invocation; the result is used for
    // every jj call below and baked (absolute) into the right-pane setup
    // command, so the pane shell's PATH never participates.
    let jj = match resolve_jj_command(&config.jj.command, &path_dirs()) {
        Ok(jj) => jj,
        Err(err) => show_config_error_and_exit(&err.to_string()),
    };
    let current_workspace = env::var("CURRENT_HERDR_WORKSPACE_ID").unwrap_or_default();
    let choices = match load_workspace_choices() {
        Ok(choices) => choices,
        Err(err) => fail(&err),
    };
    let selected = choices
        .iter()
        .position(|choice| choice.id == current_workspace)
        .unwrap_or(0);
    let root = workspaces_root(&config);

    // Prefill the wizard's base field from the initially selected source. The
    // value shown is display-only: at submit, an untouched field is
    // re-evaluated from the resolution chain so the finally selected source
    // wins (see `wizard_final_base_rev`). With no jj candidates the field
    // simply shows the config default.
    let initial_base = match choices.get(selected) {
        Some(initial_choice) => {
            // Display-only prefill: a repo-level resolution error (e.g. an
            // explicit empty `herdr.base-rev` in jj repo config) must NOT kill
            // the wizard at entry — fall back to the global default here. The
            // real resolution happens at submit (wizard_final_base_rev), where
            // errors surface as the wizard's own error line and the user can
            // edit the base field to proceed.
            resolve_base_rev(&config, &jj, Path::new(&repo_root(&initial_choice.path)))
                .unwrap_or_else(|_| config.jj.base_rev.clone())
        }
        None => config.jj.base_rev.clone(),
    };

    let selection = match run_workspace_wizard(
        &choices,
        selected,
        &root,
        &config,
        &jj,
        generated_name(seed()),
        initial_base,
    ) {
        Ok(Some(selection)) => selection,
        Ok(None) => process::exit(0),
        Err(err) => fail(&format!("terminal error: {err}")),
    };
    let source = selection.source.path.trim_end_matches('/').to_string();
    if source.is_empty() || !Path::new(&source).is_dir() {
        fail(&format!("workspace folder does not exist: {source}"));
    }

    // `jj.command` was already resolved and validated at wizard entry.
    // Resolve secondary workspaces to the main repo so sibling checkouts
    // remain grouped under a stable directory.
    let repo = repo_root(&source);
    let dest_path = root
        .join(basename(&repo))
        .join(branch_to_path_slug(&selection.name));
    if dest_path.exists() {
        fail(&format!("checkout already exists: {}", dest_path.display()));
    }
    if let Some(parent) = dest_path.parent() {
        if let Err(err) = fs::create_dir_all(parent) {
            fail(&format!("could not create {}: {err}", parent.display()));
        }
    }
    let dest = dest_path.display().to_string();

    // Create only the metadata and bootstrap startup files synchronously.
    // The full checkout, bookmark, fetch, and rebase run in the right pane.
    // The base revision comes from the wizard (resolution chain evaluated
    // for this source, or the validated user input) so `workspace add -r`
    // and the right-pane rebase destination always agree.
    let base = &selection.base_rev;
    eprintln!(
        "+ {} workspace add --name {} -r {base} --sparse-patterns empty {dest}",
        jj.executable.display(),
        selection.name
    );
    let mut add = Command::new(&jj.executable);
    add.current_dir(&repo)
        .args(&jj.extra_args)
        .args([
            "workspace",
            "add",
            "--name",
            &selection.name,
            "-r",
            base,
            "--sparse-patterns",
            "empty",
            &dest,
        ]);
    run_or(add, "jj workspace add", fail);

    let mut bootstrap = Command::new(&jj.executable);
    bootstrap
        .current_dir(&dest)
        .args(&jj.extra_args)
        .args(["sparse", "set", "--clear"]);
    for path in config.agent.effective_bootstrap_paths() {
        bootstrap.arg("--add").arg(path);
    }
    run_or(bootstrap, "materialize agent bootstrap files", fail);

    open_tab_layout(
        &config,
        &jj,
        &selection.source.id,
        &dest,
        &selection.name,
        &selection.base_rev,
    );
    process::exit(0);
}

/// Fail-fast: render an error in a minimal TUI screen, then exit non-zero.
/// The wizard runs as an interactive pane (TTY), so the error must be visible
/// there rather than only on stderr.
fn show_config_error_and_exit(message: &str) -> ! {
    // Fatal errors must reach error.log even when their UI outlet (this
    // modal) is dismissed instantly.
    log_error(message);
    let _ = enable_raw_mode();
    let mut out = io::stdout();
    let _ = execute!(out, EnterAlternateScreen);
    let mut terminal = match Terminal::new(CrosstermBackend::new(out)) {
        Ok(terminal) => terminal,
        Err(_) => {
            let _ = disable_raw_mode();
            fail(&message);
        }
    };
    let _ = terminal.draw(|frame| {
        let p = catppuccin();
        let area = frame.area();
        dim_background(frame, area);
        if let Some(inner) = render_modal_shell(frame, area, 90, 12, &p) {
            render_modal_header(
                frame,
                Rect::new(inner.x, inner.y, inner.width, 1),
                "configuration error",
                &p,
            );
            let body = Paragraph::new(vec![
                Line::from(Span::styled(message, Style::default().fg(p.red))),
                Line::from(""),
                Line::from(Span::styled(
                    "press enter to close",
                    Style::default().fg(p.overlay0),
                )),
            ])
            .wrap(Wrap { trim: false });
            frame.render_widget(
                body,
                Rect::new(
                    inner.x + 1,
                    inner.y + 1,
                    inner.width.saturating_sub(2),
                    inner.height.saturating_sub(2),
                ),
            );
        }
    });
    let _ = event::read();
    let _ = restore_terminal(&mut terminal);
    process::exit(1);
}

fn load_workspace_choices() -> Result<Vec<WorkspaceChoice>, String> {
    load_workspace_choices_with(&herdr_bin())
}

/// Like `load_workspace_choices`, with an injectable herdr executable (tests
/// pass a fake). Candidate paths are jj workspace roots only: herdr's
/// `checkout_path` already names the workspace root, while the pane-cwd
/// fallback may sit anywhere inside the repository and is normalized upward
/// by `jj_root`; a workspace with no `.jj` marker on any ancestor is not a
/// jj workspace and is filtered out.
fn load_workspace_choices_with(bin: &str) -> Result<Vec<WorkspaceChoice>, String> {
    let workspaces = herdr_json_with(bin, &["workspace", "list"])?;
    let panes = herdr_json_with(bin, &["pane", "list"])?;
    let workspace_values = workspaces
        .pointer("/result/workspaces")
        .and_then(Value::as_array)
        .ok_or_else(|| "Herdr returned an invalid workspace list".to_string())?;
    let pane_values = panes
        .pointer("/result/panes")
        .and_then(Value::as_array)
        .ok_or_else(|| "Herdr returned an invalid pane list".to_string())?;

    let mut choices = Vec::new();
    for workspace in workspace_values {
        let Some(id) = workspace.get("workspace_id").and_then(Value::as_str) else {
            continue;
        };
        let label = workspace
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or(id)
            .to_string();
        let active_tab = workspace
            .get("active_tab_id")
            .and_then(Value::as_str)
            .unwrap_or_default();

        let path = if let Some(path) = workspace
            .pointer("/worktree/checkout_path")
            .and_then(Value::as_str)
            .filter(|path| !path.is_empty())
        {
            Some(path.to_string())
        } else {
            let active_panes: Vec<&Value> = pane_values
                .iter()
                .filter(|pane| {
                    pane.get("workspace_id").and_then(Value::as_str) == Some(id)
                        && pane.get("tab_id").and_then(Value::as_str) == Some(active_tab)
                })
                .collect();
            active_panes
                .iter()
                .copied()
                .find(|pane| pane.get("focused").and_then(Value::as_bool) == Some(true))
                .or_else(|| active_panes.first().copied())
                .and_then(pane_path)
        };

        // jj-only filter + ancestor normalization: keep the candidate only
        // when the path itself or one of its ancestors is a jj workspace
        // root, and replace the path by that root. A pane cwd deep inside the
        // repository therefore still resolves to the workspace root the new
        // tab is created on; non-jj projects never appear as candidates.
        if let Some(path) = path.and_then(|path| jj_root(&path)) {
            choices.push(WorkspaceChoice {
                id: id.into(),
                label,
                path,
            });
        }
    }
    Ok(choices)
}

fn pane_path(pane: &Value) -> Option<String> {
    pane.get("foreground_cwd")
        .and_then(Value::as_str)
        .filter(|path| !path.is_empty())
        .or_else(|| pane.get("cwd").and_then(Value::as_str))
        .map(str::to_string)
}

fn herdr_json_with(bin: &str, args: &[&str]) -> Result<Value, String> {
    let output = Command::new(bin)
        .args(args)
        .output()
        .map_err(|err| format!("herdr {} failed to start: {err}", args.join(" ")))?;
    io::stderr().write_all(&output.stderr).ok();
    if !output.status.success() {
        return Err(format!(
            "herdr {} failed (exit {})",
            args.join(" "),
            output.status.code().unwrap_or(-1)
        ));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|err| format!("invalid JSON from herdr {}: {err}", args.join(" ")))
}

/// Resolve the left-pane agent start command from `agent.command`. Empty
/// values are rejected at config-parse time, so this is always the configured
/// command or the built-in default.
fn resolve_start_command(agent: &AgentConfig) -> String {
    agent.command.clone()
}

fn open_tab_layout(
    config: &Config,
    jj: &ResolvedJj,
    workspace_id: &str,
    cwd: &str,
    label: &str,
    base_rev: &str,
) {
    let herdr = herdr_bin();
    eprintln!("+ herdr tab create --workspace {workspace_id} --cwd {cwd}");
    let created = command_json(
        Command::new(&herdr).args([
            "tab",
            "create",
            "--workspace",
            workspace_id,
            "--cwd",
            cwd,
            "--label",
            label,
            "--focus",
        ]),
        "herdr tab create",
    );
    let tab_id = required_json_string(&created, "/result/tab/tab_id");
    let left_pane = required_json_string(&created, "/result/root_pane/pane_id");

    let split = command_json(
        Command::new(&herdr).args([
            "pane",
            "split",
            "--pane",
            &left_pane,
            "--direction",
            "right",
            "--ratio",
            "0.5",
            "--cwd",
            cwd,
            "--no-focus",
        ]),
        "herdr pane split",
    );
    let right_pane = required_json_string(&split, "/result/pane/pane_id");
    let right_command = setup_script_command(
        &setup_script_path(),
        jj,
        base_rev,
        label,
        workspace_id,
        &tab_id,
        &left_pane,
    );
    let mut run_right = Command::new(&herdr);
    run_right.args(["pane", "run", &right_pane, &right_command]);
    run_or(run_right, "start right-pane setup", fail);

    // Give checkout materialization a head start, then launch the coding
    // agent without changing focus away from the left pane. The start command
    // comes from `agent.command` (default "opencode"): panes run the user's
    // shell, where agent launch aliases vary between setups and cannot be
    // assumed.
    let start_command = resolve_start_command(&config.agent);
    let mut start_agent = Command::new(&herdr);
    start_agent.args(["pane", "run", &left_pane, &start_command]);
    run_or(start_agent, "start agent in left pane", fail);
}

/// Plugin root directory: `HERDR_PLUGIN_ROOT` when injected, else derived from
/// the running executable (`<root>/target/release/jj-workspace` — the layout
/// the manifest's `[[build]]` and actions already depend on).
fn plugin_root() -> PathBuf {
    if let Some(root) = env::var_os("HERDR_PLUGIN_ROOT") {
        if !root.is_empty() {
            return PathBuf::from(root);
        }
    }
    env::current_exe()
        .ok()
        .and_then(|exe| {
            exe.parent()
                .and_then(Path::parent)
                .and_then(Path::parent)
                .map(Path::to_path_buf)
        })
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Path of the shipped right-pane setup script: `<plugin_root>/scripts/…`.
fn setup_script_path() -> PathBuf {
    plugin_root().join("scripts").join("setup-workspace.sh")
}

/// The right-pane command: a single call to the shipped setup script. Every
/// argument is individually shell-quoted; the script inserts the leading
/// arguments of an argv-form `jj.command` before each jj subcommand itself
/// (via `"$@"`), so the pane shell only ever sees plain words — no command
/// sequences, subshell groups, or PATH dependence.
fn setup_script_command(
    script: &Path,
    jj: &ResolvedJj,
    base_rev: &str,
    bookmark_name: &str,
    workspace_id: &str,
    tab_id: &str,
    pane_id: &str,
) -> String {
    let mut words = vec![
        shell_quote(&script.display().to_string()),
        shell_quote(&jj.executable.display().to_string()),
        shell_quote(base_rev),
        shell_quote(bookmark_name),
        shell_quote(workspace_id),
        shell_quote(tab_id),
        shell_quote(pane_id),
    ];
    words.extend(jj.extra_args.iter().map(|arg| shell_quote(arg)));
    words.join(" ")
}

fn command_json(command: &mut Command, what: &str) -> Value {
    let output = match command.output() {
        Ok(output) => output,
        Err(err) => fail(&format!("{what} failed to start: {err}")),
    };
    io::stderr().write_all(&output.stderr).ok();
    if !output.status.success() {
        fail(&format!(
            "{what} failed (exit {})",
            output.status.code().unwrap_or(-1)
        ));
    }
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|err| fail(&format!("{what} returned invalid JSON: {err}")))
}

fn required_json_string(value: &Value, pointer: &str) -> String {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| fail(&format!("Herdr response is missing {pointer}")))
}

// --- agent readiness (herdr agent_status state machine) ---------------------

/// Internal timing constants, deliberately not configurable: they guard
/// against transient status flicker and pathological reply loops, and no
/// realistic user needs to tune them.
const BLOCKED_GRACE_SECS: u64 = 1;
const TRUST_MAX_ATTEMPTS: u32 = 5;

/// Outcome of the `finish-tab` agent wait.
#[derive(Debug)]
enum AgentWaitOutcome {
    /// The agent was detected in the pane; ready to focus.
    Ready,
    /// The agent appeared but stayed blocked without qualifying for (or
    /// surviving) auto-trust; surfaced as a toast.
    NeedsAttention { label: String },
    /// No agent was detected within the startup budget.
    NotDetected,
}

/// Layered readiness wait. Poll `herdr agent list` (JSON envelope) until the
/// left pane has an agent entry — the entry itself means herdr's process-tree
/// detection has seen the agent, which is all "ready to focus" requires. A
/// `blocked` status stable longer than the grace constant means the agent is
/// waiting for human input and is surfaced as a toast. The ONLY automated
/// response is pressing Enter for a codex trust prompt when `auto_trust` is
/// on, inside the startup window, confirmed by a state transition.
fn wait_for_agent_ready(config: &AgentConfig, herdr: &str, pane_id: &str) -> AgentWaitOutcome {
    let started = std::time::Instant::now();
    let timeout = Duration::from_secs(config.startup_timeout_secs);
    let poll = Duration::from_millis(config.poll_interval_ms);
    let trust_window = Duration::from_secs(config.trust_window_secs);
    let mut blocked_since: Option<std::time::Instant> = None;
    let mut trust_attempts: u32 = 0;
    while started.elapsed() < timeout {
        let entry = herdr_json_with(herdr, &["agent", "list"])
            .ok()
            .and_then(|agents| {
                agents
                    .pointer("/result/agents")
                    .and_then(Value::as_array)?
                    .iter()
                    .find(|agent| agent.get("pane_id").and_then(Value::as_str) == Some(pane_id))
                    .cloned()
            });
        let Some(entry) = entry else {
            thread::sleep(poll);
            continue;
        };
        let label = entry
            .get("agent")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let status = entry
            .get("agent_status")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if status != "blocked" {
            // Any non-blocked status (working/idle/done): the agent is up.
            return AgentWaitOutcome::Ready;
        }
        // The agent is blocked (waiting for human input).
        if config.auto_trust
            && label == "codex"
            && started.elapsed() < trust_window
            && trust_attempts < TRUST_MAX_ATTEMPTS
        {
            let mut accept = Command::new(herdr);
            accept.args(["pane", "send-keys", pane_id, "enter"]);
            if run(accept) {
                trust_attempts += 1;
                blocked_since = None;
                thread::sleep(poll);
                continue;
            }
        }
        let blocked_since = blocked_since.get_or_insert_with(std::time::Instant::now);
        if blocked_since.elapsed() >= Duration::from_secs(BLOCKED_GRACE_SECS) {
            return AgentWaitOutcome::NeedsAttention { label };
        }
        thread::sleep(poll);
    }
    AgentWaitOutcome::NotDetected
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn cmd_finish_tab(args: &[String]) -> ! {
    let config = load_config().unwrap_or_else(|err| die(&err.to_string()));
    let workspace_id = args.get(2).map(String::as_str).unwrap_or_default();
    let tab_id = args.get(3).map(String::as_str).unwrap_or_default();
    let left_pane = args.get(4).map(String::as_str).unwrap_or_default();
    if workspace_id.is_empty() || tab_id.is_empty() || left_pane.is_empty() {
        die("finish-tab requires workspace, tab, and pane IDs");
    }

    let herdr = herdr_bin();
    match wait_for_agent_ready(&config.agent, &herdr, left_pane) {
        AgentWaitOutcome::Ready { .. } => {}
        AgentWaitOutcome::NeedsAttention { label } => {
            agent_attention_toast(&herdr, &label, &format!("{label} is waiting for your input."));
            process::exit(1);
        }
        AgentWaitOutcome::NotDetected => {
            agent_attention_toast(
                &herdr,
                "unknown",
                "no agent was detected in the left pane before the startup timeout.",
            );
            process::exit(1);
        }
    }

    for _ in 0..6 {
        let _ = Command::new(&herdr)
            .args(["workspace", "focus", workspace_id])
            .status();
        let _ = Command::new(&herdr).args(["tab", "focus", tab_id]).status();
        let _ = Command::new(&herdr).args(["agent", "focus", left_pane]).status();
        thread::sleep(Duration::from_millis(200));
    }
    process::exit(0);
}

/// Toast surfaced when the agent needs the user: the title carries the
/// runtime detection label (never a hardcoded brand).
fn agent_attention_toast(herdr: &str, label: &str, body: &str) {
    let mut toast = Command::new(herdr);
    toast.args([
        "notification",
        "show",
        &format!("{label} needs attention"),
        "--body",
        body,
        "--position",
        "top-right",
        "--sound",
        "request",
    ]);
    let _ = toast.status();
}

/// Refuse to remove a workspace that still has uncommitted changes. Verified
/// on jj 0.45.1: `jj workspace forget` silently discards a dirty working
/// copy (exit 0, no protection), and the materialized files are deleted right
/// after — the only real data-loss point of `remove`. Already-committed work
/// and bookmarks survive (they live in the shared repo store). Fail-closed:
/// a failed check is treated as dirty.
fn check_remove_clean(jj: &ResolvedJj, workspace: &Path) -> Result<(), String> {
    // Short name on the first line (toast-critical: herdr truncates long
    // bodies); the full path follows on its own line for stderr and the log.
    let name = workspace
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| workspace.display().to_string());
    let mut command = Command::new(&jj.executable);
    command
        .current_dir(workspace)
        .args(&jj.extra_args)
        .args(["diff", "--summary", "-r", "@"]);
    let output = match command.output() {
        Ok(output) => output,
        Err(err) => {
            return Err(format!(
                "cannot check workspace '{name}' for uncommitted changes \
                 (refusing to remove): {err}\n\
                 workspace path: {}",
                workspace.display()
            ))
        }
    };
    io::stderr().write_all(&output.stderr).ok();
    if !output.status.success() {
        return Err(format!(
            "cannot check workspace '{name}' for uncommitted changes \
             (jj diff exited {}): refusing to remove\n\
             workspace path: {}",
            output.status.code().unwrap_or(-1),
            workspace.display()
        ));
    }
    if String::from_utf8_lossy(&output.stdout).trim().is_empty() {
        return Ok(());
    }
    Err(format!(
        "refusing to remove workspace '{name}': it has uncommitted changes.\n\
         workspace path: {}\n\
         Already-committed work and bookmarks are safe in the repo store, but the \
         materialized changes in this checkout would be deleted.\n\
         Commit (`jj commit`) or undo (`jj restore`) them first, then run remove again.",
        workspace.display()
    ))
}

/// Action (headless): forget the current jj workspace, delete it, close its tab.
fn cmd_remove() -> ! {
    let config = load_config().unwrap_or_else(|err| die(&err.to_string()));
    let jj = resolve_jj_command(&config.jj.command, &path_dirs())
        .unwrap_or_else(|err| die(&err.to_string()));
    let ctx = env::var("HERDR_PLUGIN_CONTEXT_JSON").unwrap_or_default();
    let tab = env::var("HERDR_TAB_ID")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| json_string_field(&ctx, "tab_id"));
    let cwd = json_string_field(&ctx, "workspace_cwd").unwrap_or_default();
    if cwd.is_empty() {
        die("no workspace cwd in context");
    }

    let canon = match fs::canonicalize(&cwd) {
        Ok(p) => p,
        Err(err) => die(&format!("cannot resolve {cwd}: {err}")),
    };
    if !canon.join(".jj").exists() {
        die(&format!("{} is not a jj workspace", canon.display()));
    }
    // The MAIN workspace stores .jj/repo as a directory; a secondary workspace
    // stores it as a file pointer. Never remove the main workspace.
    if canon.join(".jj").join("repo").is_dir() {
        die(&format!(
            "refusing to remove the MAIN jj workspace ({})",
            canon.display()
        ));
    }
    if canon == Path::new("/") || canon.parent().is_none() {
        die(&format!(
            "refusing to remove unsafe path: {}",
            canon.display()
        ));
    }
    // Refuse dirty workspaces before anything destructive happens.
    check_remove_clean(&jj, &canon).unwrap_or_else(|message| die(&message));

    let mut forget = Command::new(&jj.executable);
    forget
        .current_dir(&canon)
        .args(&jj.extra_args)
        .args(["workspace", "forget"]);
    run_or(forget, "jj workspace forget", die);

    if let Err(err) = fs::remove_dir_all(&canon) {
        die(&format!("failed to delete {}: {err}", canon.display()));
    }

    match tab {
        Some(tab) => {
            let mut close = Command::new(herdr_bin());
            close.args(["tab", "close", &tab]);
            run_or(close, "herdr tab close", die);
        }
        None => eprintln!("warning: no tab id in context; Herdr tab left open"),
    }
    println!("removed jj workspace: {}", canon.display());
    process::exit(0);
}

// --- wizard TUI (ported from herdr src/ui/dialogs.rs + widgets.rs) ----------

/// Herdr's catppuccin palette (src/app/state.rs `Palette::catppuccin`).
struct Palette {
    accent: Color,
    panel_bg: Color,
    surface0: Color,
    surface_dim: Color,
    overlay0: Color,
    text: Color,
    subtext0: Color,
    red: Color,
}

fn catppuccin() -> Palette {
    Palette {
        accent: Color::Rgb(137, 180, 250),
        panel_bg: Color::Rgb(24, 24, 37),
        surface0: Color::Rgb(49, 50, 68),
        surface_dim: Color::Rgb(30, 30, 46),
        overlay0: Color::Rgb(108, 112, 134),
        text: Color::Rgb(205, 214, 244),
        subtext0: Color::Rgb(166, 173, 200),
        red: Color::Rgb(243, 139, 168),
    }
}

/// Returns the chosen source + name + base revision, or None when cancelled.
#[allow(clippy::too_many_arguments)]
fn run_workspace_wizard(
    choices: &[WorkspaceChoice],
    initial_selection: usize,
    root: &Path,
    config: &Config,
    jj: &ResolvedJj,
    initial_name: String,
    initial_base: String,
) -> io::Result<Option<WizardResult>> {
    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(out))?;

    let mut query = String::new();
    let mut filtered = filtered_choice_indices(choices, &query);
    let mut selected = filtered
        .iter()
        .position(|index| *index == initial_selection)
        .unwrap_or(0);
    let mut field = WizardField::WorkspaceSearch;
    let mut name = initial_name;
    let mut base = initial_base;
    let mut name_edit_state = NameEditState::Fresh;
    let mut base_replace_on_type = true;
    let mut base_dirty = false;
    let mut error: Option<String> = None;

    let outcome = loop {
        let _ = terminal.draw(|frame| {
            draw_workspace_wizard(
                frame,
                &WizardView {
                    choices,
                    filtered: &filtered,
                    selected,
                    field,
                    query: &query,
                    name: &name,
                    base: &base,
                    root,
                    error: error.as_deref(),
                },
            )
        });
        match event::read() {
            Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Esc => break None,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break None,
                KeyCode::Tab | KeyCode::BackTab => {
                    // Zero-candidate collapse: no other field is reachable
                    // (the UI renders source + esc only), so Tab must not
                    // move focus to a hidden section.
                    if !choices.is_empty() {
                        field = next_wizard_field(field);
                        error = None;
                    }
                }
                KeyCode::Up if field == WizardField::WorkspaceSearch => {
                    selected = previous_index(selected, filtered.len());
                    error = None;
                }
                KeyCode::Down if field == WizardField::WorkspaceSearch => {
                    selected = next_index(selected, filtered.len());
                    error = None;
                }
                KeyCode::Char('p' | 'u' | 'k')
                    if field == WizardField::WorkspaceSearch
                        && key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    selected = previous_index(selected, filtered.len());
                    error = None;
                }
                KeyCode::Char('n' | 'd' | 'j')
                    if field == WizardField::WorkspaceSearch
                        && key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    selected = next_index(selected, filtered.len());
                    error = None;
                }
                KeyCode::Enter => {
                    // Zero-candidate collapse: Enter has nothing to submit
                    // and must not surface the generic "no matching
                    // workspace" error — the empty state is the message.
                    if choices.is_empty() {
                        continue;
                    }
                    let Some(choice_index) = filtered.get(selected).copied() else {
                        error = Some("no matching workspace".into());
                        continue;
                    };
                    if !valid_branch(&name) {
                        error = Some("name must match [A-Za-z0-9._/-]".into());
                        continue;
                    }
                    let source = choices[choice_index].clone();
                    if !Path::new(&source.path).is_dir() {
                        error = Some(format!("folder does not exist: {}", source.path));
                        continue;
                    }
                    let checkout = workspace_destination(root, &source.path, &name);
                    if checkout.exists() {
                        error =
                            Some(format!("checkout already exists: {}", checkout.display()));
                        continue;
                    }
                    // Candidates are guaranteed jj workspaces (jj-only filter
                    // in `load_workspace_choices`), so the final base is
                    // always solved from the resolution chain — or validated
                    // when the user edited the field.
                    let base_rev = match wizard_final_base_rev(
                        config,
                        jj,
                        Path::new(&repo_root(&source.path)),
                        &base,
                        base_dirty,
                    ) {
                        Ok(value) => value,
                        Err(message) => {
                            error = Some(message);
                            continue;
                        }
                    };
                    break Some(WizardResult {
                        source,
                        name: name.clone(),
                        base_rev,
                    });
                }
                KeyCode::Backspace if field == WizardField::WorkspaceSearch => {
                    query.pop();
                    filtered = filtered_choice_indices(choices, &query);
                    selected = 0;
                    error = None;
                }
                KeyCode::Backspace if field == WizardField::Name => {
                    apply_name_key(&mut name, &mut name_edit_state, NameKey::Backspace);
                    error = None;
                }
                KeyCode::Char(c)
                    if field == WizardField::Name
                        && !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    apply_name_key(&mut name, &mut name_edit_state, NameKey::Char(c));
                    error = None;
                }
                KeyCode::Backspace if field == WizardField::Base => {
                    if base_replace_on_type {
                        base.clear();
                        base_replace_on_type = false;
                    } else {
                        base.pop();
                    }
                    base_dirty = true;
                    error = None;
                }
                KeyCode::Char(c)
                    if field == WizardField::Base
                        && !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    if base_replace_on_type {
                        base.clear();
                        base_replace_on_type = false;
                    }
                    base.push(c);
                    base_dirty = true;
                    error = None;
                }
                KeyCode::Char(c)
                    if field == WizardField::WorkspaceSearch
                        && !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    query.push(c);
                    filtered = filtered_choice_indices(choices, &query);
                    selected = 0;
                    error = None;
                }
                _ => {}
            },
            Ok(_) => {}
            Err(err) => {
                let _ = restore_terminal(&mut terminal);
                return Err(err);
            }
        }
    };

    restore_terminal(&mut terminal)?;
    Ok(outcome)
}

fn filtered_choice_indices(choices: &[WorkspaceChoice], query: &str) -> Vec<usize> {
    if query.trim().is_empty() {
        return (0..choices.len()).collect();
    }

    let mut matches: Vec<(usize, i64)> = choices
        .iter()
        .enumerate()
        .filter_map(|(index, choice)| {
            let label_score = fuzzy_score(&choice.label, query).map(|score| score + 1_000);
            let path_score = fuzzy_score(&choice.path, query);
            label_score
                .into_iter()
                .chain(path_score)
                .max()
                .map(|score| (index, score))
        })
        .collect();
    matches.sort_by(|(left_index, left_score), (right_index, right_score)| {
        right_score
            .cmp(left_score)
            .then_with(|| left_index.cmp(right_index))
    });
    matches.into_iter().map(|(index, _)| index).collect()
}

fn fuzzy_score(candidate: &str, query: &str) -> Option<i64> {
    let query: Vec<char> = query
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if query.is_empty() {
        return Some(0);
    }

    let candidate: Vec<char> = candidate.chars().flat_map(char::to_lowercase).collect();
    let mut score = 0i64;
    let mut search_from = 0usize;
    let mut previous_match = None;

    for needle in query {
        let offset = candidate[search_from..]
            .iter()
            .position(|ch| *ch == needle)?;
        let index = search_from + offset;
        score += 20;
        if previous_match == Some(index.saturating_sub(1)) {
            score += 15;
        }
        if index == 0 || !candidate[index - 1].is_alphanumeric() {
            score += 10;
        }
        score -= index as i64;
        previous_match = Some(index);
        search_from = index + 1;
    }

    Some(score)
}

fn previous_index(selected: usize, len: usize) -> usize {
    if len == 0 {
        0
    } else if selected == 0 {
        len - 1
    } else {
        selected - 1
    }
}

fn next_index(selected: usize, len: usize) -> usize {
    if len == 0 {
        0
    } else {
        (selected + 1) % len
    }
}

fn workspace_destination(root: &Path, source: &str, name: &str) -> PathBuf {
    root.join(basename(&repo_root(source)))
        .join(branch_to_path_slug(name))
}

/// Content column indent shared by every section: the leading area is 3
/// columns wide (2 spaces + 1 marker slot for the source list; the same
/// gutter stays blank for name/base/checkout so their values align with the
/// list labels).
const SECTION_CONTENT_INDENT: u16 = 3;

/// Section title grammar: always bold; focus is expressed by color only —
/// accent (blue) when the field is focused, subtext0 otherwise. Read-only
/// sections (checkout) never focus and use the subtext0 form.
fn section_title_style(focused: bool, p: &Palette) -> Style {
    let color = if focused { p.accent } else { p.subtext0 };
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

fn render_section_title(frame: &mut Frame, area: Rect, title: &str, focused: bool, p: &Palette) {
    frame.render_widget(Paragraph::new(title).style(section_title_style(focused, p)), area);
}

/// Top-of-modal static hint line shown in the normal state. The zero-candidate
/// collapse replaces it with an esc-only hint.
const WIZARD_HINT: &str =
    "type to filter or edit · ↑/↓ select · tab switch · ↵ create · esc cancel";

fn draw_workspace_wizard(frame: &mut Frame, view: &WizardView<'_>) {
    let WizardView {
        choices,
        filtered,
        selected,
        field,
        query,
        name,
        base,
        root,
        error,
    } = *view;
    let p = catppuccin();
    let area = frame.area();
    dim_background(frame, area);
    let Some(inner) = render_modal_shell(frame, area, 86, 26, &p) else {
        return;
    };
    if inner.height < 14 {
        return;
    }

    let indent = usize::from(SECTION_CONTENT_INDENT);
    let pad = " ".repeat(indent);
    let mut y = inner.y;

    render_modal_header(
        frame,
        Rect::new(inner.x, y, inner.width, 1),
        "New Workspace",
        &p,
    );
    y += 1;

    // Zero-candidate collapse: only the source section + esc affordance.
    // The run loop already ignores Tab/Enter here, so nothing below is
    // reachable — render source title, the query bar, the empty message and
    // an esc hint, then stop.
    if choices.is_empty() {
        render_section_title(
            frame,
            Rect::new(inner.x, y, inner.width, 1),
            "Source Workspace",
            field == WizardField::WorkspaceSearch,
            &p,
        );
        y += 1;
        frame.render_widget(
            Paragraph::new(format!("{pad}no jj workspaces — open herdr's project picker instead"))
                .style(Style::default().fg(p.overlay0)),
            Rect::new(inner.x, y, inner.width, 1),
        );
        y += 1;
        frame.render_widget(
            Paragraph::new(format!("{pad}press esc to close")).style(Style::default().fg(p.overlay0)),
            Rect::new(inner.x, y, inner.width, 1),
        );
        return;
    }

    // Static hint line: one place for all operation hints, before any section.
    frame.render_widget(
        Paragraph::new(WIZARD_HINT).style(Style::default().fg(p.overlay0)),
        Rect::new(inner.x, y, inner.width, 1),
    );
    y += 1;
    y += 1; // blank separator after the hint block

    // --- source workspace -------------------------------------------------
    render_section_title(
        frame,
        Rect::new(inner.x, y, inner.width, 1),
        "Source Workspace",
        field == WizardField::WorkspaceSearch,
        &p,
    );
    y += 1;
    let query_focused = field == WizardField::WorkspaceSearch;
    let query_span = if query.is_empty() && query_focused {
        Span::styled(format!("{pad}filter…"), Style::default().fg(p.overlay0))
    } else {
        let cursor = if query_focused { "█" } else { "" };
        Span::styled(format!("{pad}{query}{cursor}"), Style::default().fg(p.text))
    };
    frame.render_widget(
        Paragraph::new(Line::from(query_span)).style(Style::default().bg(p.surface0)),
        Rect::new(inner.x, y, inner.width, 1),
    );
    y += 1;

    // List area: fixed block budget (header+hint+2 blanks+source title+query
    // +4 section blocks+3 separators+error+buttons) leaves the rest to the
    // list, clamped so tiny panes still fit.
    let list_height = usize::from(inner.height.saturating_sub(17).clamp(3, 8));
    let max_start = filtered.len().saturating_sub(list_height);
    let start = selected.saturating_sub(list_height / 2).min(max_start);
    let end = (start + list_height).min(filtered.len());
    for (visible_index, choice_index) in filtered[start..end].iter().enumerate() {
        let absolute_index = start + visible_index;
        let choice = &choices[*choice_index];
        let active = absolute_index == selected;
        // Marker slot occupies the first column of the shared 3-wide gutter;
        // the label always starts at the same column as other section content.
        let marker = if active { "▸  " } else { "   " };
        let line = Line::from(vec![
            Span::styled(
                format!("{marker}{} ", choice.label),
                Style::default().add_modifier(if active {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            ),
            Span::styled(choice.path.clone(), Style::default().fg(p.subtext0)),
        ]);
        let style = if active {
            Style::default().fg(p.text).bg(p.surface0)
        } else {
            Style::default().fg(p.text)
        };
        frame.render_widget(
            Paragraph::new(line).style(style),
            Rect::new(inner.x, y, inner.width, 1),
        );
        y += 1;
    }
    if filtered.is_empty() {
        frame.render_widget(
            Paragraph::new(format!("{pad}no matching workspaces"))
                .style(Style::default().fg(p.overlay0)),
            Rect::new(inner.x, y, inner.width, 1),
        );
        y += 1;
    }

    // --- new workspace name -----------------------------------------------
    y += 1;
    render_section_title(
        frame,
        Rect::new(inner.x, y, inner.width, 1),
        "New Workspace Name",
        field == WizardField::Name,
        &p,
    );
    y += 1;
    let name_cursor = if field == WizardField::Name { "█" } else { "" };
    frame.render_widget(
        Paragraph::new(format!("{pad}{name}{name_cursor}"))
            .style(Style::default().fg(p.text).bg(p.surface0)),
        Rect::new(inner.x, y, inner.width, 1),
    );
    y += 1;

    // --- base · jj revset --------------------------------------------------
    y += 1;
    render_section_title(
        frame,
        Rect::new(inner.x, y, inner.width, 1),
        "Base · jj revset",
        field == WizardField::Base,
        &p,
    );
    y += 1;
    let base_cursor = if field == WizardField::Base { "█" } else { "" };
    frame.render_widget(
        Paragraph::new(format!("{pad}{base}{base_cursor}"))
            .style(Style::default().fg(p.text).bg(p.surface0)),
        Rect::new(inner.x, y, inner.width, 1),
    );
    y += 1;

    // --- checkout (read-only preview) -------------------------------------
    y += 1;
    render_section_title(
        frame,
        Rect::new(inner.x, y, inner.width, 1),
        "Checkout",
        false,
        &p,
    );
    y += 1;
    // Every candidate is a jj workspace, so the preview is the derived
    // destination; a filter with no match has no selected source to derive
    // from and shows a placeholder instead.
    let preview = match filtered.get(selected).map(|index| &choices[*index]) {
        Some(choice) => workspace_destination(root, &choice.path, name)
            .display()
            .to_string(),
        None => "no matching workspace".into(),
    };
    frame.render_widget(
        Paragraph::new(format!("{pad}{preview}")).style(Style::default().fg(p.subtext0)),
        Rect::new(inner.x, y, inner.width, 1),
    );
    y += 1;

    if let Some(message) = error {
        y += 1;
        frame.render_widget(
            Paragraph::new(format!("{pad}{message}")).style(Style::default().fg(p.red)),
            Rect::new(inner.x, y, inner.width, 1),
        );
    }

    let (create_rect, cancel_rect) = button_rects(inner);
    render_action_button(
        frame,
        create_rect,
        Some("↵"),
        "create and open",
        Style::default()
            .fg(panel_contrast_fg(&p))
            .bg(p.accent)
            .add_modifier(Modifier::BOLD),
    );
    render_action_button(
        frame,
        cancel_rect,
        Some("esc"),
        "cancel",
        Style::default()
            .fg(p.text)
            .bg(p.surface0)
            .add_modifier(Modifier::BOLD),
    );
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}

// Ported verbatim from herdr's src/ui/widgets.rs / src/ui.rs.

fn dim_background(frame: &mut Frame, area: Rect) {
    let buf = frame.buffer_mut();
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            let cell = &mut buf[(x, y)];
            cell.set_style(cell.style().add_modifier(Modifier::DIM));
        }
    }
}

fn render_modal_shell(frame: &mut Frame, area: Rect, w: u16, h: u16, p: &Palette) -> Option<Rect> {
    let popup = centered_popup_rect(area, w, h)?;
    render_panel_shell(frame, popup, p.accent, p.panel_bg)
}

fn render_panel_shell(frame: &mut Frame, area: Rect, border: Color, bg: Color) -> Option<Rect> {
    if area.width < 2 || area.height < 2 {
        return None;
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border))
        .border_set(symbols::border::PLAIN)
        .style(Style::default().bg(bg));
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    Some(inner)
}

fn centered_popup_rect(area: Rect, popup_w: u16, popup_h: u16) -> Option<Rect> {
    let popup_w = popup_w.min(area.width.saturating_sub(4));
    let popup_h = popup_h.min(area.height.saturating_sub(2));
    if popup_w < 4 || popup_h < 4 {
        return None;
    }
    let popup_x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_h)) / 2;
    Some(Rect::new(popup_x, popup_y, popup_w, popup_h))
}

fn render_modal_header(frame: &mut Frame, area: Rect, title: &str, p: &Palette) {
    let line = Line::from(vec![Span::styled(
        title,
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]);
    frame.render_widget(Paragraph::new(line), area);
}

fn render_action_button(
    frame: &mut Frame,
    rect: Rect,
    hint: Option<&str>,
    label: &str,
    style: Style,
) {
    frame.render_widget(
        Paragraph::new(action_button_text(hint, label))
            .style(style)
            .alignment(Alignment::Center),
        rect,
    );
}

fn action_button_text(hint: Option<&str>, label: &str) -> String {
    match hint {
        Some(hint) => format!(" {hint} {label} "),
        None => format!(" {label} "),
    }
}

fn panel_contrast_fg(p: &Palette) -> Color {
    match p.panel_bg {
        Color::Reset => p.surface_dim,
        color => color,
    }
}

/// Herdr's `new_linked_worktree_button_rects`: a centered "create / cancel" row.
fn button_rects(inner: Rect) -> (Rect, Rect) {
    let create = action_button_text(Some("↵"), "create and open")
        .chars()
        .count() as u16;
    let cancel = action_button_text(Some("esc"), "cancel").chars().count() as u16;
    let gap = 2u16;
    let total = create + cancel + gap;
    let mut x = inner.x + inner.width.saturating_sub(total) / 2;
    let y = inner.y + inner.height.saturating_sub(1);
    let create_rect = Rect::new(x, y, create, 1);
    x = x.saturating_add(create).saturating_add(gap);
    let cancel_rect = Rect::new(x, y, cancel, 1);
    (create_rect, cancel_rect)
}

// --- naming (mirrors src/worktree.rs in herdr) -----------------------------

const ADJECTIVES: [&str; 8] = [
    "brave", "calm", "clear", "green", "lucky", "quiet", "rapid", "silver",
];
const NOUNS: [&str; 8] = [
    "river", "cloud", "field", "forest", "harbor", "meadow", "stone", "valley",
];

fn generated_name(seed: u64) -> String {
    let adjective = ADJECTIVES[(seed as usize) % ADJECTIVES.len()];
    let noun = NOUNS[((seed / ADJECTIVES.len() as u64) as usize) % NOUNS.len()];
    let suffix = seed & 0xffff;
    format!("workspace/{adjective}-{noun}-{suffix:04x}")
}

fn branch_to_path_slug(branch: &str) -> String {
    let mut slug = String::new();
    let mut last_was_dash = false;
    for ch in branch.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash {
            slug.push('-');
            last_was_dash = true;
        }
    }
    let trimmed = slug.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "workspace".into()
    } else {
        trimmed
    }
}

fn seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// Checkout root from `jj.workspace_root` (default ~/.herdr/workspaces), with
/// a leading `~` expanded to the user's home.
fn workspaces_root(config: &Config) -> PathBuf {
    PathBuf::from(expand_tilde(config.jj.workspace_root.trim_end_matches('/')))
}

fn expand_tilde(path: &str) -> String {
    expand_tilde_with_home(path, env::var("HOME").ok().as_deref())
}

fn expand_tilde_with_home(path: &str, home: Option<&str>) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = home {
            return format!("{home}/{rest}");
        }
    }
    path.to_string()
}

// --- helpers ---------------------------------------------------------------

fn herdr_bin() -> String {
    env::var("HERDR_BIN_PATH")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "herdr".into())
}

fn plugin_id() -> String {
    env::var("HERDR_PLUGIN_ID")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "expnn.jj-workspace".into())
}

/// Walk `path` and its ancestors for a `.jj` marker (file or directory) and
/// return the directory containing it — the jj workspace root. A workspace
/// root carrying its own `.jj` returns itself unchanged (no extra scanning);
/// a path inside a repository is normalized upward to the root; a path with
/// no `.jj` on any ancestor is not a jj workspace and yields `None` (the
/// caller filters such candidates out).
fn jj_root(path: &str) -> Option<String> {
    let mut dir = Path::new(path);
    loop {
        if dir.join(".jj").exists() {
            return Some(dir.display().to_string());
        }
        dir = dir.parent()?;
    }
}

/// Resolve any jj workspace path to its MAIN workspace root.
///
/// The main workspace stores `.jj/repo` as the store *directory*; a secondary
/// workspace stores `.jj/repo` as a *file* holding the path to the main store,
/// relative to `.jj/` (e.g. `../../../../../agent-os/.jj/repo`). Following that
/// pointer and stripping the trailing `.jj/repo` yields the repo's real root, so
/// naming + placement stay stable no matter which workspace launched the wizard.
/// Falls back to the input path if anything is unexpected.
fn repo_root(workspace: &str) -> String {
    let jj_dir = Path::new(workspace).join(".jj");
    let repo_ptr = jj_dir.join("repo");
    // Main workspace: `.jj/repo` is the store dir itself — already the root.
    if repo_ptr.is_dir() {
        return workspace.to_string();
    }
    let pointer = match fs::read_to_string(&repo_ptr) {
        Ok(s) => s.trim().to_string(),
        Err(_) => return workspace.to_string(),
    };
    // Pointer is relative to `.jj/`; drop `repo` then `.jj` to reach the root.
    let root = jj_dir
        .join(&pointer)
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf);
    match root.and_then(|r| fs::canonicalize(r).ok()) {
        Some(canon) => canon.display().to_string(),
        None => workspace.to_string(),
    }
}

fn valid_branch(branch: &str) -> bool {
    !branch.is_empty()
        && branch
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'))
}

fn basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("repo")
        .to_string()
}

fn run_or(cmd: Command, what: &str, on_err: fn(&str) -> !) {
    let mut cmd = cmd;
    match cmd.status() {
        Ok(status) if status.success() => {}
        Ok(status) => on_err(&format!(
            "{what} failed (exit {})",
            status.code().unwrap_or(-1)
        )),
        Err(err) => on_err(&format!("{what} failed to start: {err}")),
    }
}

fn run(mut cmd: Command) -> bool {
    matches!(cmd.status(), Ok(status) if status.success())
}

fn fail(message: &str) -> ! {
    // Fatal errors must reach error.log even when their UI outlet (pane
    // print + enter prompt) is transient or scrolled away.
    log_error(message);
    eprintln!("error: {message}");
    print!("\npress enter to close...");
    let _ = io::stdout().flush();
    let mut line = String::new();
    let _ = io::stdin().read_line(&mut line);
    process::exit(1);
}

fn die(message: &str) -> ! {
    eprintln!("error: {message}");
    // Herdr does not surface plugin action failures on its own — the action's
    // stderr has no visible outlet. Show a toast ourselves so headless errors
    // (bad config, unresolved jj, refused remove, …) reach the user.
    //
    // Toast limits (verified against herdr source): title ≤ 80 chars, body ≤
    // 240 chars, single-line rendering, not copyable, ~3s display. So the
    // toast carries only a one-line summary plus a pointer to the full
    // message, which is appended to `<state dir>/error.log` (copyable,
    // persistent, accumulates across runs).
    let log_path = log_error(message);
    let body = die_toast_body(message, log_path.as_deref());
    let _ = Command::new(herdr_bin())
        .args([
            "notification",
            "show",
            "jj-workspace error",
            "--body",
            &body,
            "--position",
            "top-right",
            "--sound",
            "request",
        ])
        .status();
    process::exit(1);
}

/// Append the message to `<$HERDR_PLUGIN_STATE_DIR>/error.log` (single line
/// per entry, UTC timestamp) and return the log path on success. Best-effort:
/// a logging failure must not mask the original error.
fn log_error(message: &str) -> Option<String> {
    let dir = env::var_os("HERDR_PLUGIN_STATE_DIR")?;
    let path = PathBuf::from(dir).join("error.log");
    append_error_log(&path, message)
        .then(|| path.display().to_string())
}

/// Append the message to `error.log` as ONE line: `[UTC timestamp] message`
/// with newlines escaped to a literal `\n` so entries stay greppable and
/// filterable by timestamp. Best-effort: failures return false.
fn append_error_log(path: &Path, message: &str) -> bool {
    let timestamp = format_utc_now();
    let single_line = message.replace('\n', "\\n");
    let entry = format!("[{timestamp}] {single_line}\n");
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| file.write_all(entry.as_bytes()))
        .is_ok()
}

/// The toast body for a die() message: the first line, truncated, plus a
/// pointer to the full log when one was written. herdr hard-caps bodies at
/// 240 chars and collapses newlines, so this stays short and single-line.
fn die_toast_body(message: &str, log_path: Option<&str>) -> String {
    let first_line = message.lines().next().unwrap_or(message);
    let mut summary: String = first_line.chars().take(120).collect();
    if first_line.chars().count() > 120 {
        summary.push('…');
    }
    match log_path {
        Some(path) => format!("{summary} — full log: {path}"),
        None => summary,
    }
}

/// Format the current time as `YYYY-MM-DD HH:MM:SS UTC` without a chrono
/// dependency (days-from-civil inverse, Howard Hinnant's algorithm).
fn format_utc_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_unix_timestamp(secs)
}

fn format_unix_timestamp(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (hour, minute, second) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // Civil-from-days: 1970-01-01 = day 0.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {hour:02}:{minute:02}:{second:02} UTC")
}

fn json_string_field(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let after_key = json.split_once(&needle)?.1;
    let after_colon = after_key.split_once(':')?.1.trim_start();
    let value = after_colon.strip_prefix('"')?;
    let mut out = String::new();
    let mut escaped = false;
    for ch in value.chars() {
        if escaped {
            out.push(match ch {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                other => other,
            });
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return Some(out);
        } else {
            out.push(ch);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_selector_wraps_in_both_directions() {
        assert_eq!(previous_index(0, 3), 2);
        assert_eq!(previous_index(2, 3), 1);
        assert_eq!(next_index(2, 3), 0);
        assert_eq!(next_index(0, 3), 1);
    }

    #[test]
    fn workspace_selector_fuzzy_filters_labels_and_paths() {
        let choices = vec![
            WorkspaceChoice {
                id: "w1".into(),
                label: "general".into(),
                path: "/home/nathan/misc".into(),
            },
            WorkspaceChoice {
                id: "w2".into(),
                label: "rivet-website".into(),
                path: "/home/nathan/rivet-website".into(),
            },
            WorkspaceChoice {
                id: "w3".into(),
                label: "docs".into(),
                path: "/home/nathan/dynamic-apps".into(),
            },
        ];

        assert_eq!(filtered_choice_indices(&choices, "rvws"), vec![1]);
        assert_eq!(filtered_choice_indices(&choices, "DYN APP"), vec![2]);
        assert!(filtered_choice_indices(&choices, "not-here").is_empty());
    }

    #[test]
    fn workspace_selector_prefers_label_matches() {
        let choices = vec![
            WorkspaceChoice {
                id: "w1".into(),
                label: "website".into(),
                path: "/tmp/project".into(),
            },
            WorkspaceChoice {
                id: "w2".into(),
                label: "project".into(),
                path: "/tmp/website".into(),
            },
        ];

        assert_eq!(filtered_choice_indices(&choices, "web"), vec![0, 1]);
    }

    #[test]
    fn workspace_name_maps_to_a_safe_checkout_slug() {
        assert_eq!(
            branch_to_path_slug("workspace/Fix API_v2"),
            "workspace-fix-api-v2"
        );
        assert_eq!(branch_to_path_slug("///"), "workspace");
    }

    #[test]
    fn setup_script_command_is_a_single_plain_call() {
        // The right-pane command is one call to the shipped script: every
        // argument individually quoted, no command sequences or subshell
        // groups, no dependence on the pane shell's PATH. The leading
        // arguments of an argv-form `jj.command` ride along as trailing
        // script arguments (the script inserts them via `"$@"`).
        let script = Path::new("/opt/plugin/scripts/setup-workspace.sh");
        let jj = ResolvedJj {
            executable: PathBuf::from("/usr/bin/jj"),
            extra_args: vec!["--at-op".into(), "@-".into()],
        };
        let command =
            setup_script_command(script, &jj, "trunk()", "workspace/fix-api", "w", "t", "p");
        assert_eq!(
            command,
            "'/opt/plugin/scripts/setup-workspace.sh' '/usr/bin/jj' 'trunk()' \
             'workspace/fix-api' 'w' 't' 'p' '--at-op' '@-'"
        );
    }

    #[test]
    fn setup_script_parses_under_sh() {
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/setup-workspace.sh");
        let status = std::process::Command::new("sh")
            .arg("-n")
            .arg(&script)
            .status()
            .expect("failed to run sh -n");
        assert!(status.success(), "setup script must parse under sh");
    }

    #[test]
    fn shell_arguments_are_single_quoted() {
        assert_eq!(shell_quote("plain"), "'plain'");
        assert_eq!(shell_quote("it's"), "'it'\"'\"'s'");
    }

    /// Unique per-test temp dir, removed on drop.
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new() -> TempDir {
            // Tests run concurrently in one process, so a nanosecond
            // timestamp alone is not unique: two threads creating a TempDir
            // in the same clock tick collide and tear each other down.
            // A per-process sequence counter keeps the path unique.
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock before epoch")
                .as_nanos();
            let seq = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "jj-workspace-config-{}-{nanos}-{seq}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).expect("create temp dir");
            TempDir(dir)
        }

        fn path(&self) -> &std::path::Path {
            &self.0
        }

        fn write_config(&self, content: &str) -> std::path::PathBuf {
            let path = self.0.join("config.toml");
            std::fs::write(&path, content).expect("write config fixture");
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn config_missing_file_returns_defaults() {
        let dir = TempDir::new();
        let config =
            load_config_from(&dir.path().join("config.toml")).expect("missing file = defaults");
        assert_eq!(config, Config::default());
        assert_eq!(config.jj.base_rev, "trunk()");
        assert_eq!(config.jj.workspace_root, "~/.herdr/workspaces");
        assert_eq!(config.agent.command, "opencode");
        // Pin the full 34-item default: any change to the default bootstrap
        // list must consciously update this literal (and the [..; 34] count).
        assert_eq!(
            config.agent.bootstrap_paths,
            vec![
                "AGENTS.md",
                "AGENT.md",
                "AGENTS.override.md",
                "CLAUDE.md",
                "CLAUDE.local.md",
                "GEMINI.md",
                "QWEN.md",
                "CRUSH.md",
                ".mcp.json",
                "opencode.json",
                "opencode.jsonc",
                ".cursorrules",
                ".windsurfrules",
                ".goosehints",
                ".augment-guidelines",
                ".github/copilot-instructions.md",
                ".agents",
                ".claude",
                ".codex",
                ".cursor",
                ".gemini",
                ".qwen",
                ".opencode",
                ".windsurf",
                ".devin",
                ".clinerules",
                ".cline",
                ".kilo",
                ".kilocode",
                ".augment",
                ".continue",
                ".github/instructions",
                ".crush",
                ".goose",
            ]
        );
    }

    #[test]
    fn config_parses_all_sections() {
        let dir = TempDir::new();
        let path = dir.write_config(
            "[jj]\n\
             base_rev = \"dev\"\n\
             workspace_root = \"~/code/workspaces\"\n\n\
             [agent]\n\
             command = \"opencode\"\n\
             bootstrap_paths = [\"AGENTS.md\", \"CLAUDE.md\"]\n",
        );
        let config = load_config_from(&path).expect("valid config");
        assert_eq!(config.jj.base_rev, "dev");
        assert_eq!(config.jj.workspace_root, "~/code/workspaces");
        assert_eq!(config.agent.command, "opencode");
        assert_eq!(config.agent.bootstrap_paths, vec!["AGENTS.md", "CLAUDE.md"]);
    }

    #[test]
    fn config_missing_sections_default_missing_keys() {
        let dir = TempDir::new();
        let path = dir.write_config("[agent]\ncommand = \"opencode\"\n");
        let config = load_config_from(&path).expect("partial config");
        assert_eq!(config.jj, JjConfig::default());
        assert_eq!(config.agent.command, "opencode");
        assert_eq!(config.agent.bootstrap_paths, AgentConfig::default().bootstrap_paths);
    }

    #[test]
    fn config_syntax_error_is_rejected() {
        let dir = TempDir::new();
        let path = dir.write_config("[jj]\nbase_rev = \"trunk()\n");
        let err = load_config_from(&path).expect_err("unterminated string");
        assert!(err.to_string().starts_with("invalid config"), "{}", err);
    }

    #[test]
    fn config_type_error_names_the_key() {
        let dir = TempDir::new();
        let path = dir.write_config("[jj]\nbase_rev = 123\n");
        let err = load_config_from(&path).expect_err("type error");
        let message = err.to_string();
        assert!(message.contains("base_rev"), "{message}");
    }

    #[test]
    fn config_unknown_key_is_rejected() {
        let dir = TempDir::new();
        let path = dir.write_config("[agent]\nstart_command = \"codex\"\n");
        let err = load_config_from(&path).expect_err("unknown key");
        let message = err.to_string();
        assert!(message.contains("start_command"), "{message}");
    }

    #[test]
    fn config_empty_string_is_rejected() {
        let dir = TempDir::new();
        let path = dir.write_config("[agent]\ncommand = \"\"\n");
        let err = load_config_from(&path).expect_err("empty command");
        let message = err.to_string();
        assert!(message.contains("agent.command") && message.contains("empty"), "{message}");
    }

    #[test]
    fn config_empty_bootstrap_entry_is_rejected() {
        let dir = TempDir::new();
        let path = dir.write_config("[agent]\nbootstrap_paths = [\"AGENTS.md\", \"\"]\n");
        let err = load_config_from(&path).expect_err("empty bootstrap path");
        assert!(err.to_string().contains("bootstrap_paths"), "{}", err);
    }

    #[test]
    fn config_explicit_empty_bootstrap_paths_is_an_empty_baseline() {
        let dir = TempDir::new();
        let path = dir.write_config("[agent]\nbootstrap_paths = []\n");
        let config = load_config_from(&path).expect("explicit empty list");
        assert!(config.agent.bootstrap_paths.is_empty());
        assert!(config.agent.effective_bootstrap_paths().is_empty());
    }

    #[test]
    fn config_extend_on_default_appends_to_builtin_list() {
        let dir = TempDir::new();
        let path = dir.write_config("[agent]\nextend_bootstrap_paths = [\"docs/AGENTS.md\"]\n");
        let config = load_config_from(&path).expect("extend on default");
        let effective = config.agent.effective_bootstrap_paths();
        assert_eq!(effective.len(), 34 + 1);
        assert_eq!(effective[..34], AgentConfig::default().bootstrap_paths[..]);
        assert_eq!(effective[34], "docs/AGENTS.md");
    }

    #[test]
    fn config_extend_on_custom_is_a_union() {
        let dir = TempDir::new();
        let path = dir.write_config(
            "[agent]\n\
             bootstrap_paths = [\"AGENTS.md\", \"CLAUDE.md\"]\n\
             extend_bootstrap_paths = [\"docs/X.md\"]\n",
        );
        let config = load_config_from(&path).expect("extend on custom");
        assert_eq!(
            config.agent.effective_bootstrap_paths(),
            vec!["AGENTS.md", "CLAUDE.md", "docs/X.md"]
        );
    }

    #[test]
    fn config_empty_extend_entry_is_rejected() {
        let dir = TempDir::new();
        let path = dir.write_config("[agent]\nextend_bootstrap_paths = [\"AGENTS.md\", \"\"]\n");
        let err = load_config_from(&path).expect_err("empty extend path");
        let message = err.to_string();
        assert!(
            message.contains("agent.extend_bootstrap_paths[1]") && message.contains("empty"),
            "{message}"
        );
    }

    #[test]
    fn config_extend_deduplicates_order_preserving() {
        let dir = TempDir::new();
        let path = dir.write_config(
            "[agent]\n\
             bootstrap_paths = [\"AGENTS.md\", \"CLAUDE.md\"]\n\
             extend_bootstrap_paths = [\"CLAUDE.md\", \"docs/X.md\", \"AGENTS.md\"]\n",
        );
        let config = load_config_from(&path).expect("dedup extend");
        assert_eq!(
            config.agent.effective_bootstrap_paths(),
            vec!["AGENTS.md", "CLAUDE.md", "docs/X.md"]
        );
    }

    #[test]
    fn workspace_root_expands_leading_tilde() {
        assert_eq!(
            expand_tilde_with_home("~/code/workspaces", Some("/home/nathan")),
            "/home/nathan/code/workspaces"
        );
        assert_eq!(expand_tilde_with_home("/abs/path", Some("/home/nathan")), "/abs/path");
        assert_eq!(expand_tilde_with_home("~/x", None), "~/x");
    }

    // --- jj.command resolution -------------------------------------------------

    /// Creates a file with the given permissions (executable or not).
    fn make_file(dir: &std::path::Path, name: &str, executable: bool) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\n").expect("write file");
        use std::os::unix::fs::PermissionsExt;
        let mode = if executable { 0o755 } else { 0o644 };
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
            .expect("set permissions");
        path
    }

    #[test]
    fn default_jj_command_is_bare_jj() {
        assert_eq!(
            JjCommandValue::default(),
            JjCommandValue::Single("jj".into())
        );
    }

    #[test]
    fn resolves_bare_name_across_path_dirs_in_order() {
        let dir1 = TempDir::new();
        let dir2 = TempDir::new();
        let found = make_file(dir2.path(), "jj", true);
        // dir1 exists but has no jj; the lookup continues to dir2.
        let resolved = resolve_jj_command(
            &JjCommandValue::Single("jj".into()),
            &[dir1.path().to_path_buf(), dir2.path().to_path_buf()],
        )
        .expect("resolved from the second dir");
        assert_eq!(resolved.executable, found);
        assert!(resolved.extra_args.is_empty());
    }

    #[test]
    fn bare_name_skips_non_executable_entries() {
        let dir1 = TempDir::new();
        let dir2 = TempDir::new();
        make_file(dir1.path(), "jj", false);
        let found = make_file(dir2.path(), "jj", true);
        let resolved = resolve_jj_command(
            &JjCommandValue::Single("jj".into()),
            &[dir1.path().to_path_buf(), dir2.path().to_path_buf()],
        )
        .expect("skips the non-executable entry");
        assert_eq!(resolved.executable, found);
    }

    #[test]
    fn bare_name_not_found_reports_searched_dirs_and_fix() {
        let dir = TempDir::new();
        let err = resolve_jj_command(
            &JjCommandValue::Single("jj".into()),
            &[dir.path().to_path_buf()],
        )
        .expect_err("not on path");
        let message = err.to_string();
        assert!(message.contains("jj.command"), "{message}");
        assert!(message.contains(dir.path().to_str().unwrap()), "{message}");
        assert!(message.contains("which jj"), "{message}");
    }

    #[test]
    fn absolute_path_is_used_as_is() {
        let dir = TempDir::new();
        let path = make_file(dir.path(), "jj", true);
        let resolved = resolve_jj_command(
            &JjCommandValue::Single(path.display().to_string()),
            &[],
        )
        .expect("absolute path is validated and used as-is");
        assert_eq!(resolved.executable, path);
    }

    #[test]
    fn absolute_path_must_be_an_executable_file() {
        let dir = TempDir::new();
        let non_exec = make_file(dir.path(), "jj", false);
        let err = resolve_jj_command(
            &JjCommandValue::Single(non_exec.display().to_string()),
            &[],
        )
        .expect_err("exists but is not executable");
        assert!(err.to_string().contains("not an executable file"), "{}", err);

        let missing = dir.path().join("nope");
        let err = resolve_jj_command(
            &JjCommandValue::Single(missing.display().to_string()),
            &[],
        )
        .expect_err("missing file");
        assert!(err.to_string().contains("does not exist"), "{}", err);
    }

    #[test]
    fn relative_path_with_slash_is_rejected() {
        let err = resolve_jj_command(&JjCommandValue::Single("bin/jj".into()), &[])
            .expect_err("relative path");
        let message = err.to_string();
        assert!(message.contains("relative path"), "{message}");
        assert!(message.contains("jj.command"), "{message}");
    }

    #[test]
    fn tilde_expands_before_classification() {
        let home = TempDir::new();
        let bin = home.path().join(".local").join("bin");
        std::fs::create_dir_all(&bin).expect("create bin");
        let expected = make_file(&bin, "jj", true);
        let resolved = resolve_jj_head(
            "~/.local/bin/jj",
            Vec::new(),
            "jj.command",
            Some(home.path().to_str().unwrap()),
            &[],
        )
        .expect("tilde-expanded to an absolute path");
        assert_eq!(resolved.executable, expected);
    }

    #[test]
    fn argv_form_resolves_only_the_head() {
        let dir = TempDir::new();
        let found = make_file(dir.path(), "jj", true);
        let resolved = resolve_jj_command(
            &JjCommandValue::Argv(vec!["jj".into(), "--at-op".into(), "@-".into()]),
            &[dir.path().to_path_buf()],
        )
        .expect("argv form resolves argv[0] only");
        assert_eq!(resolved.executable, found);
        assert_eq!(resolved.extra_args, vec!["--at-op", "@-"]);

        let err = resolve_jj_command(
            &JjCommandValue::Argv(vec!["nope".into()]),
            &[dir.path().to_path_buf()],
        )
        .expect_err("argv head not found");
        assert!(err.to_string().contains("argv[0]"), "{}", err);
    }

    #[test]
    fn config_jj_command_accepts_string_and_argv() {
        let dir = TempDir::new();
        let path = dir.write_config("[jj]\ncommand = \"/opt/jj/bin/jj\"\n");
        let config = load_config_from(&path).expect("string form");
        assert_eq!(
            config.jj.command,
            JjCommandValue::Single("/opt/jj/bin/jj".into())
        );

        let path = dir.write_config("[jj]\ncommand = [\"/opt/jj\", \"--at-op\", \"@-\"]\n");
        let config = load_config_from(&path).expect("argv form");
        assert_eq!(
            config.jj.command,
            JjCommandValue::Argv(vec!["/opt/jj".into(), "--at-op".into(), "@-".into()])
        );
    }

    #[test]
    fn config_jj_command_type_error_names_the_key() {
        let dir = TempDir::new();
        let path = dir.write_config("[jj]\ncommand = 123\n");
        let err = load_config_from(&path).expect_err("type error");
        assert!(err.to_string().contains("jj.command"), "{}", err);
    }

    #[test]
    fn config_empty_jj_command_is_rejected() {
        let dir = TempDir::new();
        let path = dir.write_config("[jj]\ncommand = \"\"\n");
        let err = load_config_from(&path).expect_err("empty string");
        assert!(err.to_string().contains("jj.command"), "{}", err);

        let path = dir.write_config("[jj]\ncommand = []\n");
        let err = load_config_from(&path).expect_err("empty argv");
        assert!(err.to_string().contains("jj.command"), "{}", err);
    }

    #[test]
    fn command_defaults_to_opencode_when_unset() {
        assert_eq!(resolve_start_command(&AgentConfig::default()), "opencode");
    }

    #[test]
    fn start_command_uses_configured_value() {
        let agent = AgentConfig {
            command: "opencode".into(),
            ..AgentConfig::default()
        };
        assert_eq!(resolve_start_command(&agent), "opencode");
    }

    #[test]
    fn start_command_passes_arguments_through_verbatim() {
        let agent = AgentConfig {
            command: "codex --full-auto".into(),
            ..AgentConfig::default()
        };
        assert_eq!(resolve_start_command(&agent), "codex --full-auto");
    }

    /// Runs the shipped setup script inside a fake plugin layout
    /// (`<root>/scripts/setup-workspace.sh` + `<root>/target/release/jj-workspace`
    /// + a fake `jj` on PATH) so both the script's self-located plugin exe and
    /// its jj invocations are asserted against real execution.
    #[cfg(unix)]
    fn run_setup_script(
        bookmark_name: &str,
        base_rev: &str,
        fail_on: &str,
        leading_args: &[&str],
    ) -> (std::process::Output, Vec<String>, Vec<String>) {
        use std::os::unix::fs::PermissionsExt;

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock before epoch")
            .as_nanos();
        let root = std::env::temp_dir()
            .join(format!("jj-workspace-setup-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(root.join("scripts")).expect("create scripts dir");
        std::fs::create_dir_all(root.join("target/release")).expect("create target dir");
        std::fs::create_dir_all(root.join("bin")).expect("create bin dir");

        std::fs::copy(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/setup-workspace.sh"),
            root.join("scripts/setup-workspace.sh"),
        )
        .expect("copy setup script");

        // Fake plugin exe: logs its argv and exits 0. Proves the script
        // located it via `$0`'s layout (`<root>/target/release/jj-workspace`).
        let finish_log = root.join("finish.log");
        let plugin_exe = root.join("target/release/jj-workspace");
        std::fs::write(&plugin_exe, "#!/bin/sh\necho \"$@\" >> \"$FINISH_LOG\"\n")
            .expect("write fake plugin exe");
        // Sync before the script spawns the fakes: exec'ing a freshly written
        // script can race the kernel's write-open tracking and fail with
        // ETXTBSY ("Text file busy", rust-lang/rust #114554).
        std::fs::File::open(&plugin_exe)
            .and_then(|file| file.sync_all())
            .expect("sync fake plugin exe");
        std::fs::set_permissions(&plugin_exe, std::fs::Permissions::from_mode(0o755))
            .expect("make fake plugin exe executable");

        // Fake jj: logs argv, fails on a chosen subcommand.
        let jj_log = root.join("jj.log");
        let jj = root.join("bin/jj");
        std::fs::write(
            &jj,
            format!(
                "#!/bin/sh\n\
                 echo \"$@\" >> \"$JJ_FAKE_LOG\"\n\
                 case \"$1\" in\n\
                   {fail_on}) exit 1 ;;\n\
                   sparse|bookmark|fetch|rebase) exit 0 ;;\n\
                   *) exit 0 ;;\n\
                 esac\n"
            ),
        )
        .expect("write fake jj");
        std::fs::File::open(&jj)
            .and_then(|file| file.sync_all())
            .expect("sync fake jj");
        std::fs::set_permissions(&jj, std::fs::Permissions::from_mode(0o755))
            .expect("make fake jj executable");

        let script = root.join("scripts/setup-workspace.sh");
        let mut command = std::process::Command::new("sh");
        command
            .arg(&script)
            .arg(&jj)
            .arg(base_rev)
            .arg(bookmark_name)
            .arg("w")
            .arg("t")
            .arg("p");
        for arg in leading_args {
            command.arg(arg);
        }
        let output = command
            .env("JJ_FAKE_LOG", &jj_log)
            .env("FINISH_LOG", &finish_log)
            .output()
            .expect("run setup script");

        let jj_lines = std::fs::read_to_string(&jj_log)
            .map(|content| content.lines().map(str::to_string).collect::<Vec<_>>())
            .unwrap_or_default();
        // The finish-tab watcher is launched asynchronously; poll briefly for
        // its log line before asserting (the fake exe writes near-instantly).
        let finish_lines = (0..40)
            .find_map(|_| {
                std::fs::read_to_string(&finish_log)
                    .ok()
                    .filter(|c| !c.is_empty())
                    .map(|c| c.lines().map(str::to_string).collect::<Vec<_>>())
            })
            .unwrap_or_default();
        let _ = std::fs::remove_dir_all(&root);
        (output, jj_lines, finish_lines)
    }

    #[test]
    #[cfg(unix)]
    fn setup_runs_materialize_bookmark_fetch_rebase_in_order() {
        let (output, log, finish) = run_setup_script("plain-name", "trunk()", "__never__", &[]);
        assert!(output.status.success());
        assert_eq!(
            log,
            vec![
                "sparse set --clear --add .",
                "bookmark create plain-name -r @",
                "git fetch",
                "rebase -s @ -d trunk()",
            ]
        );
        // The plugin exe was self-located via the script's `$0` layout.
        assert_eq!(finish, vec!["finish-tab w t p"]);
    }

    #[test]
    #[cfg(unix)]
    fn setup_materialization_failure_stops_the_chain() {
        let (output, log, _) = run_setup_script("workspace/x", "trunk()", "sparse", &[]);
        assert!(!output.status.success());
        assert_eq!(log, vec!["sparse set --clear --add ."]);
    }

    #[test]
    #[cfg(unix)]
    fn setup_bookmark_failure_warns_and_keeps_updating() {
        let (output, log, _) = run_setup_script("workspace/fix-api", "trunk()", "bookmark", &[]);
        // The `|| printf` fallback makes the bookmark step succeed, so
        // fetch/rebase still run and the whole script exits 0.
        assert!(output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(
                "warning: could not create bookmark workspace/fix-api (workspace still created)"
            ),
            "stderr should carry the bookmark warning: {stderr}"
        );
        assert_eq!(
            log,
            vec![
                "sparse set --clear --add .",
                "bookmark create workspace/fix-api -r @",
                "git fetch",
                "rebase -s @ -d trunk()",
            ]
        );
    }

    #[test]
    #[cfg(unix)]
    fn setup_preserves_single_quotes_in_bookmark_names() {
        let (output, log, _) = run_setup_script("it's-final", "trunk()", "__never__", &[]);
        assert!(output.status.success());
        assert_eq!(
            log,
            vec![
                "sparse set --clear --add .",
                "bookmark create it's-final -r @",
                "git fetch",
                "rebase -s @ -d trunk()",
            ]
        );
    }

    #[test]
    #[cfg(unix)]
    fn setup_propagates_leading_args_before_each_subcommand() {
        // argv-form `jj.command` leading arguments ride as trailing script
        // arguments and must be inserted before every jj subcommand.
        let (output, log, _) =
            run_setup_script("plain-name", "trunk()", "__never__", &["--at-op", "@-"]);
        assert!(output.status.success());
        assert_eq!(
            log,
            vec![
                "--at-op @- sparse set --clear --add .",
                "--at-op @- bookmark create plain-name -r @",
                "--at-op @- git fetch",
                "--at-op @- rebase -s @ -d trunk()",
            ]
        );
    }

    #[test]
    #[cfg(unix)]
    fn setup_passes_base_rev_to_rebase() {
        // The base-rev argument flows through to the rebase destination
        // (parameterized by per-repo-base-rev; here it is whatever was passed).
        let (output, log, _) = run_setup_script("w", "dev@origin", "__never__", &[]);
        assert!(output.status.success());
        assert!(
            log.iter().any(|line| line == "rebase -s @ -d dev@origin"),
            "rebase should target the passed base-rev: {log:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn setup_passes_revsets_with_spaces_and_quotes() {
        // Revsets are shell-quoted per argument by the wizard, so spaces and
        // single quotes survive the pane shell and arrive as one argv word.
        let (output, log, _) =
            run_setup_script("w", "trunk() | remote_bookmark(dev)", "__never__", &[]);
        assert!(output.status.success());
        assert!(
            log.iter()
                .any(|line| line == "rebase -s @ -d trunk() | remote_bookmark(dev)"),
            "space-containing revset must survive: {log:?}"
        );

        let (output, log, _) = run_setup_script("w", "description(\"fix'it\")", "__never__", &[]);
        assert!(output.status.success());
        assert!(
            log.iter()
                .any(|line| line == "rebase -s @ -d description(\"fix'it\")"),
            "quote-containing revset must survive: {log:?}"
        );
    }

    #[test]
    fn wizard_fields_cycle_through_three_fields() {
        assert_eq!(
            next_wizard_field(WizardField::WorkspaceSearch),
            WizardField::Name
        );
        assert_eq!(next_wizard_field(WizardField::Name), WizardField::Base);
        assert_eq!(
            next_wizard_field(WizardField::Base),
            WizardField::WorkspaceSearch
        );
    }

    // --- name field component-level editing (workspace-wizard: name 字段组件级编辑)

    fn name_char(name: &str, state: NameEditState, c: char) -> (String, NameEditState) {
        let mut n = name.to_string();
        let mut s = state;
        apply_name_key(&mut n, &mut s, NameKey::Char(c));
        (n, s)
    }

    fn name_backspace(name: &str, state: NameEditState) -> (String, NameEditState) {
        let mut n = name.to_string();
        let mut s = state;
        apply_name_key(&mut n, &mut s, NameKey::Backspace);
        (n, s)
    }

    #[test]
    fn name_fresh_char_keeps_prefix_and_replaces_slug() {
        let (n, s) = name_char("workspace/brave-river-0000", NameEditState::Fresh, 'f');
        assert_eq!(n, "workspace/f");
        assert_eq!(s, NameEditState::Free);
    }

    #[test]
    fn name_free_chars_append_after_first_keystroke() {
        let (n, s) = name_char("workspace/f", NameEditState::Free, 'i');
        assert_eq!(n, "workspace/fi");
        assert_eq!(s, NameEditState::Free);
        let (n, s) = name_char(&n, s, 'x');
        assert_eq!(n, "workspace/fix");
        assert_eq!(s, NameEditState::Free);
    }

    #[test]
    fn name_fresh_backspace_drops_slug_keeps_prefix() {
        let (n, s) = name_backspace("workspace/brave-river-0000", NameEditState::Fresh);
        assert_eq!(n, "workspace/");
        assert_eq!(s, NameEditState::Prefixed);
    }

    #[test]
    fn name_prefixed_backspace_clears_prefix() {
        let (n, s) = name_backspace("workspace/", NameEditState::Prefixed);
        assert_eq!(n, "");
        assert_eq!(s, NameEditState::Free);
    }

    #[test]
    fn name_free_backspace_pops_one_char() {
        // Typo-fix case: a per-char delete must not wipe the whole slug.
        let (n, s) = name_backspace("workspace/fix-ap1", NameEditState::Free);
        assert_eq!(n, "workspace/fix-ap");
        assert_eq!(s, NameEditState::Free);
    }

    #[test]
    fn name_double_backspace_then_free_typed_unprefixed_name() {
        let (n1, s1) = name_backspace("workspace/brave-river-0000", NameEditState::Fresh);
        assert_eq!((n1.as_str(), s1), ("workspace/", NameEditState::Prefixed));
        let (n2, s2) = name_backspace(&n1, s1);
        assert_eq!((n2.as_str(), s2), ("", NameEditState::Free));
        let (n3, s3) = name_char(&n2, s2, 'f');
        assert_eq!((n3.as_str(), s3), ("f", NameEditState::Free));
    }

    #[test]
    fn name_multi_segment_user_name_edits_per_char() {
        // User-typed names never get component-level deletion.
        let (n, s) = name_backspace("feature/foo", NameEditState::Free);
        assert_eq!(n, "feature/fo");
        assert_eq!(s, NameEditState::Free);
    }

    #[test]
    fn name_fresh_without_slash_degrades_to_clear() {
        // Defensive: Fresh only arises from the auto-generated default, which
        // always contains a '/'; without one, behavior matches the old
        // replace-on-type semantics.
        let (n, s) = name_backspace("default", NameEditState::Fresh);
        assert_eq!(n, "");
        assert_eq!(s, NameEditState::Free);
    }

    #[test]
    fn jj_root_returns_the_root_when_it_carries_jj() {
        let dir = TempDir::new();
        let repo = dir.path().join("repo");
        fs::create_dir_all(repo.join(".jj")).expect("create .jj");
        assert_eq!(
            jj_root(repo.to_str().unwrap()),
            Some(repo.display().to_string())
        );
    }

    #[test]
    fn jj_root_normalizes_subdirectories_up_to_the_jj_root() {
        let dir = TempDir::new();
        let repo = dir.path().join("repo");
        let nested = repo.join("a/b/c");
        fs::create_dir_all(&nested).expect("create nested dirs");
        fs::create_dir_all(repo.join(".jj")).expect("create .jj");
        // A pane sitting deep inside the repository resolves to the root.
        assert_eq!(
            jj_root(nested.to_str().unwrap()),
            Some(repo.display().to_string())
        );
    }

    #[test]
    fn jj_root_returns_none_without_a_jj_ancestor() {
        let dir = TempDir::new();
        let deep = dir.path().join("x/y/z");
        fs::create_dir_all(&deep).expect("create dirs");
        assert_eq!(jj_root(deep.to_str().unwrap()), None);
    }

    /// Fake herdr that answers `workspace list` / `pane list` from two JSON
    /// fixture files, for exercising candidate loading and jj filtering.
    #[cfg(unix)]
    fn make_fake_herdr_listing(dir: &Path, workspaces: &str, panes: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        fs::write(dir.join("workspaces.json"), workspaces).expect("write workspaces fixture");
        fs::write(dir.join("panes.json"), panes).expect("write panes fixture");
        let bin = dir.join("herdr");
        fs::write(
            &bin,
            "#!/bin/sh\n\
             D=$(CDPATH= cd -- \"$(dirname -- \"$0\")\" && pwd)\n\
             case \"$1 $2\" in\n\
               \"workspace list\") cat \"$D/workspaces.json\" ;;\n\
               \"pane list\") cat \"$D/panes.json\" ;;\n\
               *) exit 1 ;;\n\
             esac\n",
        )
        .expect("write fake herdr");
        fs::File::open(&bin)
            .and_then(|file| file.sync_all())
            .expect("sync fake herdr");
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755))
            .expect("make fake herdr executable");
        bin.display().to_string()
    }

    #[test]
    #[cfg(unix)]
    fn workspace_choices_keep_jj_roots_and_filter_everything_else() {
        let dir = TempDir::new();
        // jj repo whose checkout_path is present (the root carries `.jj`).
        let repo_a = dir.path().join("alpha");
        fs::create_dir_all(repo_a.join(".jj")).expect("create alpha .jj");
        // jj repo WITHOUT a checkout_path: falls back to the active pane's
        // cwd, which sits deep inside the repo — normalized up to the root.
        let repo_b = dir.path().join("beta");
        let pane_cwd_b = repo_b.join("sub/deep");
        fs::create_dir_all(&pane_cwd_b).expect("create beta pane dirs");
        fs::create_dir_all(repo_b.join(".jj")).expect("create beta .jj");
        // Plain directory with a checkout_path but no `.jj` on any ancestor.
        let repo_c = dir.path().join("gamma");
        fs::create_dir_all(&repo_c).expect("create gamma dir");

        let workspaces = format!(
            r#"{{"result":{{"workspaces":[
                {{"workspace_id":"w1","label":"alpha","active_tab_id":"w1:t1",
                  "worktree":{{"checkout_path":"{a}"}}}},
                {{"workspace_id":"w2","label":"beta","active_tab_id":"w2:t1",
                  "worktree":{{}}}},
                {{"workspace_id":"w3","label":"gamma","active_tab_id":"w3:t1",
                  "worktree":{{"checkout_path":"{c}"}}}}
            ]}}}}"#,
            a = repo_a.display(),
            c = repo_c.display(),
        );
        let panes = format!(
            r#"{{"result":{{"panes":[
                {{"workspace_id":"w2","tab_id":"w2:t1","focused":true,
                  "foreground_cwd":"{deep}"}}
            ]}}}}"#,
            deep = pane_cwd_b.display(),
        );
        let herdr = make_fake_herdr_listing(dir.path(), &workspaces, &panes);
        let choices = load_workspace_choices_with(&herdr).expect("choices load");
        // w1 keeps its checkout path; w2's pane subdirectory is normalized to
        // the beta root; w3 (no `.jj` anywhere) is filtered out entirely.
        let got: Vec<(String, String, String)> = choices
            .iter()
            .map(|c| (c.id.clone(), c.label.clone(), c.path.clone()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("w1".into(), "alpha".into(), repo_a.display().to_string()),
                ("w2".into(), "beta".into(), repo_b.display().to_string()),
            ]
        );
    }

    #[test]
    #[cfg(unix)]
    fn workspace_choices_are_empty_when_no_workspace_is_a_jj_repo() {
        let dir = TempDir::new();
        let plain = dir.path().join("plain");
        fs::create_dir_all(&plain).expect("create plain dir");
        let workspaces = format!(
            r#"{{"result":{{"workspaces":[
                {{"workspace_id":"w1","label":"plain","active_tab_id":"w1:t1",
                  "worktree":{{"checkout_path":"{p}"}}}}
            ]}}}}"#,
            p = plain.display(),
        );
        let herdr = make_fake_herdr_listing(dir.path(), &workspaces, r#"{"result":{"panes":[]}}"#);
        assert!(
            load_workspace_choices_with(&herdr)
                .expect("an all-non-jj herd resolves to an empty choice list")
                .is_empty()
        );
    }

    /// Writes a fake jj executable that runs `body` and returns a ResolvedJj
    /// pointing at it, for exercising the base-rev resolution chain without
    /// touching process env. The file is synced before the spawn: executing a
    /// freshly written script can otherwise race the kernel's write-open
    /// tracking and fail with ETXTBSY ("Text file busy", rust-lang/rust
    /// #114554), especially with concurrent spawns in one test process.
    #[cfg(unix)]
    fn make_fake_jj(dir: &Path, name: &str, body: &str) -> ResolvedJj {
        use std::os::unix::fs::PermissionsExt;

        let jj = dir.join(name);
        std::fs::write(&jj, format!("#!/bin/sh\n{body}")).expect("write fake jj");
        std::fs::File::open(&jj)
            .and_then(|file| file.sync_all())
            .expect("sync fake jj");
        std::fs::set_permissions(&jj, std::fs::Permissions::from_mode(0o755))
            .expect("make fake jj executable");
        ResolvedJj {
            executable: jj,
            extra_args: Vec::new(),
        }
    }

    #[test]
    #[cfg(unix)]
    fn base_rev_resolves_from_repo_config() {
        let dir = TempDir::new();
        let jj = make_fake_jj(dir.path(), "jj", "echo dev@origin\n");
        let config = Config::default();
        assert_eq!(
            resolve_base_rev(&config, &jj, dir.path()).expect("repo config value"),
            "dev@origin"
        );
    }

    #[test]
    #[cfg(unix)]
    fn base_rev_falls_back_to_config_when_unset() {
        let dir = TempDir::new();
        let jj = make_fake_jj(dir.path(), "jj", "exit 1\n");
        let config = Config::default();
        assert_eq!(
            resolve_base_rev(&config, &jj, dir.path()).expect("fallback value"),
            "trunk()"
        );

        let mut configured = Config::default();
        configured.jj.base_rev = "main@origin".into();
        assert_eq!(
            resolve_base_rev(&configured, &jj, dir.path()).expect("config value"),
            "main@origin"
        );
    }

    #[test]
    #[cfg(unix)]
    fn base_rev_empty_repo_value_fails_fast() {
        // `jj config get` succeeding with empty output = explicitly empty
        // repo-level value → invalid config, not a silent fallback.
        let dir = TempDir::new();
        let jj = make_fake_jj(dir.path(), "jj", "exit 0\n");
        let config = Config::default();
        let err = resolve_base_rev(&config, &jj, dir.path())
            .expect_err("empty repo-level value must fail fast");
        assert!(err.contains("herdr.base-rev"), "{err}");
        assert!(err.contains("empty"), "{err}");
    }

    #[test]
    #[cfg(unix)]
    fn wizard_final_base_rev_uses_chain_when_untouched() {
        let dir = TempDir::new();
        let jj = make_fake_jj(dir.path(), "jj", "echo repo-value\n");
        let config = Config::default();
        assert_eq!(
            wizard_final_base_rev(&config, &jj, dir.path(), "trunk()", false)
                .expect("untouched field resolves lazily"),
            "repo-value"
        );
    }

    #[test]
    #[cfg(unix)]
    fn wizard_final_base_rev_validates_edited_values() {
        let dir = TempDir::new();
        let ok = make_fake_jj(dir.path(), "ok-jj", "exit 0\n");
        let bad = make_fake_jj(
            dir.path(),
            "bad-jj",
            "echo \"Error: Revision 'dev@origin' doesn't exist\" >&2\nexit 1\n",
        );
        let config = Config::default();
        assert_eq!(
            wizard_final_base_rev(&config, &ok, dir.path(), "dev@origin", true)
                .expect("valid revset passes"),
            "dev@origin"
        );
        let err = wizard_final_base_rev(&config, &bad, dir.path(), "dev@origin", true)
            .expect_err("invalid revset must fail");
        assert!(err.contains("doesn't exist"), "{err}");
    }

    #[test]
    #[cfg(unix)]
    fn wizard_final_base_rev_validates_chain_values_too() {
        // Regression guard: a repo-level `herdr.base-rev` referencing a
        // nonexistent revision must fail at submit (inside the wizard, where
        // the user can edit the base field), not later at `jj workspace add`.
        let dir = TempDir::new();
        let jj = make_fake_jj(
            dir.path(),
            "jj",
            "if [ \"$1\" = \"config\" ]; then echo fwggowngggw\n\
             else echo \"Error: Revision 'fwggowngggw' doesn't exist\" >&2\n\
             exit 1\n\
             fi\n",
        );
        let config = Config::default();
        let err = wizard_final_base_rev(&config, &jj, dir.path(), "trunk()", false)
            .expect_err("invalid chain value must surface at submit");
        assert!(err.contains("doesn't exist"), "{err}");
    }

    /// Creates a fake herdr executable that answers `agent list` from a
    /// schedule file — one `"<status> <label>"` line per poll (`missing` =
    /// empty agents list; label defaults to codex; exhausted schedule keeps
    /// answering `missing`) — and logs `pane send-keys` calls to keys.log.
    #[cfg(unix)]
    fn make_fake_herdr(dir: &Path, schedule: &[&str]) -> String {
        use std::os::unix::fs::PermissionsExt;

        let script = r#"#!/bin/sh
D=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
case "$1 $2" in
  "agent list")
    N=$(($(cat "$D/count" 2>/dev/null || echo 0) + 1))
    echo "$N" > "$D/count"
    LINE=$(sed -n "${N}p" "$D/schedule")
    STATUS=$(echo "$LINE" | awk '{print $1}')
    LABEL=$(echo "$LINE" | awk '{print $2}')
    [ -z "$LABEL" ] && LABEL=codex
    if [ "$STATUS" = "missing" ] || [ -z "$STATUS" ]; then
      echo '{"result":{"agents":[]}}'
    else
      printf '{"result":{"agents":[{"pane_id":"p1","agent":"%s","agent_status":"%s"}]}}\n' "$LABEL" "$STATUS"
    fi
    ;;
  "pane send-keys")
    echo "$3 $4" >> "$D/keys.log"
    ;;
esac
"#;
        let bin = dir.join("herdr");
        std::fs::write(&bin, script).expect("write fake herdr");
        // Sync before spawn: executing a freshly written script can race the
        // kernel's write-open tracking and fail with ETXTBSY ("Text file
        // busy", rust-lang/rust #114554).
        std::fs::File::open(&bin)
            .and_then(|file| file.sync_all())
            .expect("sync fake herdr");
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755))
            .expect("make fake herdr executable");
        std::fs::write(dir.join("schedule"), schedule.join("\n")).expect("write schedule");
        bin.display().to_string()
    }

    /// Runs `wait_for_agent_ready` against a fake herdr with the given
    /// schedule and agent config; returns the outcome and the send-keys log.
    #[cfg(unix)]
    fn run_wait(
        schedule: &[&str],
        auto_trust: bool,
        trust_window_secs: u64,
        timeout_secs: u64,
        poll_ms: u64,
    ) -> (AgentWaitOutcome, Vec<String>) {
        let dir = TempDir::new();
        let herdr = make_fake_herdr(dir.path(), schedule);
        let mut agent = AgentConfig::default();
        agent.auto_trust = auto_trust;
        agent.trust_window_secs = trust_window_secs;
        agent.startup_timeout_secs = timeout_secs;
        agent.poll_interval_ms = poll_ms;
        let outcome = wait_for_agent_ready(&agent, &herdr, "p1");
        let keys = std::fs::read_to_string(dir.path().join("keys.log"))
            .map(|content| content.lines().map(str::to_string).collect())
            .unwrap_or_default();
        (outcome, keys)
    }

    #[test]
    #[cfg(unix)]
    fn agent_entry_ready_immediately() {
        // The entry itself is readiness: no stable-period wait, no specific
        // status required, no keys sent.
        let (outcome, keys) = run_wait(&["working"], false, 10, 1, 10);
        assert!(matches!(outcome, AgentWaitOutcome::Ready), "{outcome:?}");
        assert!(keys.is_empty(), "{keys:?}");
    }

    #[test]
    #[cfg(unix)]
    fn agent_not_detected_times_out() {
        let (outcome, keys) = run_wait(&["missing"], false, 10, 1, 10);
        assert!(matches!(outcome, AgentWaitOutcome::NotDetected), "{outcome:?}");
        assert!(keys.is_empty(), "{keys:?}");
    }

    #[test]
    #[cfg(unix)]
    fn transient_blocked_does_not_fail() {
        // A single blocked poll inside the grace window recovers on its own.
        let (outcome, keys) = run_wait(&["blocked", "working"], false, 10, 2, 10);
        assert!(matches!(outcome, AgentWaitOutcome::Ready), "{outcome:?}");
        assert!(keys.is_empty(), "{keys:?}");
    }

    #[test]
    #[cfg(unix)]
    fn stable_blocked_fails_with_runtime_label() {
        let (outcome, keys) =
            run_wait(&["blocked claude"; 200], false, 10, 3, 10);
        assert!(
            matches!(outcome, AgentWaitOutcome::NeedsAttention { ref label } if label == "claude"),
            "{outcome:?}"
        );
        assert!(keys.is_empty(), "{keys:?}");
    }

    #[test]
    #[cfg(unix)]
    fn auto_trust_off_never_enters() {
        let (outcome, keys) =
            run_wait(&["blocked codex"; 200], false, 10, 3, 10);
        assert!(
            matches!(outcome, AgentWaitOutcome::NeedsAttention { ref label } if label == "codex"),
            "{outcome:?}"
        );
        assert!(keys.is_empty(), "{keys:?}");
    }

    #[test]
    #[cfg(unix)]
    fn auto_trust_non_codex_never_enters() {
        let (outcome, keys) =
            run_wait(&["blocked claude"; 200], true, 10, 3, 10);
        assert!(
            matches!(outcome, AgentWaitOutcome::NeedsAttention { ref label } if label == "claude"),
            "{outcome:?}"
        );
        assert!(keys.is_empty(), "{keys:?}");
    }

    #[test]
    #[cfg(unix)]
    fn auto_trust_codex_enters_and_confirms_by_transition() {
        let (outcome, keys) = run_wait(&["blocked codex", "working codex"], true, 10, 2, 10);
        assert!(matches!(outcome, AgentWaitOutcome::Ready), "{outcome:?}");
        assert_eq!(keys, vec!["p1 enter"]);
    }

    #[test]
    #[cfg(unix)]
    fn auto_trust_retries_up_to_internal_cap() {
        let (outcome, keys) =
            run_wait(&["blocked codex"; 200], true, 10, 3, 10);
        assert!(
            matches!(outcome, AgentWaitOutcome::NeedsAttention { ref label } if label == "codex"),
            "{outcome:?}"
        );
        assert_eq!(keys.len(), 5, "{keys:?}");
    }

    #[test]
    #[cfg(unix)]
    fn auto_trust_outside_window_does_not_enter() {
        // The blocked status only appears after the trust window has passed.
        let mut schedule = vec!["missing"; 20];
        schedule.extend(std::iter::repeat("blocked codex").take(100));
        let (outcome, keys) = run_wait(&schedule, true, 1, 4, 100);
        assert!(
            matches!(outcome, AgentWaitOutcome::NeedsAttention { ref label } if label == "codex"),
            "{outcome:?}"
        );
        assert!(keys.is_empty(), "{keys:?}");
    }

    #[test]
    fn agent_ready_defaults() {
        let agent = AgentConfig::default();
        assert!(!agent.auto_trust);
        assert_eq!(agent.trust_window_secs, 10);
        assert_eq!(agent.startup_timeout_secs, 20);
        assert_eq!(agent.poll_interval_ms, 200);
    }

    #[test]
    fn agent_ready_keys_parse() {
        let dir = TempDir::new();
        let path = dir.write_config(
            "[agent]\nauto_trust = true\ntrust_window_secs = 5\nstartup_timeout_secs = 30\npoll_interval_ms = 100\n",
        );
        let config = load_config_from(&path).expect("parse agent readiness keys");
        assert!(config.agent.auto_trust);
        assert_eq!(config.agent.trust_window_secs, 5);
        assert_eq!(config.agent.startup_timeout_secs, 30);
        assert_eq!(config.agent.poll_interval_ms, 100);
    }

    #[test]
    fn agent_timing_zero_rejected() {
        let dir = TempDir::new();
        let path = dir.write_config("[agent]\npoll_interval_ms = 0\n");
        let err = load_config_from(&path).expect_err("zero poll interval");
        assert!(err.to_string().contains("agent.poll_interval_ms"), "{err}");

        let path = dir.write_config("[agent]\ntrust_window_secs = 0\n");
        let err = load_config_from(&path).expect_err("zero trust window");
        assert!(err.to_string().contains("agent.trust_window_secs"), "{err}");

        let path = dir.write_config("[agent]\nstartup_timeout_secs = 0\n");
        let err = load_config_from(&path).expect_err("zero timeout");
        assert!(err.to_string().contains("agent.startup_timeout_secs"), "{err}");
    }

    #[test]
    fn agent_auto_trust_type_error_names_key() {
        let dir = TempDir::new();
        let path = dir.write_config("[agent]\nauto_trust = \"yes\"\n");
        let err = load_config_from(&path).expect_err("type error");
        assert!(err.to_string().contains("auto_trust"), "{err}");
    }

    #[test]
    #[cfg(unix)]
    fn remove_dirty_workspace_is_refused_with_guidance() {
        let dir = TempDir::new();
        let jj = make_fake_jj(dir.path(), "jj", "echo 'A new-file.txt'\n");
        let err = check_remove_clean(&jj, dir.path())
            .expect_err("dirty workspace must be refused");
        assert!(err.contains("uncommitted changes"), "{err}");
        assert!(err.contains("bookmarks are safe"), "{err}");
        assert!(err.contains("`jj commit`"), "{err}");
        assert!(err.contains("`jj restore`"), "{err}");
        assert!(err.contains("refusing to remove"), "{err}");
    }

    #[test]
    #[cfg(unix)]
    fn remove_clean_workspace_is_allowed() {
        let dir = TempDir::new();
        let jj = make_fake_jj(dir.path(), "jj", "");
        assert!(
            check_remove_clean(&jj, dir.path()).is_ok(),
            "clean workspace must pass the check"
        );
    }

    #[test]
    #[cfg(unix)]
    fn remove_check_failure_is_fail_closed() {
        let dir = TempDir::new();
        let jj = make_fake_jj(dir.path(), "jj", "exit 3\n");
        let err = check_remove_clean(&jj, dir.path())
            .expect_err("a failed check must refuse, not pass");
        assert!(err.contains("cannot check"), "{err}");
        assert!(err.contains("refusing to remove"), "{err}");
    }

    #[test]
    fn die_toast_body_first_line_only_with_log_pointer() {
        let message = "refusing to remove workspace 'ws': it has uncommitted changes.\n\
                       workspace path: /long/path/to/ws\n\
                       Already-committed work is safe.";
        let body = die_toast_body(message, Some("/state/error.log"));
        assert_eq!(
            body,
            "refusing to remove workspace 'ws': it has uncommitted changes. \
             — full log: /state/error.log"
        );
    }

    #[test]
    fn die_toast_body_truncates_long_first_lines() {
        let message = "x".repeat(300);
        let body = die_toast_body(&message, None);
        assert!(body.chars().count() <= 121, "{}", body.chars().count());
        assert!(body.ends_with('…'), "{body}");
    }

    #[test]
    fn die_toast_body_without_log_falls_back_to_summary() {
        let body = die_toast_body("short error", None);
        assert_eq!(body, "short error");
    }

    #[test]
    fn error_log_appends_entries_with_timestamps() {
        let dir = TempDir::new();
        let log = dir.path().join("error.log");
        assert!(append_error_log(&log, "first failure"));
        assert!(append_error_log(&log, "second failure"));
        let content = fs::read_to_string(&log).expect("read log");
        let lines: Vec<&str> = content.lines().collect();
        // One line per entry: [timestamp] message.
        assert_eq!(lines.len(), 2, "{content}");
        assert!(lines[0].starts_with('[') && lines[0].contains("UTC"), "{content}");
        assert!(lines[0].ends_with("first failure"), "{content}");
        assert!(lines[1].starts_with('[') && lines[1].contains("UTC"), "{content}");
        assert!(lines[1].ends_with("second failure"), "{content}");
    }

    #[test]
    fn error_log_escapes_multiline_messages_to_single_lines() {
        let dir = TempDir::new();
        let log = dir.path().join("error.log");
        assert!(append_error_log(
            &log,
            "refusing to remove workspace 'ws': it has uncommitted changes.\n\
             workspace path: /long/path/to/ws\n\
             Commit (`jj commit`) or undo (`jj restore`) first."
        ));
        let content = fs::read_to_string(&log).expect("read log");
        let lines: Vec<&str> = content.lines().collect();
        // The multi-line message collapses to ONE log line with literal \n.
        assert_eq!(lines.len(), 1, "{content}");
        assert!(lines[0].contains("\\n"), "{content}");
        assert!(lines[0].contains("refusing to remove"), "{content}");
        assert!(!content.contains("\n\n"), "{content}");
    }

    #[test]
    fn unix_timestamp_formatting_matches_known_dates() {
        assert_eq!(format_unix_timestamp(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(format_unix_timestamp(86_400), "1970-01-02 00:00:00 UTC");
        // 2026-09-09 00:00:00 UTC = 1788912000.
        assert_eq!(format_unix_timestamp(1_788_912_000), "2026-09-09 00:00:00 UTC");
        // Leap-year day: 2024-02-29 12:34:56 UTC = 1709210096.
        assert_eq!(format_unix_timestamp(1_709_210_096), "2024-02-29 12:34:56 UTC");
    }

    // --- wizard render tests --------------------------------------------
    //
    // draw_workspace_wizard is a pure function over a Frame; a TestBackend
    // captures the frame buffer so the section layout, title grammar, shared
    // content indent, hint line and the three state branches are assertable
    // without a TTY.

    use ratatui::backend::TestBackend;

    fn wizard_choice(id: &str, label: &str, path: &str) -> WorkspaceChoice {
        WorkspaceChoice {
            id: id.into(),
            label: label.into(),
            path: path.into(),
        }
    }

    fn wizard_view<'a>(
        choices: &'a [WorkspaceChoice],
        filtered: &'a [usize],
        selected: usize,
        field: WizardField,
        query: &'a str,
        name: &'a str,
        base: &'a str,
        error: Option<&'a str>,
    ) -> WizardView<'a> {
        WizardView {
            choices,
            filtered,
            selected,
            field,
            query,
            name,
            base,
            root: Path::new("/tmp/wizard-root"),
            error,
        }
    }

    /// Renders the wizard at 90x30 into a buffer and returns it.
    fn render_wizard(view: WizardView<'_>) -> ratatui::buffer::Buffer {
        let backend = TestBackend::new(90, 30);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| draw_workspace_wizard(frame, &view))
            .expect("draw wizard");
        terminal.backend().buffer().clone()
    }

    /// Extracts one line of the buffer as a plain string (trailing spaces trimmed).
    fn line_text(buffer: &ratatui::buffer::Buffer, y: u16) -> String {
        let mut text = String::new();
        for x in 0..buffer.area.width {
            let cell = &buffer[(x, y)];
            text.push_str(cell.symbol());
        }
        text.trim_end().to_string()
    }

    fn lines_containing(buffer: &ratatui::buffer::Buffer, needle: &str) -> Vec<u16> {
        (0..buffer.area.height)
            .filter(|&y| line_text(buffer, y).contains(needle))
            .collect()
    }

    #[test]
    fn wizard_renders_sectioned_layout_with_unified_titles_and_indent() {
        let choices = vec![
            wizard_choice("w1", "alpha", "/tmp/alpha"),
            wizard_choice("w2", "beta", "/tmp/beta"),
        ];
        let filtered: Vec<usize> = vec![0, 1];
        let buffer = render_wizard(wizard_view(
            &choices,
            &filtered,
            0,
            WizardField::WorkspaceSearch,
            "",
            "ws/alpha",
            "trunk()",
            None,
        ));

        // Modal header + single static hint line at the top.
        let hint = lines_containing(&buffer, "type to filter or edit · ↑/↓ select · tab switch");
        assert_eq!(hint.len(), 1, "hint must appear exactly once: {hint:?}");
        assert!(hint[0] > 0, "hint sits below the modal header");

        // Section titles each on their own line, no operation-hint suffixes.
        let source = lines_containing(&buffer, "Source Workspace");
        assert_eq!(source.len(), 1, "{source:?}");
        let name = lines_containing(&buffer, "New Workspace Name");
        assert_eq!(name.len(), 1, "{name:?}");
        let base = lines_containing(&buffer, "Base · jj revset");
        assert_eq!(base.len(), 1, "{base:?}");
        let checkout = lines_containing(&buffer, "Checkout");
        assert_eq!(checkout.len(), 1, "{checkout:?}");
        // No stray per-section hints remain on the title lines.
        assert!(lines_containing(&buffer, "tab to edit").is_empty());
        assert!(lines_containing(&buffer, "tab edit name/base").is_empty());

        // Titles render bold: assert on the first glyph column of each title line.
        for (y, title_text) in [
            (source[0], "Source Workspace"),
            (name[0], "New Workspace Name"),
            (base[0], "Base · jj revset"),
            (checkout[0], "Checkout"),
        ] {
            let title_start = line_text(&buffer, y).find(title_text).unwrap_or(0) as u16;
            let cell = &buffer[(title_start, y)];
            assert!(
                cell.style().add_modifier.contains(Modifier::BOLD),
                "title line {y} ({title_text:?}) must be bold"
            );
        }

        // Content lines share the 3-column indent: query bar, name/base values
        // and checkout path all start at the same column as the list label.
        let source_y = source[0];
        let query_y = source_y + 1;
        let query_line = line_text(&buffer, query_y);
        assert!(query_line.contains("   filter…"), "placeholder: {query_line:?}");
        let list_y = query_y + 1;
        let list_line = line_text(&buffer, list_y);
        assert!(list_line.contains("▸  alpha "), "active row: {list_line:?}");
        assert!(
            line_text(&buffer, list_y + 1).contains("   beta "),
            "inactive row keeps label column"
        );
        // name value, base value, checkout path all start with the 3-space pad.
        let name_value_y = name[0] + 1;
        assert!(line_text(&buffer, name_value_y).contains("   ws/alpha"), "name value indent");
        let base_value_y = base[0] + 1;
        assert!(line_text(&buffer, base_value_y).contains("   trunk()"), "base value indent");
        let checkout_value_y = checkout[0] + 1;
        assert!(
            line_text(&buffer, checkout_value_y).contains("   "),
            "checkout value indent: {:?}",
            line_text(&buffer, checkout_value_y)
        );
    }

    #[test]
    fn wizard_focus_switches_title_color_only() {
        let choices = vec![wizard_choice("w1", "alpha", "/tmp/alpha")];
        let filtered: Vec<usize> = vec![0];

        let focused_name = render_wizard(wizard_view(
            &choices,
            &filtered,
            0,
            WizardField::Name,
            "",
            "ws/alpha",
            "trunk()",
            None,
        ));
        let name_y = lines_containing(&focused_name, "New Workspace Name")[0];
        let source_y = lines_containing(&focused_name, "Source Workspace")[0];

        // Focused title is accent; unfocused is subtext0; both bold.
        let name_x = line_text(&focused_name, name_y)
            .find("New Workspace Name")
            .unwrap_or(0) as u16;
        let source_x = line_text(&focused_name, source_y)
            .find("Source Workspace")
            .unwrap_or(0) as u16;
        let name_cell = &focused_name[(name_x, name_y)];
        let source_cell = &focused_name[(source_x, source_y)];
        assert_eq!(name_cell.style().fg, Some(palette_accent()));
        assert_eq!(source_cell.style().fg, Some(palette_subtext0()));
        assert!(name_cell.style().add_modifier.contains(Modifier::BOLD));
        assert!(source_cell.style().add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn wizard_zero_candidate_collapses_to_source_and_esc_only() {
        let choices: Vec<WorkspaceChoice> = vec![];
        let filtered: Vec<usize> = vec![];
        let buffer = render_wizard(wizard_view(
            &choices,
            &filtered,
            0,
            WizardField::WorkspaceSearch,
            "",
            "ws/alpha",
            "trunk()",
            None,
        ));

        assert_eq!(lines_containing(&buffer, "Source Workspace").len(), 1);
        assert!(
            lines_containing(&buffer, "no jj workspaces — open herdr's project picker instead")
                .len()
                == 1
        );
        assert!(lines_containing(&buffer, "New Workspace Name").is_empty());
        assert!(lines_containing(&buffer, "Base · jj revset").is_empty());
        assert!(lines_containing(&buffer, "create and open").is_empty());
        // No full hint line in the collapsed state (create/tab would lie).
        assert!(lines_containing(&buffer, "tab switch").is_empty());
        assert!(lines_containing(&buffer, "press esc to close").len() == 1);
    }

    #[test]
    fn wizard_query_no_match_keeps_sections_with_placeholder_preview() {
        let choices = vec![wizard_choice("w1", "alpha", "/tmp/alpha")];
        let filtered: Vec<usize> = vec![];
        let buffer = render_wizard(wizard_view(
            &choices,
            &filtered,
            0,
            WizardField::WorkspaceSearch,
            "zzz",
            "ws/alpha",
            "trunk()",
            None,
        ));

        assert!(lines_containing(&buffer, "no matching workspaces").len() == 1);
        assert_eq!(lines_containing(&buffer, "New Workspace Name").len(), 1);
        assert_eq!(lines_containing(&buffer, "Base · jj revset").len(), 1);
        assert_eq!(lines_containing(&buffer, "Checkout").len(), 1);
        // Checkout preview shows the placeholder, not a fabricated path.
        let checkout_y = lines_containing(&buffer, "Checkout")[0];
        assert!(line_text(&buffer, checkout_y + 1).contains("no matching workspace"));
    }

    #[test]
    fn wizard_query_empty_unfocused_shows_no_placeholder() {
        let choices = vec![wizard_choice("w1", "alpha", "/tmp/alpha")];
        let filtered: Vec<usize> = vec![0];
        let buffer = render_wizard(wizard_view(
            &choices,
            &filtered,
            0,
            WizardField::Name,
            "",
            "ws/alpha",
            "trunk()",
            None,
        ));
        assert!(lines_containing(&buffer, "filter…").is_empty());
    }

    /// Palette probes for render assertions (mirrors `catppuccin`).
    fn palette_accent() -> Color {
        Color::Rgb(137, 180, 250)
    }
    fn palette_subtext0() -> Color {
        Color::Rgb(166, 173, 200)
    }
}
