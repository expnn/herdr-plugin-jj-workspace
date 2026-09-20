// jj-workspace: a Herdr plugin to create/remove Jujutsu (jj) workspaces,
// mirroring Herdr's own git-worktree flow and dialog.
//
// One binary, dispatched by subcommand (set in herdr-plugin.toml):
//   open <workspace|tab>  action: capture the caller, open the wizard pane
//   wizard                pane:   select a source + name, create the two-pane workspace
//   remove                action: precheck config/jj/target, open the remove dialog pane
//   remove-wizard         pane:   review + execute the workspace removal
//
// The wizard renders the actual "new worktree" modal using the same TUI stack as
// Herdr (ratatui + crossterm), ported from herdr's src/ui/dialogs.rs and
// src/ui/widgets.rs so it looks and behaves like the built-in dialog.

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{self, Command};
use std::thread;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
        MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    symbols,
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
    },
    Frame, Terminal,
};
use serde::Deserialize;
use serde_json::Value;

mod opencode_migration;

/// The wizard's single resolved source: the caller's focused pane directory
/// normalized to the main repository root, plus the caller's workspace id
/// (the landing workspace of the new tab).
#[derive(Clone, Debug)]
struct WorkspaceSource {
    id: String,
    path: String,
}

struct WizardResult {
    source: WorkspaceSource,
    name: String,
    /// The base revision for workspace creation: the resolution-chain value
    /// evaluated for the single source, or the user-edited revset (validated)
    /// when the base field was touched.
    base_rev: String,
}

#[derive(Clone, Copy)]
struct WizardView<'a> {
    source: &'a str,
    field: WizardField,
    name: &'a str,
    name_cursor: usize,
    base: &'a str,
    base_cursor: usize,
    root: &'a Path,
    error: Option<&'a str>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum WizardField {
    Name,
    Base,
}

/// Tab/BackTab cycle order of the wizard's editable fields: Name ↔ Base.
fn next_wizard_field(field: WizardField) -> WizardField {
    match field {
        WizardField::Name => WizardField::Base,
        WizardField::Base => WizardField::Name,
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

/// A single-line text buffer with an explicit char-index cursor. Shared by
/// the wizard's name/base fields and the remove dialog's commit message so
/// all three follow one editing contract; the name field wraps it in its
/// component-level state machine.
#[derive(Clone, Debug, PartialEq, Eq)]
struct LineEdit {
    text: String,
    /// Char index into `text` (`0..=char count`), never a byte index:
    /// insert/delete must not split a multi-byte character.
    cursor: usize,
}

impl LineEdit {
    /// A line with the cursor parked at the end.
    fn new(text: String) -> Self {
        let cursor = text.chars().count();
        Self { text, cursor }
    }

    /// Byte offset of the cursor; an out-of-range cursor clamps to the end.
    fn byte_offset(&self) -> usize {
        self.text
            .char_indices()
            .nth(self.cursor)
            .map(|(offset, _)| offset)
            .unwrap_or(self.text.len())
    }

    fn char_count(&self) -> usize {
        self.text.chars().count()
    }

    /// Replace the whole text and park the cursor at the end.
    fn set_text(&mut self, text: String) {
        self.cursor = text.chars().count();
        self.text = text;
    }

    /// Clear the text and move the cursor to the start.
    fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    /// Insert `c` at the cursor and advance it past the new char.
    fn insert_char(&mut self, c: char) {
        let offset = self.byte_offset();
        self.text.insert(offset, c);
        self.cursor += 1;
    }

    /// Delete the char before the cursor (no-op at the start).
    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let end = self.byte_offset();
        let start = self.text[..end]
            .char_indices()
            .next_back()
            .map(|(offset, _)| offset)
            .unwrap_or(0);
        self.text.replace_range(start..end, "");
        self.cursor -= 1;
    }

    /// Delete the char at the cursor (no-op at the end).
    fn delete(&mut self) {
        let start = self.byte_offset();
        if start >= self.text.len() {
            return;
        }
        let end = start
            + self.text[start..]
                .chars()
                .next()
                .map_or(0, char::len_utf8);
        self.text.replace_range(start..end, "");
    }

    fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    fn move_right(&mut self) {
        if self.cursor < self.char_count() {
            self.cursor += 1;
        }
    }

    fn home(&mut self) {
        self.cursor = 0;
    }

    fn end(&mut self) {
        self.cursor = self.char_count();
    }
}

/// A single-line editing keypress, applied by `apply_line_key`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EditKey {
    Char(char),
    Backspace,
    Delete,
    Left,
    Right,
    Home,
    End,
}

/// The shared single-line editing primitive: every key acts at the cursor.
fn apply_line_key(line: &mut LineEdit, key: EditKey) {
    match key {
        EditKey::Char(c) => line.insert_char(c),
        EditKey::Backspace => line.backspace(),
        EditKey::Delete => line.delete(),
        EditKey::Left => line.move_left(),
        EditKey::Right => line.move_right(),
        EditKey::Home => line.home(),
        EditKey::End => line.end(),
    }
}

/// Map a navigation keycode to its editing action; `None` for other keys.
fn line_nav_key(code: KeyCode) -> Option<EditKey> {
    match code {
        KeyCode::Left => Some(EditKey::Left),
        KeyCode::Right => Some(EditKey::Right),
        KeyCode::Home => Some(EditKey::Home),
        KeyCode::End => Some(EditKey::End),
        KeyCode::Delete => Some(EditKey::Delete),
        _ => None,
    }
}

/// Visible text of a single-line field: focused fields insert the block
/// cursor at the cursor's char boundary, unfocused fields render plain.
fn render_line_with_cursor(text: &str, cursor: usize, focused: bool) -> String {
    if !focused {
        return text.to_string();
    }
    let offset = text
        .char_indices()
        .nth(cursor)
        .map(|(offset, _)| offset)
        .unwrap_or(text.len());
    format!("{}█{}", &text[..offset], &text[offset..])
}

/// Apply a keypress to the name field (component-level state machine wrapping
/// the shared line-edit primitive):
/// - Fresh + Char    → keep prefix, replace slug, then Free
/// - Fresh + Bksp    → drop slug, keep prefix (→ Prefixed)
/// - Prefixed + Char → append after prefix (→ Free)
/// - Prefixed + Bksp → clear whole name (→ Free)
/// - Free + any key  → delegate to `apply_line_key`
/// - Fresh/Prefixed + any navigation key → text untouched, cursor moves, Free
///
/// The prefix is derived from the current name via the last `/`, never
/// hardcoded, so user-typed multi-segment names edit per-character.
fn apply_name_key(name: &mut LineEdit, state: &mut NameEditState, key: EditKey) {
    match key {
        EditKey::Char(c) => {
            if *state == NameEditState::Fresh {
                // Replace the slug part, keep the prefix (everything up to
                // and including the last '/').
                let prefix = match name.text.rfind('/') {
                    Some(slash) => name.text[..slash + 1].to_string(),
                    None => String::new(),
                };
                name.set_text(prefix);
            }
            name.insert_char(c);
            *state = NameEditState::Free;
        }
        EditKey::Backspace => match *state {
            NameEditState::Fresh => match name.text.rfind('/') {
                Some(slash) => {
                    let prefix = name.text[..slash + 1].to_string();
                    name.set_text(prefix);
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
            NameEditState::Free => name.backspace(),
        },
        // Any navigation key exits an anchor state without touching the
        // text: the safety valve only ever fails toward per-char editing.
        // The rule is "any nav key press", not "the cursor actually moved".
        EditKey::Delete | EditKey::Left | EditKey::Right | EditKey::Home | EditKey::End => {
            *state = NameEditState::Free;
            apply_line_key(name, key);
        }
    }
}

/// Apply a keypress to the wizard's base field. `replace_on_type` mirrors the
/// name field's component semantics: the first Char/Backspace clears the
/// prefilled value before applying, while any navigation key cancels the
/// anchor without touching the text. Returns whether the key dirtied the
/// field (navigation never does).
fn apply_base_key(base: &mut LineEdit, replace_on_type: &mut bool, key: EditKey) -> bool {
    match key {
        EditKey::Char(_) | EditKey::Backspace => {
            if *replace_on_type {
                base.clear();
                *replace_on_type = false;
            }
            apply_line_key(base, key);
            true
        }
        _ => {
            *replace_on_type = false;
            apply_line_key(base, key);
            false
        }
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
        let mut effective =
            Vec::with_capacity(self.bootstrap_paths.len() + self.extend_bootstrap_paths.len());
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
    let config: Config =
        toml::from_str(&content).map_err(|err| ConfigError::Parse(err.to_string()))?;
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
                return Err(ConfigError::Empty(format!(
                    "agent.bootstrap_paths[{index}]"
                )));
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
            ResolveError::BadAbsolutePath {
                origin,
                path,
                reason,
            } => {
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
        JjCommandValue::Argv(items) => {
            (items[0].clone(), items[1..].to_vec(), "jj.command argv[0]")
        }
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
        Some("remove-wizard") => cmd_remove_wizard(),
        other => {
            eprintln!(
                "usage: jj-workspace <open [workspace|tab] | wizard | remove | remove-wizard>"
            );
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

/// Determine the final base revision at wizard submit time for the single jj
/// source: an untouched base field (dirty = false) is re-evaluated from the
/// resolution chain for that source; a user-edited value (dirty = true) is
/// validated against the repo with `jj log -r <expr>` (a cheap parse check
/// with `--limit 1`). A revset that resolves to no commits is rejected too
/// (empty stdout), so an empty-set expression like `none()` can never reach
/// `jj workspace add` and leave a half-finished workspace behind. Failure
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
/// and resolves it; `--limit 1` keeps it fast and pager-free. Exit 0 with
/// empty stdout means the revset resolved to no commits (e.g. `none()`),
/// which is rejected here so `jj workspace add` never registers a
/// half-finished workspace. Non-zero exit carries jj's native message into
/// the wizard error line.
fn validate_revset(jj: &ResolvedJj, repo: &Path, value: &str) -> Result<(), String> {
    let mut validate = Command::new(&jj.executable);
    validate.current_dir(repo).args(&jj.extra_args).args([
        "log",
        "-r",
        value,
        "--no-graph",
        "--limit",
        "1",
        "--no-pager",
    ]);
    match validate.output() {
        Ok(output) if output.status.success() => {
            if output.stdout.is_empty() {
                Err(format!("base revset resolves to no commits: {value}"))
            } else {
                Ok(())
            }
        }
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

/// Action (headless): precheck the caller's source, then open the wizard pane.
fn cmd_open(_mode: &str) -> ! {
    let config = load_config().unwrap_or_else(|err| die(&err.to_string()));
    // Resolve eagerly so a broken `jj.command` fails the action (and surfaces
    // as a toast) instead of surfacing only inside the wizard pane.
    if let Err(err) = resolve_jj_command(&config.jj.command, &path_dirs()) {
        die(&err.to_string());
    }
    // Precheck the source before opening the pane: a missing or non-jj
    // focused pane directory fails the action with a toast (the action's
    // stderr has no visible outlet) and the wizard pane never opens. The
    // pane itself re-resolves from its own injected context; the wizard's
    // fail-fast modal covers any divergence.
    let ctx = env::var("HERDR_PLUGIN_CONTEXT_JSON").unwrap_or_default();
    let _source = resolve_source_from_ctx(&ctx).unwrap_or_else(|err| die(&err));

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
    .arg("--focus");
    match cmd.status() {
        Ok(status) => process::exit(status.code().unwrap_or(0)),
        Err(err) => {
            eprintln!("error: failed to open wizard pane: {err}");
            process::exit(1);
        }
    }
}

/// Pane (interactive TTY): resolve the single source, name the workspace, then
/// create the agent-left / terminal-right Herdr workspace.
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
    // Single source from our own injected context (no `--env` forwarding from
    // the action): the focused pane's cwd normalized to the main repo root,
    // plus the caller's workspace id for the new tab. Failure renders a
    // fail-fast modal instead of the wizard (covers direct pane entrypoint
    // opens that bypass the action's toast precheck).
    let ctx = env::var("HERDR_PLUGIN_CONTEXT_JSON").unwrap_or_default();
    let source = match resolve_source_from_ctx(&ctx) {
        Ok(source) => source,
        Err(err) => show_resolution_error_and_exit(&err),
    };
    let root = workspaces_root(&config);

    // Prefill the wizard's base field from the single source. The value shown
    // is display-only: at submit, an untouched field is re-evaluated from the
    // resolution chain for this source (see `wizard_final_base_rev`).
    let initial_base = {
        // Display-only prefill: a repo-level resolution error (e.g. an
        // explicit empty `herdr.base-rev` in jj repo config) must NOT kill
        // the wizard at entry — fall back to the global default here. The
        // real resolution happens at submit (wizard_final_base_rev), where
        // errors surface as the wizard's own error line and the user can
        // edit the base field to proceed.
        resolve_base_rev(&config, &jj, Path::new(&source.path))
            .unwrap_or_else(|_| config.jj.base_rev.clone())
    };

    let selection = match run_workspace_wizard(
        &source,
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

    // `jj.command` was already resolved and validated at wizard entry. The
    // source is already the main repository root (resolved by
    // `resolve_source_from_ctx`), so sibling checkouts remain grouped under a
    // stable directory; `repo_root` is an identity here.
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
    add.current_dir(&repo).args(&jj.extra_args).args([
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
    log_error(message);
    show_error_modal_and_exit("configuration error", message);
}

/// Fail-fast for the interactive panes' context resolution (wizard source /
/// remove target): same one-line summary + error.log pointer wording as the
/// action-side `die()` toast, so both outlets carry identical copyable text.
/// Rendered as a modal because a pane has no toast outlet.
fn show_resolution_error_and_exit(message: &str) -> ! {
    let log_path = log_error(message);
    let body = die_toast_body(message, log_path.as_deref());
    show_error_modal_and_exit("jj-workspace error", &body);
}

/// Render a fatal error in a minimal TUI modal, wait for one keypress, exit
/// non-zero. Callers log first; the modal only displays.
fn show_error_modal_and_exit(title: &str, message: &str) -> ! {
    let _ = enable_raw_mode();
    let mut out = io::stdout();
    let _ = execute!(out, EnterAlternateScreen);
    let mut terminal = match Terminal::new(CrosstermBackend::new(out)) {
        Ok(terminal) => terminal,
        Err(_) => {
            let _ = disable_raw_mode();
            fail(message);
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
                title,
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
            agent_attention_toast(
                &herdr,
                &label,
                &format!("{label} is waiting for your input."),
            );
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
        let _ = Command::new(&herdr)
            .args(["agent", "focus", left_pane])
            .status();
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

/// The uncommitted changes of `workspace` as `jj diff --summary -r @` output
/// lines (trimmed, blanks dropped). Empty = clean. A spawn failure or a
/// non-zero exit is an `Err` (fail-closed: the caller must refuse removal
/// rather than treat "could not check" as "clean"). The error wording keeps
/// the `refusing to remove` marker the toast/tests rely on.
fn workspace_change_lines(jj: &ResolvedJj, workspace: &Path) -> Result<Vec<String>, String> {
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
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

/// Action (headless): precheck config, `jj.command` and the removal target,
/// then open the removal dialog pane. All mutation happens in the pane after
/// explicit authorization; the action only gates reachability (missing cwd,
/// non-jj directory, unsafe path, unresolvable main repo) with a toast.
fn cmd_remove() -> ! {
    let config = load_config().unwrap_or_else(|err| die(&err.to_string()));
    // Resolve eagerly so a broken `jj.command` fails the action (and surfaces
    // as a toast) instead of surfacing only inside the dialog pane.
    if let Err(err) = resolve_jj_command(&config.jj.command, &path_dirs()) {
        die(&err.to_string());
    }
    // Precheck the target from our own injected context (no `--env`
    // forwarding): the pane re-resolves from its own context, so any
    // divergence falls through to the pane's fail-fast modal.
    let ctx = env::var("HERDR_PLUGIN_CONTEXT_JSON").unwrap_or_default();
    let _target = resolve_remove_target(&ctx).unwrap_or_else(|err| die(&err));

    let mut cmd = Command::new(herdr_bin());
    cmd.args([
        "plugin",
        "pane",
        "open",
        "--plugin",
        &plugin_id(),
        "--entrypoint",
        "remove-wizard",
    ])
    .arg("--focus");
    match cmd.status() {
        Ok(status) => process::exit(status.code().unwrap_or(0)),
        Err(err) => {
            eprintln!("error: failed to open remove dialog pane: {err}");
            process::exit(1);
        }
    }
}

/// Pane (interactive TTY): review the removal in the dialog and, on
/// authorization, run the staged pipeline. Esc leaves without any mutation.
fn cmd_remove_wizard() -> ! {
    let config = match load_config() {
        Ok(config) => config,
        Err(err) => show_config_error_and_exit(&err.to_string()),
    };
    let jj = match resolve_jj_command(&config.jj.command, &path_dirs()) {
        Ok(jj) => jj,
        Err(err) => show_config_error_and_exit(&err.to_string()),
    };
    // Re-resolve from our own injected context: the dialog never trusts an
    // action-side resolution. Failure renders the fail-fast modal (same copy
    // as the action toast + error.log pointer) instead of the dialog.
    let ctx = env::var("HERDR_PLUGIN_CONTEXT_JSON").unwrap_or_default();
    let target = match resolve_remove_target(&ctx) {
        Ok(target) => target,
        Err(err) => show_resolution_error_and_exit(&err),
    };
    let herdr = herdr_bin();

    match run_remove_dialog(&jj, &target, &herdr) {
        ReviewOutcome::Cancelled => process::exit(0),
        ReviewOutcome::Authorized(plan) => {
            let failed = run_remove_pipeline(&jj, &herdr, &plan);
            process::exit(if failed { 1 } else { 0 });
        }
    }
}

/// Execute an authorized plan in the Status view (tasks 5.2–5.3). Stages run
/// strictly in order; a destructive failure logs, points Status at
/// `error.log` and stops, leaving later stages Pending. Per-pane close
/// failures are warnings inside the close stage, never failures.
fn run_remove_pipeline(jj: &ResolvedJj, herdr: &str, plan: &RemovePlan) -> bool {
    let mut state = StatusState::new(plan.dir.as_deref());
    let mut view = match StatusView::open(&state) {
        Ok(view) => view,
        Err(err) => {
            // Fail closed: without the Status view there is no visible outlet
            // for progress or failure, so do not start destructive work.
            let _ = disable_raw_mode();
            eprintln!("error: cannot start status view: {err}");
            return true;
        }
    };

    // Stage 1: migrate the selected opencode sessions. `dir: None` was
    // already marked skipped by the constructor — never call with no path.
    if plan.dir.is_some() {
        state.running(StatusTask::Migrate);
        let _ = view.update(&state);
        let dir = plan.dir.as_deref().expect("dir is Some");
        match opencode_migration::migrate_selected_opencode_sessions(
            dir,
            &plan.main_repo,
            &plan.session_ids,
        ) {
            opencode_migration::Outcome::Skipped(reason) => {
                state.skipped(StatusTask::Migrate, reason)
            }
            opencode_migration::Outcome::Migrated { count, main_repo } => state.done(
                StatusTask::Migrate,
                format!("{count} session(s) migrated to {}", main_repo.display()),
            ),
            opencode_migration::Outcome::Refused(message) => {
                return fail_pipeline(&mut view, &mut state, StatusTask::Migrate, &message)
            }
        }
        let _ = view.update(&state);
    }

    // Stage 2: unregister the workspace. Commits and bookmarks stay in the
    // shared store, so this is the safe step even for stale registrations.
    state.running(StatusTask::Forget);
    let _ = view.update(&state);
    match resolve_forget_name(jj, plan) {
        Some(name) => match jj_forget_workspace(jj, &plan.main_repo, &name) {
            Ok(()) => state.done(
                StatusTask::Forget,
                "unregistered; commits & bookmarks stay in the shared store",
            ),
            Err(message) => {
                return fail_pipeline(&mut view, &mut state, StatusTask::Forget, &message)
            }
        },
        None => {
            return fail_pipeline(
                &mut view,
                &mut state,
                StatusTask::Forget,
                "cannot resolve the workspace name to forget",
            )
        }
    }
    let _ = view.update(&state);

    // Stage 3: delete the directory (missing = skipped, never failed).
    if let Some(dir) = plan.dir.as_deref() {
        if dir.exists() {
            state.running(StatusTask::Delete);
            let _ = view.update(&state);
            match fs::remove_dir_all(dir) {
                Ok(()) => state.done(StatusTask::Delete, dir.display().to_string()),
                Err(err) => {
                    return fail_pipeline(
                        &mut view,
                        &mut state,
                        StatusTask::Delete,
                        &format!("failed to delete {}: {err}", dir.display()),
                    )
                }
            }
        } else {
            state.skipped(StatusTask::Delete, "already missing");
        }
        let _ = view.update(&state);
    }

    // Stage 4: close the selected panes, best-effort. Individual refusals are
    // warnings (logged + summarised), never a pipeline failure.
    if plan.dir.is_some() {
        if plan.pane_ids.is_empty() {
            state.skipped(StatusTask::ClosePanes, "no panes selected");
        } else {
            state.running(StatusTask::ClosePanes);
            let _ = view.update(&state);
            let mut closed = 0usize;
            let mut failed = 0usize;
            for pane_id in &plan.pane_ids {
                match close_herdr_pane(herdr, pane_id) {
                    Ok(()) => closed += 1,
                    Err(message) => {
                        failed += 1;
                        log_error(&message);
                    }
                }
            }
            state.done(StatusTask::ClosePanes, close_summary(closed, failed));
        }
        let _ = view.update(&state);
    }

    // Reaching this point means no destructive stage failed; `any_failure`
    // stays the single source of truth for the exit code.
    let failed = state.any_failure();
    let _ = view.finish(failed);
    failed
}

/// Record a destructive-stage failure: log it, point Status at the log, draw
/// the failure and wait for the user to close. Always returns `true` (the
/// pipeline failure marker).
fn fail_pipeline(
    view: &mut StatusView,
    state: &mut StatusState,
    task: StatusTask,
    message: &str,
) -> bool {
    state.failed(task, message);
    if let Some(path) = log_error(message) {
        state.set_error_log(path);
    }
    let _ = view.update(state);
    let _ = view.finish(true);
    true
}

/// The workspace name `jj workspace forget` needs. Picker selections carry
/// the name directly; a secondary target resolves it by matching the
/// canonical workspace root in `jj workspace list`; the last resort is the
/// directory basename.
fn resolve_forget_name(jj: &ResolvedJj, plan: &RemovePlan) -> Option<String> {
    if let Some(name) = plan.workspace_name.clone().filter(|name| !name.is_empty()) {
        return Some(name);
    }
    let dir = plan.dir.as_deref()?;
    if let Ok(entries) = list_secondary_workspaces(jj, &plan.main_repo) {
        if let Ok(canon) = fs::canonicalize(dir) {
            let matched = entries.iter().find(|entry| {
                entry
                    .root
                    .as_deref()
                    .and_then(|root| fs::canonicalize(root).ok())
                    .as_deref()
                    == Some(canon.as_path())
            });
            if let Some(entry) = matched {
                return Some(entry.name.clone());
            }
        }
    }
    dir.file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

/// Close-stage summary: individual `pane close` refusals are warnings (the
/// stage is still Done), never a pipeline failure (design D4).
fn close_summary(closed: usize, failed: usize) -> String {
    if failed == 0 {
        format!("{closed} pane(s) closed")
    } else {
        format!("{closed} closed, {failed} failed (warning)")
    }
}

// --- remove dialog data sources (remove-workspace-dialog, tasks 2.1–2.4) ---
//
// Backend half of the removal dialog: target resolution, the secondary
// workspace picker, the global pane scan, the opencode session preview,
// display formatting and the individual commands the execution pipeline
// drives. The TUI and the pipeline itself live in later lanes and call
// exactly these signatures.

/// Where a `remove` invocation points: the main workspace (the dialog then
/// runs the secondary-workspace picker) or one secondary workspace (review).
#[derive(Debug, Clone, PartialEq, Eq)]
enum RemoveTarget {
    /// `.jj/repo` is a directory: the focused pane is in the main workspace,
    /// which can never itself be removed — pick a secondary workspace first.
    Main { main_root: PathBuf },
    /// A secondary workspace root plus the main repo it belongs to.
    Secondary { target: PathBuf, main_repo: PathBuf },
}

/// A path the removal flow refuses to touch: `/` or anything without a
/// parent (there is nothing above it to keep, and deleting it is never the
/// intent of "remove this workspace").
fn unsafe_remove_path(path: &Path) -> bool {
    path == Path::new("/") || path.parent().is_none()
}

/// Resolve the removal target from the plugin's own context JSON
/// (`focused_pane_cwd`, falling back to `workspace_cwd`): canonicalize, then
/// walk up with [`jj_root`] to the nearest directory carrying `.jj`. That
/// directory IS the target — secondary workspaces are deliberately not
/// resolved through to the main repo root (the workspace itself is what gets
/// removed).
///
/// `.jj/repo` being a directory marks the main workspace; a secondary
/// workspace additionally resolves its `.jj/repo` pointer to the main repo
/// and fails closed when that pointer is missing, unreadable or
/// self-referencing. The first error line is short and actionable: it is
/// what the action's toast renders.
fn resolve_remove_target(ctx: &str) -> Result<RemoveTarget, String> {
    let cwd = json_string_field(ctx, "focused_pane_cwd")
        .filter(|cwd| !cwd.is_empty())
        .or_else(|| json_string_field(ctx, "workspace_cwd").filter(|cwd| !cwd.is_empty()))
        .ok_or_else(|| {
            "no focused pane cwd in plugin context (is there an active workspace?)".to_string()
        })?;
    let canon = fs::canonicalize(&cwd)
        .map_err(|err| format!("cannot resolve focused pane cwd '{cwd}': {err}"))?;
    if unsafe_remove_path(&canon) {
        return Err(format!(
            "refusing to remove unsafe path: {}",
            canon.display()
        ));
    }
    let root = jj_root(&canon.display().to_string())
        .ok_or_else(|| format!("{cwd} is not inside a jj workspace (no .jj marker found)"))?;
    let target = fs::canonicalize(&root).unwrap_or_else(|_| PathBuf::from(&root));
    if unsafe_remove_path(&target) {
        return Err(format!(
            "refusing to remove unsafe path: {}",
            target.display()
        ));
    }
    // The MAIN workspace stores `.jj/repo` as a directory; a secondary
    // workspace stores it as a file pointer to the main store.
    if target.join(".jj").join("repo").is_dir() {
        return Ok(RemoveTarget::Main { main_root: target });
    }
    let main_repo = opencode_migration::resolve_main_repo(&target).ok_or_else(|| {
        format!(
            "cannot resolve the main repo for this workspace (refusing to remove)\n\
             the .jj/repo pointer is missing, unreadable, or points at the workspace itself\n\
             workspace path: {}",
            target.display()
        )
    })?;
    Ok(RemoveTarget::Secondary { target, main_repo })
}

/// One row of the secondary-workspace picker. A `None` root is a stale
/// registration: the path was never recorded or the directory is gone (jj
/// renders it empty); such entries can still be forgotten.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkspaceEntry {
    name: String,
    root: Option<PathBuf>,
}

/// The jj template rendering one `name<TAB>root` line per registered
/// workspace (`root` empty when the path is not recorded / deleted). Passed
/// as ONE argv item — jj compiles it, the shell never touches it.
const WORKSPACE_LIST_TEMPLATE: &str = "name ++ \"\\t\" ++ root ++ \"\\n\"";

/// Parse the template output of `jj workspace list`: `name<TAB>root` per
/// line, empty root → `None`, malformed lines (no tab, empty name) skipped,
/// result sorted by name.
fn parse_workspace_list(stdout: &str) -> Vec<WorkspaceEntry> {
    let mut entries: Vec<WorkspaceEntry> = stdout
        .lines()
        .filter_map(|line| {
            let (name, root) = line.split_once('\t')?;
            let name = name.trim();
            if name.is_empty() {
                return None;
            }
            let root = root.trim();
            Some(WorkspaceEntry {
                name: name.to_string(),
                root: (!root.is_empty()).then(|| PathBuf::from(root)),
            })
        })
        .collect();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

/// List every secondary workspace registered in `main_repo`, excluding the
/// main workspace itself (canonical path comparison) and sorting by name.
/// Spawn failure or a non-zero exit is an `Err` carrying the first stderr
/// line — the picker must not silently show an empty list when jj failed.
fn list_secondary_workspaces(
    jj: &ResolvedJj,
    main_repo: &Path,
) -> Result<Vec<WorkspaceEntry>, String> {
    let mut command = Command::new(&jj.executable);
    command
        .args(&jj.extra_args)
        .arg("-R")
        .arg(main_repo)
        .arg("--ignore-working-copy")
        .args(["workspace", "list", "-T", WORKSPACE_LIST_TEMPLATE]);
    let output = command
        .output()
        .map_err(|err| format!("jj workspace list failed to start: {err}"))?;
    if !output.status.success() {
        let reason = first_stderr_line(&output.stderr)
            .unwrap_or_else(|| format!("exit {}", output.status.code().unwrap_or(-1)));
        return Err(format!("jj workspace list failed: {reason}"));
    }
    let main_canon = fs::canonicalize(main_repo).ok();
    Ok(
        parse_workspace_list(&String::from_utf8_lossy(&output.stdout))
            .into_iter()
            .filter(|entry| match (&entry.root, &main_canon) {
                (Some(root), Some(main)) => match fs::canonicalize(root) {
                    Ok(root) => root != *main,
                    Err(_) => true,
                },
                _ => true,
            })
            .collect(),
    )
}

/// First non-empty stderr line, trimmed; `None` when stderr is empty.
fn first_stderr_line(stderr: &[u8]) -> Option<String> {
    String::from_utf8_lossy(stderr)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

/// One `herdr pane list` entry; every optional field stays optional so field
/// drift between herdr versions only narrows the matching, never panics.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PaneInfo {
    pane_id: String,
    tab_id: String,
    workspace_id: String,
    label: Option<String>,
    agent: Option<String>,
    agent_status: Option<String>,
    cwd: Option<String>,
    foreground_cwd: Option<String>,
}

/// Why a pane matched the removal target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaneMatch {
    /// The pane's shell cwd is inside the target.
    Cwd,
    /// Only the foreground process cwd is inside the target (the shell
    /// itself still sits elsewhere).
    ForegroundOnly,
}

/// A candidate pane plus the reason it matched.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PaneCandidate {
    info: PaneInfo,
    matched_via: PaneMatch,
}

/// Optional string field of a JSON object.
fn pane_string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

/// Parse `herdr pane list` JSON (`result.panes[]`). Entries without a
/// `pane_id` are skipped; every other field tolerates absence and defaults
/// to empty / `None`.
fn parse_panes(json: &Value) -> Vec<PaneInfo> {
    let panes = match json
        .get("result")
        .and_then(|result| result.get("panes"))
        .and_then(Value::as_array)
    {
        Some(panes) => panes,
        None => return Vec::new(),
    };
    panes
        .iter()
        .filter_map(|pane| {
            let pane_id = pane.get("pane_id").and_then(Value::as_str)?;
            if pane_id.is_empty() {
                return None;
            }
            Some(PaneInfo {
                pane_id: pane_id.to_string(),
                tab_id: pane_string(pane, "tab_id").unwrap_or_default(),
                workspace_id: pane_string(pane, "workspace_id").unwrap_or_default(),
                label: pane_string(pane, "label"),
                agent: pane_string(pane, "agent"),
                agent_status: pane_string(pane, "agent_status"),
                cwd: pane_string(pane, "cwd"),
                foreground_cwd: pane_string(pane, "foreground_cwd"),
            })
        })
        .collect()
}

/// True when `dir` is `target` itself or a descendant of it, with a
/// component boundary (`/a/bc` must never match `/a/b`). Canonicalizes both
/// sides when possible (so symlinked pane cwds still match); a deleted pane
/// cwd — canonicalize fails — falls back to a lexical, component-based
/// prefix comparison against the canonical target.
fn path_within(target: &Path, dir: &str) -> bool {
    if dir.is_empty() {
        return false;
    }
    let dir_path = Path::new(dir);
    let target = fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf());
    match fs::canonicalize(dir_path) {
        Ok(canon_dir) => canon_dir == target || canon_dir.starts_with(&target),
        Err(_) => dir_path == target.as_path() || dir_path.starts_with(&target),
    }
}

/// Match `pane` against `target`: the shell cwd first, then the foreground
/// process cwd. `None` when neither is inside the target.
fn pane_matches_target(target: &Path, pane: &PaneInfo) -> Option<PaneMatch> {
    if let Some(cwd) = pane.cwd.as_deref() {
        if path_within(target, cwd) {
            return Some(PaneMatch::Cwd);
        }
    }
    if let Some(cwd) = pane.foreground_cwd.as_deref() {
        if path_within(target, cwd) {
            return Some(PaneMatch::ForegroundOnly);
        }
    }
    None
}

/// The plugin's own overlay panes: cwd == plugin root AND one of this
/// plugin's pane titles. `pane process-info` cannot identify overlay panes
/// (herdr 0.8.2 returns `pane_not_found`, measured), so this rule (design
/// D5) keeps the dialog from listing or closing itself.
fn is_plugin_own_pane(pane: &PaneInfo, plugin_root: &Path) -> bool {
    let label = match pane.label.as_deref() {
        Some(label) => label,
        None => return false,
    };
    if !matches!(label, "Remove jj workspace" | "New jj workspace") {
        return false;
    }
    let cwd = match pane.cwd.as_deref() {
        Some(cwd) => cwd,
        None => return false,
    };
    match (fs::canonicalize(cwd), fs::canonicalize(plugin_root)) {
        (Ok(pane_cwd), Ok(root)) => pane_cwd == root,
        _ => Path::new(cwd) == plugin_root,
    }
}

/// Scan every herdr pane for candidates inside `target`, excluding this
/// plugin's own overlay panes. A failed `pane list` degrades to an empty
/// candidate list (no panes listed, no panes closed) rather than an error.
fn scan_pane_candidates(herdr: &str, target: &Path, plugin_root: &Path) -> Vec<PaneCandidate> {
    let json = match herdr_json_with(herdr, &["pane", "list"]) {
        Ok(json) => json,
        Err(_) => return Vec::new(),
    };
    parse_panes(&json)
        .into_iter()
        .filter(|pane| !is_plugin_own_pane(pane, plugin_root))
        .filter_map(|pane| {
            pane_matches_target(target, &pane).map(|matched_via| PaneCandidate {
                info: pane,
                matched_via,
            })
        })
        .collect()
}

/// Extract `<id_field> → label` from a `result.<collection>[]` response,
/// skipping entries missing either side.
fn label_map(
    result: Result<Value, String>,
    collection: &str,
    id_field: &str,
) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let json = match result {
        Ok(json) => json,
        Err(_) => return map,
    };
    let items = match json
        .get("result")
        .and_then(|result| result.get(collection))
        .and_then(Value::as_array)
    {
        Some(items) => items,
        None => return map,
    };
    for item in items {
        if let (Some(id), Some(label)) = (pane_string(item, id_field), pane_string(item, "label")) {
            map.insert(id, label);
        }
    }
    map
}

/// Display labels for pane grouping: `(workspace_id → label, tab_id →
/// label)` from `herdr workspace list` / `herdr tab list`. Either command
/// failing (or malformed JSON) degrades to an empty map, so the dialog falls
/// back to raw ids instead of failing.
fn pane_group_labels(herdr: &str) -> (HashMap<String, String>, HashMap<String, String>) {
    let workspaces = label_map(
        herdr_json_with(herdr, &["workspace", "list"]),
        "workspaces",
        "workspace_id",
    );
    let tabs = label_map(herdr_json_with(herdr, &["tab", "list"]), "tabs", "tab_id");
    (workspaces, tabs)
}

/// One session row for the dialog's Plan section. The raw `id` rides along
/// so the later subset migration call can pass it back verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionDisplay {
    id: String,
    title: String,
    directory: String,
    time_updated: i64,
}

/// The dialog's view of the opencode preview: `Skipped` (opencode/DB absent
/// or no rows — nothing to migrate), `Refused` (fail-closed blocking check),
/// or `Ready` rows plus the migration destination.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SessionPreview {
    Skipped(String),
    Refused(String),
    Ready {
        rows: Vec<SessionDisplay>,
        main_repo: PathBuf,
    },
}

/// Read-only preview of the sessions bound to `ws`, mapped from the frozen
/// `opencode_migration::inspect_opencode_sessions` API (SELECTs only).
/// Preview success does not relax the execution-time fail-closed check — the
/// pipeline re-inspects there.
fn session_preview(ws: &Path, main_repo: &Path) -> SessionPreview {
    match opencode_migration::inspect_opencode_sessions(ws, main_repo) {
        opencode_migration::Inspection::Skipped(reason) => SessionPreview::Skipped(reason),
        opencode_migration::Inspection::Refused(reason) => SessionPreview::Refused(reason),
        opencode_migration::Inspection::Ready { rows, main_repo } => SessionPreview::Ready {
            rows: rows
                .into_iter()
                .map(|row| SessionDisplay {
                    id: row.id,
                    title: row.title,
                    directory: row.directory,
                    time_updated: row.time_updated,
                })
                .collect(),
            main_repo,
        },
    }
}

/// `dir` as a path relative to `target`: `.` for the target itself,
/// otherwise the suffix with no leading `/`. Paths outside the target (or
/// stale, deleted pane cwds) fall back to the full string.
fn relative_to_target(target: &Path, dir: &str) -> String {
    let dir_path = Path::new(dir);
    let base = fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf());
    let candidate = fs::canonicalize(dir_path).unwrap_or_else(|_| dir_path.to_path_buf());
    match candidate.strip_prefix(&base) {
        Ok(rest) if rest.as_os_str().is_empty() => ".".to_string(),
        Ok(rest) => rest.display().to_string(),
        Err(_) => dir.to_string(),
    }
}

const MINUTE_MS: i64 = 60_000;
const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 86_400_000;

/// Human age for an epoch-millisecond timestamp: `just now`, `5m ago`,
/// `3h ago`, `6d ago`, then an absolute `YYYY-MM-DD` (UTC) from a week on.
/// Future timestamps clamp to `just now`.
fn format_age_ms(ms: i64, now_ms: i64) -> String {
    let delta = now_ms.saturating_sub(ms).max(0);
    if delta < MINUTE_MS {
        "just now".to_string()
    } else if delta < HOUR_MS {
        format!("{}m ago", delta / MINUTE_MS)
    } else if delta < DAY_MS {
        format!("{}h ago", delta / HOUR_MS)
    } else if delta < 7 * DAY_MS {
        format!("{}d ago", delta / DAY_MS)
    } else {
        format_date_utc(ms)
    }
}

/// `YYYY-MM-DD` (UTC) for an epoch-millisecond timestamp, reusing the
/// civil-from-days timestamp formatter.
fn format_date_utc(ms: i64) -> String {
    let secs = ms.div_euclid(1000).max(0) as u64;
    format_unix_timestamp(secs).chars().take(10).collect()
}

/// `jj commit -m <message>` in `workspace` (argv, no shell). A non-zero exit
/// carries the first stderr line so the dialog can show jj's own error; the
/// caller refreshes the clean check and only then proceeds.
fn jj_commit(jj: &ResolvedJj, workspace: &Path, message: &str) -> Result<(), String> {
    let mut command = Command::new(&jj.executable);
    command
        .current_dir(workspace)
        .args(&jj.extra_args)
        .args(["commit", "-m", message]);
    let output = command
        .output()
        .map_err(|err| format!("jj commit failed to start: {err}"))?;
    if output.status.success() {
        return Ok(());
    }
    let code = output.status.code().unwrap_or(-1);
    Err(match first_stderr_line(&output.stderr) {
        Some(line) => format!("jj commit failed (exit {code}): {line}"),
        None => format!("jj commit failed (exit {code})"),
    })
}

/// Forget the registered workspace `name` from `main_repo`'s store. Works
/// for stale registrations whose directory is already gone (design D3), so
/// the picker can clean those up.
fn jj_forget_workspace(jj: &ResolvedJj, main_repo: &Path, name: &str) -> Result<(), String> {
    let mut command = Command::new(&jj.executable);
    command
        .args(&jj.extra_args)
        .arg("-R")
        .arg(main_repo)
        .args(["workspace", "forget", name]);
    let output = command
        .output()
        .map_err(|err| format!("jj workspace forget failed to start: {err}"))?;
    if output.status.success() {
        return Ok(());
    }
    let code = output.status.code().unwrap_or(-1);
    Err(match first_stderr_line(&output.stderr) {
        Some(line) => format!("jj workspace forget failed (exit {code}): {line}"),
        None => format!("jj workspace forget failed (exit {code})"),
    })
}

/// Close one herdr pane (`herdr pane close <pane_id>`). Individual failures
/// are warnings at the call site (design D4): the pipeline keeps closing the
/// remaining panes.
fn close_herdr_pane(herdr: &str, pane_id: &str) -> Result<(), String> {
    let output = Command::new(herdr)
        .args(["pane", "close", pane_id])
        .output()
        .map_err(|err| format!("herdr pane close failed to start: {err}"))?;
    if output.status.success() {
        return Ok(());
    }
    let code = output.status.code().unwrap_or(-1);
    Err(match first_stderr_line(&output.stderr) {
        Some(line) => format!("herdr pane close failed (exit {code}): {line}"),
        None => format!("herdr pane close failed (exit {code})"),
    })
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
    green: Color,
    yellow: Color,
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
        green: Color::Rgb(166, 227, 161),
        yellow: Color::Rgb(249, 226, 175),
    }
}

/// Returns the chosen name + base revision, or None when cancelled.
fn run_workspace_wizard(
    source: &WorkspaceSource,
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

    let mut field = WizardField::Name;
    let mut name = LineEdit::new(initial_name);
    let mut base = LineEdit::new(initial_base);
    let mut name_edit_state = NameEditState::Fresh;
    let mut base_replace_on_type = true;
    let mut base_dirty = false;
    let mut error: Option<String> = None;

    let outcome = loop {
        let _ = terminal.draw(|frame| {
            draw_workspace_wizard(
                frame,
                &WizardView {
                    source: &source.path,
                    field,
                    name: &name.text,
                    name_cursor: name.cursor,
                    base: &base.text,
                    base_cursor: base.cursor,
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
                    field = next_wizard_field(field);
                    error = None;
                }
                KeyCode::Enter => {
                    if !valid_branch(&name.text) {
                        error = Some("name must match [A-Za-z0-9._/-]".into());
                        continue;
                    }
                    if !Path::new(&source.path).is_dir() {
                        error = Some(format!("folder does not exist: {}", source.path));
                        continue;
                    }
                    let checkout = workspace_destination(root, &source.path, &name.text);
                    if checkout.exists() {
                        error = Some(format!("checkout already exists: {}", checkout.display()));
                        continue;
                    }
                    // The source was validated as a jj workspace at entry
                    // (`resolve_source_from_ctx`), so the final base is always
                    // solved from the resolution chain — or validated when the
                    // user edited the field.
                    let base_rev = match wizard_final_base_rev(
                        config,
                        jj,
                        Path::new(&source.path),
                        &base.text,
                        base_dirty,
                    ) {
                        Ok(value) => value,
                        Err(message) => {
                            error = Some(message);
                            continue;
                        }
                    };
                    break Some(WizardResult {
                        source: source.clone(),
                        name: name.text.clone(),
                        base_rev,
                    });
                }
                KeyCode::Backspace if field == WizardField::Name => {
                    apply_name_key(&mut name, &mut name_edit_state, EditKey::Backspace);
                    error = None;
                }
                KeyCode::Char(c)
                    if field == WizardField::Name
                        && !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    apply_name_key(&mut name, &mut name_edit_state, EditKey::Char(c));
                    error = None;
                }
                KeyCode::Left | KeyCode::Right | KeyCode::Home | KeyCode::End | KeyCode::Delete
                    if field == WizardField::Name
                        && !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    if let Some(nav) = line_nav_key(key.code) {
                        apply_name_key(&mut name, &mut name_edit_state, nav);
                    }
                    error = None;
                }
                KeyCode::Backspace if field == WizardField::Base => {
                    let dirty =
                        apply_base_key(&mut base, &mut base_replace_on_type, EditKey::Backspace);
                    base_dirty |= dirty;
                    error = None;
                }
                KeyCode::Char(c)
                    if field == WizardField::Base
                        && !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    let dirty =
                        apply_base_key(&mut base, &mut base_replace_on_type, EditKey::Char(c));
                    base_dirty |= dirty;
                    error = None;
                }
                KeyCode::Left | KeyCode::Right | KeyCode::Home | KeyCode::End | KeyCode::Delete
                    if field == WizardField::Base
                        && !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    if let Some(nav) = line_nav_key(key.code) {
                        apply_base_key(&mut base, &mut base_replace_on_type, nav);
                    }
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

fn workspace_destination(root: &Path, source: &str, name: &str) -> PathBuf {
    root.join(basename(&repo_root(source)))
        .join(branch_to_path_slug(name))
}

/// Content column indent shared by every section: the leading area is 3
/// columns wide (2 spaces + 1 marker slot retained from the old source list);
/// the same gutter stays blank for every content line so all values — name,
/// base, source path and checkout path — align at one column.
const SECTION_CONTENT_INDENT: u16 = 3;

/// Section title grammar: always bold; focus is expressed by color only —
/// accent (blue) when the field is focused, subtext0 otherwise. Read-only
/// sections (source, checkout) never focus and use the subtext0 form.
fn section_title_style(focused: bool, p: &Palette) -> Style {
    let color = if focused { p.accent } else { p.subtext0 };
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

fn render_section_title(frame: &mut Frame, area: Rect, title: &str, focused: bool, p: &Palette) {
    frame.render_widget(
        Paragraph::new(title).style(section_title_style(focused, p)),
        area,
    );
}

/// Top-of-modal static hint line: one line covering editing, tab cycling,
/// submit and cancel; no list-selection wording (select/filter).
const WIZARD_HINT: &str = "type to edit · tab switch · ↵ create · esc cancel";

fn draw_workspace_wizard(frame: &mut Frame, view: &WizardView<'_>) {
    let WizardView {
        source,
        field,
        name,
        name_cursor,
        base,
        base_cursor,
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

    // Static hint line: one place for all operation hints, before any section.
    frame.render_widget(
        Paragraph::new(WIZARD_HINT).style(Style::default().fg(p.overlay0)),
        Rect::new(inner.x, y, inner.width, 1),
    );
    y += 1;
    y += 1; // blank separator after the hint block

    // --- new workspace name -----------------------------------------------
    render_section_title(
        frame,
        Rect::new(inner.x, y, inner.width, 1),
        "New Workspace Name",
        field == WizardField::Name,
        &p,
    );
    y += 1;
    frame.render_widget(
        Paragraph::new(format!(
            "{pad}{}",
            render_line_with_cursor(name, name_cursor, field == WizardField::Name)
        ))
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
    frame.render_widget(
        Paragraph::new(format!(
            "{pad}{}",
            render_line_with_cursor(base, base_cursor, field == WizardField::Base)
        ))
        .style(Style::default().fg(p.text).bg(p.surface0)),
        Rect::new(inner.x, y, inner.width, 1),
    );
    y += 1;

    // --- source workspace (read-only) --------------------------------------
    y += 1;
    render_section_title(
        frame,
        Rect::new(inner.x, y, inner.width, 1),
        "Source Workspace",
        false,
        &p,
    );
    y += 1;
    // Read-only: the resolved main repository root; never focused, rendered
    // like the checkout preview (subtext0, no edit background).
    frame.render_widget(
        Paragraph::new(format!("{pad}{source}")).style(Style::default().fg(p.subtext0)),
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
    let preview = workspace_destination(root, source, name).display().to_string();
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
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
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

// --- remove dialog (remove-workspace-dialog, tasks 3.1–3.5) -----------------
//
// The dialog shares the wizard's modal chrome. This module owns the pure
// selection model (flat rows, cursor, toggles, counts), the row builder that
// renders Workspace/Plan/Checks, the picker/review event loops, and the
// Status view API the execution pipeline drives.

/// Modal width shared with the wizard.
const REMOVE_MODAL_WIDTH: u16 = 96;
/// Static hint lines, one per dialog mode.
const PICKER_HINT: &str = "↑↓ move · ↵ select · esc cancel";
const REVIEW_HINT: &str = "↑↓ move · space toggle · a all/none · c commit… · ↵ remove · esc cancel";
const COMMIT_HINT: &str = "type message · ↵ commit · esc back";
const STATUS_WORKING_HINT: &str = "removing workspace…";
const STATUS_FAILED_HINT: &str = "↵ close · esc close";

/// Dynamic review rows: their text is derived from the selection model at
/// draw time (it changes with the selection), never frozen in the row.
const PANE_WARNING_ID: &str = "pane-warning";
const SESSION_COUNT_ID: &str = "session-count";
const PANE_COUNT_ID: &str = "pane-count";

/// What a flat review row represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowKind {
    Section,
    Task,
    Group,
    Session,
    Pane,
    Note,
    Check,
    Warning,
}

/// Semantic color of a row, mapped to the palette at render time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowTone {
    Normal,
    Dim,
    Accent,
    Error,
    Ok,
    Warning,
}

/// One line of the review screen. Selection state lives in [`ReviewModel`]
/// (keyed by `id`); the row carries presentation only.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReviewRow {
    kind: RowKind,
    depth: u8,
    text: String,
    tone: RowTone,
    /// Leaf id for Session/Pane rows; semantic tag for Task/Warning rows.
    id: Option<String>,
}

impl ReviewRow {
    fn section(text: impl Into<String>) -> ReviewRow {
        ReviewRow {
            kind: RowKind::Section,
            depth: 0,
            text: text.into(),
            tone: RowTone::Normal,
            id: None,
        }
    }

    fn task(text: impl Into<String>, id: &str) -> ReviewRow {
        ReviewRow {
            kind: RowKind::Task,
            depth: 1,
            text: text.into(),
            tone: RowTone::Normal,
            id: Some(id.to_string()),
        }
    }

    fn group(text: impl Into<String>, depth: u8) -> ReviewRow {
        ReviewRow {
            kind: RowKind::Group,
            depth,
            text: text.into(),
            tone: RowTone::Normal,
            id: None,
        }
    }

    fn leaf(kind: RowKind, id: String, text: String) -> ReviewRow {
        ReviewRow {
            kind,
            depth: 2,
            text,
            tone: RowTone::Normal,
            id: Some(id),
        }
    }

    fn session(id: String, text: String) -> ReviewRow {
        ReviewRow::leaf(RowKind::Session, id, text)
    }

    fn pane(id: String, text: String) -> ReviewRow {
        // Panes sit under workspace (depth 2) → tab (depth 3) groups.
        ReviewRow {
            kind: RowKind::Pane,
            depth: 4,
            text,
            tone: RowTone::Normal,
            id: Some(id),
        }
    }

    fn note(text: impl Into<String>, depth: u8, tone: RowTone) -> ReviewRow {
        ReviewRow {
            kind: RowKind::Note,
            depth,
            text: text.into(),
            tone,
            id: None,
        }
    }

    fn check(text: impl Into<String>, tone: RowTone) -> ReviewRow {
        ReviewRow {
            kind: RowKind::Check,
            depth: 1,
            text: text.into(),
            tone,
            id: None,
        }
    }

    /// Dynamic "N of M selected" note; the text is computed at draw time
    /// from the live selection counts.
    fn count_note(id: &str, depth: u8) -> ReviewRow {
        ReviewRow {
            kind: RowKind::Note,
            depth,
            text: String::new(),
            tone: RowTone::Dim,
            id: Some(id.to_string()),
        }
    }

    fn warning() -> ReviewRow {
        ReviewRow {
            kind: RowKind::Warning,
            depth: 2,
            text: String::new(),
            tone: RowTone::Warning,
            id: Some(PANE_WARNING_ID.to_string()),
        }
    }
}

/// Which Plan tasks a review target can perform. A picker entry whose `root`
/// is empty has an unknown working directory: only `jj workspace forget` is
/// scopeable, so migration, directory deletion and pane closing are skipped
/// and shown as such.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PlanAvailability {
    migrate: bool,
    forget: bool,
    delete: bool,
    close_panes: bool,
}

fn plan_availability(dir: Option<&Path>) -> PlanAvailability {
    let path_known = dir.is_some();
    PlanAvailability {
        migrate: path_known,
        forget: true,
        delete: path_known,
        close_panes: path_known,
    }
}

/// The authorized removal the dialog hands back to the execution pipeline.
///
/// `dir: None` = the picker selected a `missing on disk` entry whose working
/// directory is unknown: only `jj workspace forget` runs, and the empty
/// `session_ids`/`pane_ids` encode "nothing to scope". `workspace_name: None`
/// means the pipeline resolves the name itself at execution time.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RemovePlan {
    dir: Option<PathBuf>,
    main_repo: PathBuf,
    workspace_name: Option<String>,
    session_ids: Vec<String>,
    pane_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReviewOutcome {
    Cancelled,
    Authorized(RemovePlan),
}

/// Everything the review screen renders and checks, gathered once when the
/// dialog opens (and refreshed after an in-dialog commit).
struct ReviewData {
    dir: Option<PathBuf>,
    main_repo: PathBuf,
    workspace_name: Option<String>,
    target_label: String,
    availability: PlanAvailability,
    clean: Option<Result<Vec<String>, String>>,
    sessions: SessionPreview,
    panes: Vec<PaneCandidate>,
    workspace_labels: HashMap<String, String>,
    tab_labels: HashMap<String, String>,
    triggered_pane: Option<String>,
    now_ms: i64,
}

impl ReviewData {
    /// `c` is offered only when the clean check came back dirty and the
    /// working directory is known (a commit needs a cwd). A failed check is
    /// fail-closed and not a dirty-workcopy case, so it does not offer `c`.
    fn can_commit(&self) -> bool {
        if self.dir.is_none() {
            return false;
        }
        matches!(&self.clean, Some(Ok(changes)) if !changes.is_empty())
    }
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

/// Render `path` for display, abbreviating a leading `$HOME` prefix with `~`
/// (component-boundary match, so a sibling like `/home/user2` stays verbatim).
/// Degrades to the plain path when `HOME` is unset or empty.
fn display_home_path(path: &Path) -> String {
    let home = env::var_os("HOME").map(PathBuf::from);
    display_home_path_with(path, home.as_deref())
}

/// Testable core of [`display_home_path`] with the home directory injected.
fn display_home_path_with(path: &Path, home: Option<&Path>) -> String {
    let Some(home) = home.filter(|home| !home.as_os_str().is_empty()) else {
        return path.display().to_string();
    };
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// Truncate by characters with an ellipsis (session titles can be long).
fn truncate_title(title: &str, max: usize) -> String {
    let mut chars = title.chars();
    let truncated: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

fn session_row_text(session: &SessionDisplay, target: &Path, now_ms: i64) -> String {
    format!(
        "{} — {} · {}",
        truncate_title(&session.title, 40),
        relative_to_target(target, &session.directory),
        format_age_ms(session.time_updated, now_ms)
    )
}

fn pane_row_text(pane: &PaneCandidate, dir: Option<&Path>, triggered: Option<&str>) -> String {
    let mut parts = vec![pane.info.pane_id.clone()];
    match (&pane.info.agent, &pane.info.agent_status) {
        (Some(agent), Some(status)) => parts.push(format!("{agent} {status}")),
        (Some(agent), None) => parts.push(agent.clone()),
        (None, Some(status)) => parts.push(status.clone()),
        (None, None) => {}
    }
    if let (Some(target), Some(cwd)) = (dir, pane.info.cwd.as_deref()) {
        parts.push(relative_to_target(target, cwd));
    }
    if pane.matched_via == PaneMatch::ForegroundOnly {
        parts.push("[^fg]".to_string());
    }
    if triggered == Some(pane.info.pane_id.as_str()) {
        parts.push("(triggered here)".to_string());
    }
    parts.join(" · ")
}

fn group_label(id: &str, labels: &HashMap<String, String>, fallback: &str) -> String {
    if id.is_empty() {
        return fallback.to_string();
    }
    labels.get(id).cloned().unwrap_or_else(|| id.to_string())
}

/// Group candidates herdr-workspace → tab, preserving first-seen order.
fn group_panes(panes: &[PaneCandidate]) -> Vec<(String, Vec<(String, Vec<&PaneCandidate>)>)> {
    let mut groups: Vec<(String, Vec<(String, Vec<&PaneCandidate>)>)> = Vec::new();
    for pane in panes {
        let workspace_index = match groups
            .iter()
            .position(|(id, _)| *id == pane.info.workspace_id)
        {
            Some(index) => index,
            None => {
                groups.push((pane.info.workspace_id.clone(), Vec::new()));
                groups.len() - 1
            }
        };
        let tabs = &mut groups[workspace_index].1;
        let tab_index = match tabs.iter().position(|(id, _)| *id == pane.info.tab_id) {
            Some(index) => index,
            None => {
                tabs.push((pane.info.tab_id.clone(), Vec::new()));
                tabs.len() - 1
            }
        };
        tabs[tab_index].1.push(pane);
    }
    groups
}

/// Build the flat review row list. Pure: rendering and the selection model
/// share it, and it is the single place the stale-target skip rules live.
fn build_review_rows(data: &ReviewData) -> Vec<ReviewRow> {
    let mut rows = Vec::new();

    rows.push(ReviewRow::section("Workspace"));
    rows.push(ReviewRow::note(
        format!("target    {}", data.target_label),
        1,
        RowTone::Normal,
    ));
    rows.push(ReviewRow::note(
        format!("main repo {}", display_home_path(&data.main_repo)),
        1,
        RowTone::Dim,
    ));

    rows.push(ReviewRow::section("Plan"));

    // 1. migrate opencode sessions -----------------------------------------
    rows.push(ReviewRow::task("1. migrate opencode sessions", "migrate"));
    if !data.availability.migrate {
        rows.push(ReviewRow::note(
            "skipped (workspace path unknown)",
            2,
            RowTone::Dim,
        ));
    } else {
        match &data.sessions {
            SessionPreview::Skipped(reason) => rows.push(ReviewRow::note(
                format!("skipped: {reason}"),
                2,
                RowTone::Dim,
            )),
            SessionPreview::Refused(reason) => rows.push(ReviewRow::note(
                format!("blocked: {reason}"),
                2,
                RowTone::Error,
            )),
            SessionPreview::Ready { rows: sessions, .. } => {
                if sessions.is_empty() {
                    rows.push(ReviewRow::note(
                        "no sessions bound to this workspace",
                        2,
                        RowTone::Dim,
                    ));
                } else if let Some(target) = data.dir.as_deref() {
                    rows.push(ReviewRow::count_note(SESSION_COUNT_ID, 2));
                    for session in sessions {
                        rows.push(ReviewRow::session(
                            session.id.clone(),
                            session_row_text(session, target, data.now_ms),
                        ));
                    }
                }
            }
        }
    }

    // 2. jj workspace forget -------------------------------------------------
    rows.push(ReviewRow::task("2. jj workspace forget", "forget"));
    if data.availability.forget {
        rows.push(ReviewRow::note(
            "commits and bookmarks stay in the shared repo store",
            2,
            RowTone::Dim,
        ));
    } else {
        rows.push(ReviewRow::note("skipped", 2, RowTone::Dim));
    }

    // 3. delete directory ----------------------------------------------------
    rows.push(ReviewRow::task("3. delete directory", "delete"));
    if !data.availability.delete {
        rows.push(ReviewRow::note(
            "skipped (workspace path unknown)",
            2,
            RowTone::Dim,
        ));
    } else if let Some(dir) = data.dir.as_deref() {
        if dir.is_dir() {
            rows.push(ReviewRow::note(display_home_path(dir), 2, RowTone::Normal));
        } else {
            rows.push(ReviewRow::note(
                format!("already missing: {}", display_home_path(dir)),
                2,
                RowTone::Dim,
            ));
        }
    }

    // 4. close panes ---------------------------------------------------------
    rows.push(ReviewRow::task("4. close panes", "close"));
    if !data.availability.close_panes {
        rows.push(ReviewRow::note(
            "skipped (workspace path unknown)",
            2,
            RowTone::Dim,
        ));
    } else if data.panes.is_empty() {
        rows.push(ReviewRow::note(
            "no panes inside the target",
            2,
            RowTone::Dim,
        ));
    } else {
        rows.push(ReviewRow::count_note(PANE_COUNT_ID, 2));
        for (workspace_id, tabs) in group_panes(&data.panes) {
            rows.push(ReviewRow::group(
                group_label(&workspace_id, &data.workspace_labels, "(unknown workspace)"),
                2,
            ));
            for (tab_id, panes) in tabs {
                rows.push(ReviewRow::group(
                    group_label(&tab_id, &data.tab_labels, "(unknown tab)"),
                    3,
                ));
                for pane in panes {
                    rows.push(ReviewRow::pane(
                        pane.info.pane_id.clone(),
                        pane_row_text(pane, data.dir.as_deref(), data.triggered_pane.as_deref()),
                    ));
                }
            }
        }
        rows.push(ReviewRow::warning());
    }

    // Checks -----------------------------------------------------------------
    rows.push(ReviewRow::section("Checks"));
    match &data.clean {
        None => rows.push(ReviewRow::check(
            "· clean working copy: skipped (workspace path unknown)",
            RowTone::Dim,
        )),
        Some(Ok(changes)) if changes.is_empty() => {
            rows.push(ReviewRow::check("✓ clean working copy", RowTone::Ok))
        }
        Some(Ok(changes)) => {
            rows.push(ReviewRow::check(
                format!(
                    "✗ clean working copy: {} uncommitted change(s)",
                    changes.len()
                ),
                RowTone::Error,
            ));
            for line in changes.iter().take(3) {
                rows.push(ReviewRow::note(format!("  {line}"), 2, RowTone::Dim));
            }
            if changes.len() > 3 {
                rows.push(ReviewRow::note(
                    format!("  … {} more", changes.len() - 3),
                    2,
                    RowTone::Dim,
                ));
            }
            rows.push(ReviewRow::note(
                "preserve: jj commit -m \"<message>\" (press c)",
                2,
                RowTone::Dim,
            ));
            rows.push(ReviewRow::note(
                "discard:  jj restore (not run by this dialog)",
                2,
                RowTone::Dim,
            ));
        }
        Some(Err(message)) => {
            rows.push(ReviewRow::check(
                "✗ clean working copy: check failed (refusing to remove)",
                RowTone::Error,
            ));
            let first = message.lines().next().unwrap_or(message);
            rows.push(ReviewRow::note(first.to_string(), 2, RowTone::Dim));
            rows.push(ReviewRow::note(
                "fix the jj repository first; commit/restore cannot help",
                2,
                RowTone::Dim,
            ));
        }
    }
    if !data.availability.migrate {
        rows.push(ReviewRow::check(
            "· opencode sessions: skipped (workspace path unknown)",
            RowTone::Dim,
        ));
    } else {
        match &data.sessions {
            SessionPreview::Skipped(reason) => rows.push(ReviewRow::check(
                format!("· opencode sessions: skipped ({reason})"),
                RowTone::Dim,
            )),
            SessionPreview::Refused(reason) => {
                rows.push(ReviewRow::check(
                    "✗ opencode sessions: DB unreadable (refusing to remove)",
                    RowTone::Error,
                ));
                rows.push(ReviewRow::note(reason.clone(), 2, RowTone::Dim));
                rows.push(ReviewRow::note(
                    "resolve this outside the dialog, then reopen it",
                    2,
                    RowTone::Dim,
                ));
            }
            SessionPreview::Ready {
                rows: sessions,
                main_repo,
            } => {
                rows.push(ReviewRow::check(
                    format!("✓ opencode sessions: {} bound", sessions.len()),
                    RowTone::Ok,
                ));
                rows.push(ReviewRow::note(
                    format!("migrate to {}", display_home_path(main_repo)),
                    2,
                    RowTone::Dim,
                ));
            }
        }
    }

    rows
}

/// Why `↵` must not authorize yet: `Some(summary)` when any check is ✗.
/// Path-unknown targets skip the clean check, so nothing blocks them there.
fn review_blocking_reason(data: &ReviewData) -> Option<String> {
    match &data.clean {
        Some(Err(message)) => {
            let first = message.lines().next().unwrap_or(message);
            Some(format!("blocked: {first}"))
        }
        Some(Ok(changes)) if !changes.is_empty() => Some(format!(
            "blocked: {} uncommitted change(s) — press c to commit, or run jj restore outside the dialog",
            changes.len()
        )),
        _ => match &data.sessions {
            SessionPreview::Refused(reason) => {
                let first = reason.lines().next().unwrap_or(reason);
                Some(format!(
                    "blocked: opencode sessions cannot be migrated: {first}"
                ))
            }
            _ => None,
        },
    }
}

/// Default verdict of a group header checkbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GroupSelection {
    All,
    None,
    Partial,
}

/// Pure selection/scroll state over the flat review rows: cursor movement,
/// leaf toggles, group cascade, global toggle, counts and warning text.
/// `scroll` is independent of the cursor (wheel scrolling never moves the
/// selection; cursor moves re-follow it). Rendering reads the same rows and
/// helpers, so the view needs no parallel state.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReviewModel {
    rows: Vec<ReviewRow>,
    cursor: usize,
    scroll: usize,
    selected_sessions: HashSet<String>,
    selected_panes: HashSet<String>,
}

impl ReviewModel {
    /// Build the model with every session and pane selected (spec default).
    fn new(rows: Vec<ReviewRow>) -> ReviewModel {
        let mut model = ReviewModel {
            rows,
            cursor: 0,
            scroll: 0,
            selected_sessions: HashSet::new(),
            selected_panes: HashSet::new(),
        };
        model.select_all(true);
        model.cursor = model.first_toggleable().unwrap_or(0);
        model
    }

    fn is_selected(&self, kind: RowKind, id: &str) -> bool {
        match kind {
            RowKind::Session => self.selected_sessions.contains(id),
            RowKind::Pane => self.selected_panes.contains(id),
            _ => false,
        }
    }

    fn set_selected(&mut self, kind: RowKind, id: &str, selected: bool) {
        let set = match kind {
            RowKind::Session => &mut self.selected_sessions,
            RowKind::Pane => &mut self.selected_panes,
            _ => return,
        };
        if selected {
            set.insert(id.to_string());
        } else {
            set.remove(id);
        }
    }

    /// Select or clear every session and pane.
    fn select_all(&mut self, selected: bool) {
        let leaves: Vec<(RowKind, String)> = self
            .rows
            .iter()
            .filter_map(|row| match row.kind {
                RowKind::Session | RowKind::Pane => row.id.clone().map(|id| (row.kind, id)),
                _ => None,
            })
            .collect();
        for (kind, id) in leaves {
            self.set_selected(kind, &id, selected);
        }
    }

    /// A group header (herdr workspace / tab) has descendants when a deeper
    /// leaf follows before the next row at its own depth or shallower.
    fn has_descendants(&self, index: usize) -> bool {
        let Some(row) = self.rows.get(index) else {
            return false;
        };
        let depth = row.depth;
        self.rows[index + 1..]
            .iter()
            .take_while(|next| next.depth > depth)
            .any(|next| matches!(next.kind, RowKind::Session | RowKind::Pane))
    }

    /// Leaves and group headers can be toggled/cursored. Task rows are plain
    /// text by design: the hierarchical lists underneath carry selection.
    fn is_toggleable(&self, index: usize) -> bool {
        match self.rows.get(index) {
            Some(row) => match row.kind {
                RowKind::Session | RowKind::Pane => row.id.is_some(),
                RowKind::Group => self.has_descendants(index),
                _ => false,
            },
            None => false,
        }
    }

    fn first_toggleable(&self) -> Option<usize> {
        (0..self.rows.len()).find(|&index| self.is_toggleable(index))
    }

    /// Move the cursor to the next/previous toggleable row; stays put when
    /// there is none in that direction.
    fn move_cursor(&mut self, delta: isize) {
        let mut index = self.cursor as isize + delta;
        while index >= 0 && (index as usize) < self.rows.len() {
            if self.is_toggleable(index as usize) {
                self.cursor = index as usize;
                return;
            }
            index += delta;
        }
    }

    /// Space on a leaf toggles just it; on a group header (herdr workspace /
    /// tab) it cascades to every descendant leaf (a header with no leaves is
    /// inert). Task rows are never toggleable, so `space` is a no-op there.
    fn toggle_current(&mut self) {
        let index = self.cursor;
        let Some((kind, depth, id)) = self
            .rows
            .get(index)
            .map(|row| (row.kind, row.depth, row.id.clone()))
        else {
            return;
        };
        match kind {
            RowKind::Session | RowKind::Pane => {
                if let Some(id) = id {
                    let selected = !self.is_selected(kind, &id);
                    self.set_selected(kind, &id, selected);
                }
            }
            RowKind::Group => {
                let select = self.group_state(index) != GroupSelection::All;
                let toggles: Vec<(RowKind, String)> = self.rows[index + 1..]
                    .iter()
                    .take_while(|next| next.depth > depth)
                    .filter_map(|next| match next.kind {
                        RowKind::Session | RowKind::Pane => {
                            next.id.clone().map(|id| (next.kind, id))
                        }
                        _ => None,
                    })
                    .collect();
                for (kind, id) in toggles {
                    self.set_selected(kind, &id, select);
                }
            }
            _ => {}
        }
    }

    /// `a`: if everything is selected, clear all; otherwise select all.
    fn toggle_all(&mut self) {
        let all_selected = {
            let (session_selected, session_total) = self.session_count();
            let (pane_selected, pane_total) = self.pane_count();
            session_selected == session_total && pane_selected == pane_total
        };
        self.select_all(!all_selected);
    }

    fn leaf_count(&self, kind: RowKind) -> (usize, usize) {
        let mut selected = 0;
        let mut total = 0;
        for row in &self.rows {
            if row.kind != kind {
                continue;
            }
            if let Some(id) = row.id.as_deref() {
                total += 1;
                if self.is_selected(kind, id) {
                    selected += 1;
                }
            }
        }
        (selected, total)
    }

    fn session_count(&self) -> (usize, usize) {
        self.leaf_count(RowKind::Session)
    }

    fn pane_count(&self) -> (usize, usize) {
        self.leaf_count(RowKind::Pane)
    }

    /// Live "N of M selected" text for a leaf list; `None` when the list has
    /// no leaves (the row then renders nothing).
    fn count_text(&self, kind: RowKind) -> Option<String> {
        let (selected, total) = self.leaf_count(kind);
        (total > 0).then(|| format!("{selected} of {total} selected"))
    }

    fn session_count_text(&self) -> Option<String> {
        self.count_text(RowKind::Session)
    }

    fn pane_count_text(&self) -> Option<String> {
        self.count_text(RowKind::Pane)
    }

    /// Selected/total leaves under the group/task header at `index`.
    fn group_counts(&self, index: usize) -> (usize, usize) {
        let Some(row) = self.rows.get(index) else {
            return (0, 0);
        };
        let depth = row.depth;
        let mut selected = 0;
        let mut total = 0;
        for next in self.rows[index + 1..]
            .iter()
            .take_while(|next| next.depth > depth)
        {
            if matches!(next.kind, RowKind::Session | RowKind::Pane) {
                if let Some(id) = next.id.as_deref() {
                    total += 1;
                    if self.is_selected(next.kind, id) {
                        selected += 1;
                    }
                }
            }
        }
        (selected, total)
    }

    fn group_state(&self, index: usize) -> GroupSelection {
        let (selected, total) = self.group_counts(index);
        if total == 0 || selected == 0 {
            GroupSelection::None
        } else if selected == total {
            GroupSelection::All
        } else {
            GroupSelection::Partial
        }
    }

    /// Panes the user unchecked: they keep running while their cwd vanishes.
    fn unselected_pane_count(&self) -> usize {
        let (selected, total) = self.pane_count();
        total - selected
    }

    fn pane_warning_text(&self) -> Option<String> {
        let unselected = self.unselected_pane_count();
        if unselected == 0 {
            return None;
        }
        let total = self.pane_count().1;
        if unselected == total {
            Some(format!(
                "all {total} pane(s) keep running; their cwd will be deleted"
            ))
        } else {
            Some(format!(
                "{unselected} pane(s) keep running; their cwd will be deleted"
            ))
        }
    }

    fn selected_leaf_ids(&self, kind: RowKind) -> Vec<String> {
        self.rows
            .iter()
            .filter(|row| row.kind == kind)
            .filter_map(|row| {
                row.id
                    .as_deref()
                    .filter(|id| self.is_selected(kind, id))
                    .map(str::to_string)
            })
            .collect()
    }

    fn selected_session_ids(&self) -> Vec<String> {
        self.selected_leaf_ids(RowKind::Session)
    }

    fn selected_pane_ids(&self) -> Vec<String> {
        self.selected_leaf_ids(RowKind::Pane)
    }

    /// Stored offset clamped to the current content/viewport bounds. The
    /// cursor does not move it; [`ReviewModel::follow_cursor`] does, after
    /// cursor moves.
    fn scroll_offset(&self, view_height: usize) -> usize {
        clamp_scroll(self.scroll, view_height, self.rows.len())
    }

    /// Manual scroll (wheel): clamped to the content bounds, never moves the
    /// cursor.
    fn scroll_by(&mut self, delta: isize, view_height: usize, content_height: usize) {
        let next = (self.scroll as isize + delta).max(0) as usize;
        self.scroll = clamp_scroll(next, view_height, content_height);
    }

    /// Re-adjust the stored offset just enough to keep the cursor visible
    /// (the pre-wheel `scroll_offset` semantics).
    fn follow_cursor(&mut self, view_height: usize) {
        self.scroll = follow_cursor_offset(self.cursor, self.scroll, view_height);
    }

    /// Rebuild rows (e.g. after a commit refreshed the checks) keeping the
    /// current selection; the cursor is clamped to a toggleable row.
    fn replace_rows(&mut self, rows: Vec<ReviewRow>) {
        self.rows = rows;
        if self.cursor >= self.rows.len() {
            self.cursor = self.rows.len().saturating_sub(1);
        }
        if !self.is_toggleable(self.cursor) {
            self.cursor = self.first_toggleable().unwrap_or(0);
        }
    }

    /// The authorized plan. `dir: None` (picker `missing on disk`) yields
    /// empty session/pane lists: there is no known working directory to scope
    /// them to, and only forget runs.
    fn to_plan(
        &self,
        dir: Option<PathBuf>,
        main_repo: PathBuf,
        workspace_name: Option<String>,
    ) -> RemovePlan {
        let path_known = dir.is_some();
        RemovePlan {
            dir,
            main_repo,
            workspace_name,
            session_ids: if path_known {
                self.selected_session_ids()
            } else {
                Vec::new()
            },
            pane_ids: if path_known {
                self.selected_pane_ids()
            } else {
                Vec::new()
            },
        }
    }
}

/// The four removal pipeline stages, in execution order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusTask {
    Migrate,
    Forget,
    Delete,
    ClosePanes,
}

impl StatusTask {
    fn label(self) -> &'static str {
        match self {
            StatusTask::Migrate => "migrate opencode sessions",
            StatusTask::Forget => "jj workspace forget",
            StatusTask::Delete => "delete directory",
            StatusTask::ClosePanes => "close panes",
        }
    }
}

/// One task's display state in the Status view.
#[derive(Debug, Clone, PartialEq, Eq)]
enum StatusItemState {
    Pending,
    Running,
    Done(String),
    Skipped(String),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StatusItem {
    task: StatusTask,
    state: StatusItemState,
}

/// Pure Status view model: the four ordered items plus the optional
/// `error.log` pointer shown on failure.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StatusState {
    items: Vec<StatusItem>,
    error_log: Option<String>,
}

impl StatusState {
    /// Fresh state for a plan. A target whose working directory is unknown
    /// (picker `missing on disk`) can only run `jj workspace forget`; the
    /// path-scoped tasks start Skipped with the reason.
    fn new(dir: Option<&Path>) -> StatusState {
        let mut state = StatusState {
            items: Vec::new(),
            error_log: None,
        };
        for task in [
            StatusTask::Migrate,
            StatusTask::Forget,
            StatusTask::Delete,
            StatusTask::ClosePanes,
        ] {
            let path_scoped = matches!(
                task,
                StatusTask::Migrate | StatusTask::Delete | StatusTask::ClosePanes
            );
            let item_state = if dir.is_none() && path_scoped {
                StatusItemState::Skipped("skipped: workspace path unknown".to_string())
            } else {
                StatusItemState::Pending
            };
            state.items.push(StatusItem {
                task,
                state: item_state,
            });
        }
        state
    }

    fn set(&mut self, task: StatusTask, state: StatusItemState) {
        if let Some(item) = self.items.iter_mut().find(|item| item.task == task) {
            item.state = state;
        }
    }

    fn running(&mut self, task: StatusTask) {
        self.set(task, StatusItemState::Running);
    }

    fn done(&mut self, task: StatusTask, message: impl Into<String>) {
        self.set(task, StatusItemState::Done(message.into()));
    }

    fn skipped(&mut self, task: StatusTask, message: impl Into<String>) {
        self.set(task, StatusItemState::Skipped(message.into()));
    }

    fn failed(&mut self, task: StatusTask, message: impl Into<String>) {
        self.set(task, StatusItemState::Failed(message.into()));
    }

    fn set_error_log(&mut self, path: impl Into<String>) {
        self.error_log = Some(path.into());
    }

    fn any_failure(&self) -> bool {
        self.items
            .iter()
            .any(|item| matches!(item.state, StatusItemState::Failed(_)))
    }

    fn first_failure(&self) -> Option<(StatusTask, &str)> {
        self.items.iter().find_map(|item| match &item.state {
            StatusItemState::Failed(message) => Some((item.task, message.as_str())),
            _ => None,
        })
    }
}

/// Review modes (the commit sub-mode swaps the hint and the status line).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DialogMode {
    Review,
    Commit,
}

/// Adaptive modal height: content-sized, clamped to
/// `[31, min(area.height - 4, 45)]` (the wizard's fixed 26 is the floor).
fn remove_modal_height(area: Rect, content_height: u16) -> u16 {
    let cap = area.height.saturating_sub(4).min(45);
    let floor = 31.min(cap);
    content_height.clamp(floor, cap.max(floor))
}

/// Scrollable content viewport of the remove modal in `area` for `row_count`
/// rows: the adaptive modal height minus the fixed chrome (header, hint,
/// blank, status line, button row = 7 rows). Mirrors the draw layout.
fn dialog_view_height(area: Rect, row_count: usize) -> usize {
    remove_modal_height(area, (row_count as u16).saturating_add(7)).saturating_sub(7) as usize
}

/// Clamp a scroll offset to `[0, content_height - view_height]`; content that
/// fits the viewport stays at 0.
fn clamp_scroll(offset: usize, view_height: usize, content_height: usize) -> usize {
    if view_height == 0 {
        return 0;
    }
    offset.min(content_height.saturating_sub(view_height))
}

/// Minimal adjustment of `offset` that brings `cursor` back into view.
fn follow_cursor_offset(cursor: usize, offset: usize, view_height: usize) -> usize {
    if view_height == 0 {
        return offset;
    }
    if cursor < offset {
        cursor
    } else if cursor >= offset + view_height {
        cursor + 1 - view_height
    } else {
        offset
    }
}

/// Vertical scrollbar for a list viewport: `area` is the 1-column strip on
/// the modal's right content edge; nothing is drawn when the list fits.
fn render_list_scrollbar(
    frame: &mut Frame,
    area: Rect,
    content_len: usize,
    view_height: usize,
    offset: usize,
    p: &Palette,
) {
    if view_height == 0 || content_len <= view_height {
        return;
    }
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(Some("▲"))
        .end_symbol(Some("▼"))
        .track_symbol(Some("┃"))
        .track_style(Style::default().fg(p.overlay0))
        .thumb_symbol("█")
        .thumb_style(Style::default().fg(p.subtext0));
    let mut state = ScrollbarState::new(content_len)
        .viewport_content_length(view_height)
        .position(offset);
    frame.render_stateful_widget(scrollbar, area, &mut state);
}

/// Centered action + cancel row with the wizard's geometry, but with dialog
/// labels (`remove` is destructive, the picker uses `select`).
fn remove_button_rects(inner: Rect, action: &str, cancel: &str) -> (Rect, Rect) {
    let action_text = action_button_text(Some("↵"), action);
    let cancel_text = action_button_text(Some("esc"), cancel);
    let action_w = action_text.chars().count() as u16;
    let cancel_w = cancel_text.chars().count() as u16;
    let gap = 2u16;
    let mut x = inner.x + inner.width.saturating_sub(action_w + cancel_w + gap) / 2;
    let y = inner.y + inner.height.saturating_sub(1);
    let action_rect = Rect::new(x, y, action_w, 1);
    x = x.saturating_add(action_w).saturating_add(gap);
    (action_rect, Rect::new(x, y, cancel_w, 1))
}

/// One review row: `›` cursor (toggleable rows only), depth indent aligned
/// with `SECTION_CONTENT_INDENT`, checkbox for selectable leaves and group
/// headers. Section titles render flush at the row area's left edge, exactly
/// like the create wizard's; they are never a cursor target.
fn draw_review_row(
    frame: &mut Frame,
    area: Rect,
    model: &ReviewModel,
    index: usize,
    row: &ReviewRow,
    p: &Palette,
) {
    if row.kind == RowKind::Section {
        frame.render_widget(
            Paragraph::new(row.text.clone()).style(section_title_style(false, p)),
            area,
        );
        return;
    }
    let cursor = if index == model.cursor && model.is_toggleable(index) {
        "›"
    } else {
        " "
    };
    let depth = "  ".repeat(usize::from(row.depth));
    let marker = match row.kind {
        RowKind::Session | RowKind::Pane => {
            let id = row.id.as_deref().unwrap_or_default();
            if model.is_selected(row.kind, id) {
                "[x] "
            } else {
                "[ ] "
            }
        }
        RowKind::Group if model.has_descendants(index) => match model.group_state(index) {
            GroupSelection::All => "[x] ",
            GroupSelection::None => "[ ] ",
            GroupSelection::Partial => "[-] ",
        },
        _ => "",
    };
    let text = match row.id.as_deref() {
        Some(PANE_WARNING_ID) => model.pane_warning_text().unwrap_or_default(),
        Some(SESSION_COUNT_ID) => model.session_count_text().unwrap_or_default(),
        Some(PANE_COUNT_ID) => model.pane_count_text().unwrap_or_default(),
        _ => row.text.clone(),
    };
    let mut style = match row.kind {
        RowKind::Task => Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        _ => match row.tone {
            RowTone::Normal => Style::default().fg(p.text),
            RowTone::Dim => Style::default().fg(p.overlay0),
            RowTone::Accent => Style::default().fg(p.accent),
            RowTone::Error => Style::default().fg(p.red),
            RowTone::Ok => Style::default().fg(p.green),
            RowTone::Warning => Style::default().fg(p.yellow),
        },
    };
    if index == model.cursor && model.is_toggleable(index) {
        style = style.add_modifier(Modifier::BOLD);
    }
    frame.render_widget(
        Paragraph::new(format!("{cursor}  {depth}{marker}{text}")).style(style),
        area,
    );
}

fn draw_review_dialog(
    frame: &mut Frame,
    model: &ReviewModel,
    mode: DialogMode,
    error: Option<&str>,
    commit_message: &str,
    commit_cursor: usize,
) {
    let p = catppuccin();
    let area = frame.area();
    dim_background(frame, area);
    let desired = remove_modal_height(area, (model.rows.len() as u16).saturating_add(7));
    let Some(inner) = render_modal_shell(frame, area, REMOVE_MODAL_WIDTH, desired, &p) else {
        return;
    };
    if inner.height < 8 {
        return;
    }

    let hint = match mode {
        DialogMode::Review => REVIEW_HINT,
        DialogMode::Commit => COMMIT_HINT,
    };
    let mut y = inner.y;
    render_modal_header(
        frame,
        Rect::new(inner.x, y, inner.width, 1),
        "Remove jj workspace",
        &p,
    );
    y += 1;
    frame.render_widget(
        Paragraph::new(hint).style(Style::default().fg(p.overlay0)),
        Rect::new(inner.x, y, inner.width, 1),
    );
    y += 2; // hint + blank separator

    let buttons_y = inner.y + inner.height.saturating_sub(1);
    let status_y = buttons_y.saturating_sub(1);
    let content_height = status_y.saturating_sub(y) as usize;
    let offset = model.scroll_offset(content_height);
    for (row_index, row) in model
        .rows
        .iter()
        .enumerate()
        .skip(offset)
        .take(content_height)
    {
        let rect = Rect::new(inner.x, y + (row_index - offset) as u16, inner.width, 1);
        draw_review_row(frame, rect, model, row_index, row, &p);
    }

    // Vertical scrollbar on the right edge inside the border, only while the
    // rows overflow the viewport.
    render_list_scrollbar(
        frame,
        Rect::new(
            inner.x + inner.width.saturating_sub(1),
            y,
            1,
            content_height as u16,
        ),
        model.rows.len(),
        content_height,
        offset,
        &p,
    );

    // Blocked/error/commit line, always directly above the buttons.
    let (status_text, status_style) = match mode {
        DialogMode::Commit => (
            format!(
                "c commit> {}",
                render_line_with_cursor(commit_message, commit_cursor, true)
            ),
            Style::default().fg(p.text),
        ),
        DialogMode::Review => match error {
            Some(message) => (message.to_string(), Style::default().fg(p.red)),
            None => (String::new(), Style::default().fg(p.overlay0)),
        },
    };
    frame.render_widget(
        Paragraph::new(status_text)
            .style(status_style)
            .wrap(Wrap { trim: false }),
        Rect::new(inner.x, status_y, inner.width, 1),
    );

    let (action_rect, cancel_rect) = remove_button_rects(inner, "remove", "cancel");
    render_action_button(
        frame,
        action_rect,
        Some("↵"),
        "remove",
        Style::default()
            .fg(panel_contrast_fg(&p))
            .bg(p.red)
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

/// Picker-local state: the enumerated secondary workspaces, the cursor, the
/// persistent scroll offset (wheel; independent of the cursor) and the load
/// error (if `jj workspace list` failed).
struct PickerState {
    entries: Vec<WorkspaceEntry>,
    cursor: usize,
    scroll: usize,
    error: Option<String>,
}

fn draw_picker(frame: &mut Frame, state: &PickerState) {
    let p = catppuccin();
    let area = frame.area();
    dim_background(frame, area);
    let desired = remove_modal_height(area, (state.entries.len() as u16).saturating_add(7));
    let Some(inner) = render_modal_shell(frame, area, REMOVE_MODAL_WIDTH, desired, &p) else {
        return;
    };
    if inner.height < 8 {
        return;
    }

    let mut y = inner.y;
    render_modal_header(
        frame,
        Rect::new(inner.x, y, inner.width, 1),
        "Remove jj workspace",
        &p,
    );
    y += 1;
    frame.render_widget(
        Paragraph::new(PICKER_HINT).style(Style::default().fg(p.overlay0)),
        Rect::new(inner.x, y, inner.width, 1),
    );
    y += 2;

    let buttons_y = inner.y + inner.height.saturating_sub(1);
    let status_y = buttons_y.saturating_sub(1);
    let content_height = status_y.saturating_sub(y) as usize;

    if state.entries.is_empty() && state.error.is_none() {
        frame.render_widget(
            Paragraph::new("no secondary jj workspaces").style(Style::default().fg(p.overlay0)),
            Rect::new(inner.x, y, inner.width, 1),
        );
    }
    let offset = clamp_scroll(state.scroll, content_height, state.entries.len());
    if content_height > 0 && !state.entries.is_empty() {
        for (index, entry) in state
            .entries
            .iter()
            .enumerate()
            .skip(offset)
            .take(content_height)
        {
            let is_cursor = index == state.cursor;
            let cursor = if is_cursor { "›" } else { " " };
            let (path, tone) = match &entry.root {
                Some(root) => (display_home_path(root), p.text),
                None => ("(missing on disk)".to_string(), p.overlay0),
            };
            let style = if is_cursor {
                Style::default().fg(p.accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(tone)
            };
            frame.render_widget(
                Paragraph::new(format!("{cursor}  {:<28} {path}", entry.name)).style(style),
                Rect::new(inner.x, y + (index - offset) as u16, inner.width, 1),
            );
        }
    }

    // Same scrollbar placement/styling as the review list.
    render_list_scrollbar(
        frame,
        Rect::new(
            inner.x + inner.width.saturating_sub(1),
            y,
            1,
            content_height as u16,
        ),
        state.entries.len(),
        content_height,
        offset,
        &p,
    );

    let error_text = state.error.clone().unwrap_or_default();
    frame.render_widget(
        Paragraph::new(error_text)
            .style(Style::default().fg(p.red))
            .wrap(Wrap { trim: false }),
        Rect::new(inner.x, status_y, inner.width, 1),
    );

    let (action_rect, cancel_rect) = remove_button_rects(inner, "select", "cancel");
    render_action_button(
        frame,
        action_rect,
        Some("↵"),
        "select",
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

/// Picker event loop. Returns the chosen entry or `None` on esc / load error
/// / terminal failure (fail-closed: no removal).
fn run_picker(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    jj: &ResolvedJj,
    main_root: &Path,
) -> Option<WorkspaceEntry> {
    let mut state = PickerState {
        entries: Vec::new(),
        cursor: 0,
        scroll: 0,
        error: None,
    };
    match list_secondary_workspaces(jj, main_root) {
        Ok(entries) => state.entries = entries,
        Err(message) => state.error = Some(message),
    }
    loop {
        let view_height = terminal
            .size()
            .map(|size| {
                dialog_view_height(
                    Rect::new(0, 0, size.width, size.height),
                    state.entries.len(),
                )
            })
            .unwrap_or(0);
        let _ = terminal.draw(|frame| draw_picker(frame, &state));
        match event::read() {
            Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Esc => return None,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return None,
                KeyCode::Up | KeyCode::Char('k') => {
                    state.cursor = state.cursor.saturating_sub(1);
                    state.scroll = follow_cursor_offset(state.cursor, state.scroll, view_height);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if state.cursor + 1 < state.entries.len() {
                        state.cursor += 1;
                    }
                    state.scroll = follow_cursor_offset(state.cursor, state.scroll, view_height);
                }
                KeyCode::Enter => {
                    if state.error.is_none() {
                        if let Some(entry) = state.entries.get(state.cursor) {
                            return Some(entry.clone());
                        }
                    }
                }
                _ => {}
            },
            Ok(Event::Mouse(mouse)) => {
                let delta = match mouse.kind {
                    MouseEventKind::ScrollUp => -3isize,
                    MouseEventKind::ScrollDown => 3,
                    _ => 0,
                };
                if delta != 0 {
                    state.scroll = clamp_scroll(
                        (state.scroll as isize + delta).max(0) as usize,
                        view_height,
                        state.entries.len(),
                    );
                }
            }
            Ok(_) => {}
            Err(_) => return None,
        }
    }
}

/// Collect the review screen's data. `dir: None` means the picker chose a
/// `missing on disk` entry: migration, deletion and pane closing cannot be
/// scoped, so no scan/check is attempted and the rows say skipped.
fn gather_review_data(
    jj: &ResolvedJj,
    dir: Option<PathBuf>,
    main_repo: PathBuf,
    workspace_name: Option<String>,
    herdr: &str,
) -> ReviewData {
    let target_label = match (&dir, &workspace_name) {
        (Some(path), _) => display_home_path(path),
        (None, Some(name)) => format!("{name} (missing on disk)"),
        (None, None) => "(missing on disk)".to_string(),
    };
    let availability = plan_availability(dir.as_deref());
    let (clean, sessions, panes, workspace_labels, tab_labels) = if let Some(path) = dir.as_deref()
    {
        let (workspace_labels, tab_labels) = pane_group_labels(herdr);
        (
            Some(workspace_change_lines(jj, path)),
            session_preview(path, &main_repo),
            scan_pane_candidates(herdr, path, &plugin_root()),
            workspace_labels,
            tab_labels,
        )
    } else {
        (
            None,
            SessionPreview::Skipped("workspace path unknown".to_string()),
            Vec::new(),
            HashMap::new(),
            HashMap::new(),
        )
    };
    let triggered_pane = json_string_field(
        &env::var("HERDR_PLUGIN_CONTEXT_JSON").unwrap_or_default(),
        "focused_pane_id",
    )
    .filter(|id| !id.is_empty());

    ReviewData {
        dir,
        main_repo,
        workspace_name,
        target_label,
        availability,
        clean,
        sessions,
        panes,
        workspace_labels,
        tab_labels,
        triggered_pane,
        now_ms: now_millis(),
    }
}

/// Run the full remove dialog: picker (main workspace only), review, commit
/// sub-mode. Returns the authorized plan or `Cancelled` (esc / terminal
/// failure — neither produces a change). IO failures fail closed.
fn run_remove_dialog(jj: &ResolvedJj, target: &RemoveTarget, herdr: &str) -> ReviewOutcome {
    if enable_raw_mode().is_err() {
        return ReviewOutcome::Cancelled;
    }
    let mut out = io::stdout();
    if execute!(out, EnterAlternateScreen).is_err() {
        let _ = disable_raw_mode();
        return ReviewOutcome::Cancelled;
    }
    // Mouse reporting so herdr forwards real wheel events instead of its
    // alternate-scroll Up/Down translation (review scroll never moves the
    // cursor). Every exit path either goes through `restore_terminal` or the
    // explicit cleanup below.
    if execute!(out, EnableMouseCapture).is_err() {
        let _ = execute!(out, LeaveAlternateScreen);
        let _ = disable_raw_mode();
        return ReviewOutcome::Cancelled;
    }
    let mut terminal = match Terminal::new(CrosstermBackend::new(out)) {
        Ok(terminal) => terminal,
        Err(_) => {
            let _ = execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
            let _ = disable_raw_mode();
            return ReviewOutcome::Cancelled;
        }
    };

    // Picker first when the caller sits in the main workspace.
    let (workspace_name, dir, main_repo) = match target {
        RemoveTarget::Main { main_root } => match run_picker(&mut terminal, jj, main_root) {
            Some(entry) => (Some(entry.name), entry.root, main_root.clone()),
            None => {
                let _ = restore_terminal(&mut terminal);
                return ReviewOutcome::Cancelled;
            }
        },
        RemoveTarget::Secondary { target, main_repo } => {
            (None, Some(target.clone()), main_repo.clone())
        }
    };

    let mut data = gather_review_data(jj, dir, main_repo, workspace_name, herdr);
    let mut model = ReviewModel::new(build_review_rows(&data));

    let mut mode = DialogMode::Review;
    let mut error: Option<String> = None;
    let mut commit_message = LineEdit::new(String::new());

    let outcome = loop {
        let view_height = terminal
            .size()
            .map(|size| {
                dialog_view_height(Rect::new(0, 0, size.width, size.height), model.rows.len())
            })
            .unwrap_or(0);
        let _ = terminal.draw(|frame| {
            draw_review_dialog(
                frame,
                &model,
                mode,
                error.as_deref(),
                &commit_message.text,
                commit_message.cursor,
            )
        });
        match event::read() {
            Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Esc => {
                    if mode == DialogMode::Commit {
                        mode = DialogMode::Review;
                        commit_message.clear();
                        error = None;
                    } else {
                        break ReviewOutcome::Cancelled;
                    }
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    break ReviewOutcome::Cancelled;
                }
                KeyCode::Up | KeyCode::Char('k') if mode == DialogMode::Review => {
                    model.move_cursor(-1);
                    model.follow_cursor(view_height);
                    error = None;
                }
                KeyCode::Down | KeyCode::Char('j') if mode == DialogMode::Review => {
                    model.move_cursor(1);
                    model.follow_cursor(view_height);
                    error = None;
                }
                KeyCode::Char(' ') if mode == DialogMode::Review => {
                    model.toggle_current();
                    error = None;
                }
                KeyCode::Char('a') if mode == DialogMode::Review => {
                    model.toggle_all();
                    error = None;
                }
                KeyCode::Char('c') if mode == DialogMode::Review && data.can_commit() => {
                    mode = DialogMode::Commit;
                    commit_message.clear();
                    error = None;
                }
                KeyCode::Backspace if mode == DialogMode::Commit => {
                    commit_message.backspace();
                    error = None;
                }
                KeyCode::Char(c)
                    if mode == DialogMode::Commit
                        && !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    apply_line_key(&mut commit_message, EditKey::Char(c));
                    error = None;
                }
                KeyCode::Left | KeyCode::Right | KeyCode::Home | KeyCode::End | KeyCode::Delete
                    if mode == DialogMode::Commit
                        && !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    if let Some(nav) = line_nav_key(key.code) {
                        apply_line_key(&mut commit_message, nav);
                    }
                    error = None;
                }
                KeyCode::Enter => match mode {
                    DialogMode::Review => {
                        // Task 5.1: recheck the work copy immediately before
                        // authorizing (TOCTOU convergence). A fresh failure or
                        // new dirt refreshes the checks, returns to review and
                        // executes nothing.
                        if let Some(dir) = data.dir.clone() {
                            data.clean = Some(workspace_change_lines(jj, &dir));
                            model.replace_rows(build_review_rows(&data));
                            model.follow_cursor(view_height);
                        }
                        match review_blocking_reason(&data) {
                            Some(reason) => error = Some(reason),
                            None => {
                                break ReviewOutcome::Authorized(model.to_plan(
                                    data.dir.clone(),
                                    data.main_repo.clone(),
                                    data.workspace_name.clone(),
                                ))
                            }
                        }
                    }
                    DialogMode::Commit => {
                        if commit_message.text.trim().is_empty() {
                            mode = DialogMode::Review;
                            error = Some("commit message must not be empty".to_string());
                            continue;
                        }
                        let Some(dir) = data.dir.clone() else {
                            mode = DialogMode::Review;
                            error = Some("cannot commit: workspace path unknown".to_string());
                            continue;
                        };
                        match jj_commit(jj, &dir, &commit_message.text) {
                            Ok(()) => {
                                data.clean = Some(workspace_change_lines(jj, &dir));
                                model.replace_rows(build_review_rows(&data));
                                model.follow_cursor(view_height);
                                commit_message.clear();
                                mode = DialogMode::Review;
                                error = None;
                            }
                            Err(message) => {
                                mode = DialogMode::Review;
                                error = Some(message);
                            }
                        }
                    }
                },
                _ => {}
            },
            Ok(Event::Mouse(mouse)) => {
                // Wheel = content scroll only; cursor and selection stay put.
                let delta = match mouse.kind {
                    MouseEventKind::ScrollUp => -3isize,
                    MouseEventKind::ScrollDown => 3,
                    _ => 0,
                };
                if delta != 0 {
                    model.scroll_by(delta, view_height, model.rows.len());
                }
            }
            Ok(_) => {}
            Err(_) => {
                let _ = restore_terminal(&mut terminal);
                return ReviewOutcome::Cancelled;
            }
        }
    };

    let _ = restore_terminal(&mut terminal);
    outcome
}

fn status_row_text(item: &StatusItem) -> (String, RowTone) {
    let (glyph, tone) = match &item.state {
        StatusItemState::Pending => ("·", RowTone::Dim),
        StatusItemState::Running => ("◌", RowTone::Accent),
        StatusItemState::Done(_) => ("✓", RowTone::Ok),
        StatusItemState::Skipped(_) => ("·", RowTone::Dim),
        StatusItemState::Failed(_) => ("✗", RowTone::Error),
    };
    let message = match &item.state {
        StatusItemState::Pending => "",
        StatusItemState::Running => "running…",
        StatusItemState::Done(message)
        | StatusItemState::Skipped(message)
        | StatusItemState::Failed(message) => message,
    };
    let text = if message.is_empty() {
        format!("{glyph} {}", item.task.label())
    } else {
        format!("{glyph} {} — {message}", item.task.label())
    };
    (text, tone)
}

fn draw_status(frame: &mut Frame, state: &StatusState, waiting: bool) {
    let p = catppuccin();
    let area = frame.area();
    dim_background(frame, area);
    let desired = remove_modal_height(area, (state.items.len() as u16).saturating_add(10));
    let Some(inner) = render_modal_shell(frame, area, REMOVE_MODAL_WIDTH, desired, &p) else {
        return;
    };
    if inner.height < 8 {
        return;
    }

    let hint = if waiting {
        STATUS_FAILED_HINT
    } else {
        STATUS_WORKING_HINT
    };
    let mut y = inner.y;
    render_modal_header(
        frame,
        Rect::new(inner.x, y, inner.width, 1),
        "Remove jj workspace",
        &p,
    );
    y += 1;
    frame.render_widget(
        Paragraph::new(hint).style(Style::default().fg(p.overlay0)),
        Rect::new(inner.x, y, inner.width, 1),
    );
    y += 2;

    render_section_title(
        frame,
        Rect::new(inner.x, y, inner.width, 1),
        "Status",
        true,
        &p,
    );
    y += 1;
    for item in &state.items {
        let (text, tone) = status_row_text(item);
        let style = match tone {
            RowTone::Normal => Style::default().fg(p.text),
            RowTone::Dim => Style::default().fg(p.overlay0),
            RowTone::Accent => Style::default().fg(p.accent),
            RowTone::Error => Style::default().fg(p.red),
            RowTone::Ok => Style::default().fg(p.green),
            RowTone::Warning => Style::default().fg(p.yellow),
        };
        frame.render_widget(
            Paragraph::new(format!("   {text}")).style(style),
            Rect::new(inner.x, y, inner.width, 1),
        );
        y += 1;
        if y + 3 >= inner.y + inner.height {
            break;
        }
    }

    if waiting {
        if let Some((task, reason)) = state.first_failure() {
            y += 1;
            frame.render_widget(
                Paragraph::new(format!(
                    "failed: {} — {}",
                    task.label(),
                    truncate_title(reason, 90)
                ))
                .style(Style::default().fg(p.red)),
                Rect::new(inner.x, y, inner.width, 1),
            );
            y += 1;
            let log = state
                .error_log
                .clone()
                .unwrap_or_else(|| "(error.log unavailable)".to_string());
            frame.render_widget(
                Paragraph::new(format!("full log: {log}")).style(Style::default().fg(p.overlay0)),
                Rect::new(inner.x, y, inner.width, 1),
            );
        }
        let text = action_button_text(Some("↵"), "close");
        let width = text.chars().count() as u16;
        let x = inner.x + inner.width.saturating_sub(width) / 2;
        let rect = Rect::new(x, inner.y + inner.height.saturating_sub(1), width, 1);
        render_action_button(
            frame,
            rect,
            Some("↵"),
            "close",
            Style::default()
                .fg(panel_contrast_fg(&p))
                .bg(p.accent)
                .add_modifier(Modifier::BOLD),
        );
    }
}

/// Drives the Status TUI: call [`StatusView::open`] once, [`StatusView::update`]
/// after every task transition, then [`StatusView::finish`] with the overall
/// failure flag. On success `finish` restores the terminal immediately (the
/// process then exits and its overlay pane closes); on failure it waits for
/// ↵ / esc so the reason and the error.log pointer stay readable.
struct StatusView {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    /// Last drawn state; `finish` renders it once more before restoring.
    state: StatusState,
}

impl StatusView {
    fn open(state: &StatusState) -> io::Result<StatusView> {
        enable_raw_mode()?;
        let mut out = io::stdout();
        execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
        let mut terminal = match Terminal::new(CrosstermBackend::new(out)) {
            Ok(terminal) => terminal,
            Err(err) => {
                let _ = execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
                let _ = disable_raw_mode();
                return Err(err);
            }
        };
        terminal.draw(|frame| draw_status(frame, state, false))?;
        Ok(StatusView {
            terminal,
            state: state.clone(),
        })
    }

    fn update(&mut self, state: &StatusState) -> io::Result<()> {
        self.state = state.clone();
        self.terminal
            .draw(|frame| draw_status(frame, state, false))?;
        Ok(())
    }

    fn finish(&mut self, any_failure: bool) -> io::Result<()> {
        let state = self.state.clone();
        self.terminal
            .draw(|frame| draw_status(frame, &state, any_failure))?;
        if any_failure {
            loop {
                match event::read() {
                    Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => match key.code {
                        KeyCode::Enter | KeyCode::Esc => break,
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            break
                        }
                        _ => {}
                    },
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
        }
        restore_terminal(&mut self.terminal)
    }
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

/// Resolve the wizard's single source from the plugin's own injected context
/// (`HERDR_PLUGIN_CONTEXT_JSON`): `focused_pane_cwd` (the pane's shell cwd,
/// the same authority herdr uses for labels/follow-cwd — never
/// `foreground_cwd`) is walked up to its jj workspace root by `jj_root()`,
/// then a secondary workspace is resolved to the main repository root by
/// `repo_root()`. The caller's `workspace_id` (new-tab landing workspace)
/// rides along. Missing cwd, an empty workspace id, a nonexistent path, or a
/// non-jj directory are structured errors.
fn resolve_source_from_ctx(ctx: &str) -> Result<WorkspaceSource, String> {
    let workspace_id = json_string_field(ctx, "workspace_id").unwrap_or_default();
    if workspace_id.is_empty() {
        return Err(
            "no workspace id in plugin context (is there an active workspace?)".to_string()
        );
    }
    let cwd = json_string_field(ctx, "focused_pane_cwd")
        .filter(|cwd| !cwd.is_empty())
        .ok_or_else(|| {
            "no focused pane cwd in plugin context (is there an active workspace?)".to_string()
        })?;
    if !Path::new(&cwd).is_dir() {
        return Err(format!("focused pane cwd does not exist: {cwd}"));
    }
    let root = jj_root(&cwd)
        .ok_or_else(|| format!("{cwd} is not inside a jj workspace (no .jj marker found)"))?;
    Ok(WorkspaceSource {
        id: workspace_id,
        path: repo_root(&root),
    })
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
    append_error_log(&path, message).then(|| path.display().to_string())
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
    fn wizard_fields_cycle_through_the_two_editable_fields() {
        assert_eq!(next_wizard_field(WizardField::Name), WizardField::Base);
        assert_eq!(next_wizard_field(WizardField::Base), WizardField::Name);
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
        assert_eq!(
            config.agent.bootstrap_paths,
            AgentConfig::default().bootstrap_paths
        );
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
        assert!(
            message.contains("agent.command") && message.contains("empty"),
            "{message}"
        );
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
        assert_eq!(
            expand_tilde_with_home("/abs/path", Some("/home/nathan")),
            "/abs/path"
        );
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
        let resolved = resolve_jj_command(&JjCommandValue::Single(path.display().to_string()), &[])
            .expect("absolute path is validated and used as-is");
        assert_eq!(resolved.executable, path);
    }

    #[test]
    fn absolute_path_must_be_an_executable_file() {
        let dir = TempDir::new();
        let non_exec = make_file(dir.path(), "jj", false);
        let err = resolve_jj_command(&JjCommandValue::Single(non_exec.display().to_string()), &[])
            .expect_err("exists but is not executable");
        assert!(
            err.to_string().contains("not an executable file"),
            "{}",
            err
        );

        let missing = dir.path().join("nope");
        let err = resolve_jj_command(&JjCommandValue::Single(missing.display().to_string()), &[])
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
    /// its jj invocations are asserted against real execution. The script runs
    /// with cwd = `<root>/dest`, a secondary workspace whose `.jj/repo` pointer
    /// (`../../main/.jj/repo`) resolves to `<root>/main` (created so the
    /// pointer derivation does not degrade). Returns the canonicalized main
    /// root so callers can assert the resolve call's `-R` argument.
    #[cfg(unix)]
    fn run_setup_script(
        bookmark_name: &str,
        base_rev: &str,
        fail_on: &str,
        leading_args: &[&str],
        resolve_mode: &str,
        resolve_output: Option<&str>,
    ) -> (std::process::Output, Vec<String>, Vec<String>, String) {
        use std::os::unix::fs::PermissionsExt;

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock before epoch")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("jj-workspace-setup-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(root.join("scripts")).expect("create scripts dir");
        std::fs::create_dir_all(root.join("target/release")).expect("create target dir");
        std::fs::create_dir_all(root.join("bin")).expect("create bin dir");
        // The script's cwd is the fresh secondary workspace: `dest` carries a
        // `.jj/repo` relative pointer to the MAIN repo (`main`), which must
        // exist — the script's `cd` target cannot be missing or the pointer
        // derivation silently degrades.
        std::fs::create_dir_all(root.join("dest/.jj")).expect("create dest dir");
        std::fs::create_dir_all(root.join("main")).expect("create main dir");
        std::fs::write(root.join("dest/.jj/repo"), "../../main/.jj/repo\n")
            .expect("write repo pointer");
        // Canonicalized before the cleanup below: `/tmp` may itself be a
        // symlink (macOS), and the script normalizes with `pwd -P`, so the
        // expected `-R` argument is the physical path.
        let main_root = std::fs::canonicalize(root.join("main"))
            .expect("canonicalize main root")
            .display()
            .to_string();

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

        // Fake jj: logs argv, fails on a chosen subcommand, and answers the
        // `log` subcommand (the MAIN-context resolve call) per
        // JJ_FAKE_RESOLVE_MODE / JJ_FAKE_RESOLVE_OUTPUT. The subcommand is
        // matched anywhere in argv: the resolve call's first argument is the
        // global `-R`, so `$1` is not the subcommand on that call.
        let jj_log = root.join("jj.log");
        let jj = root.join("bin/jj");
        std::fs::write(
            &jj,
            format!(
                "#!/bin/sh\n\
                 printf '%s\\n' \"$*\" >> \"$JJ_FAKE_LOG\"\n\
                 for a in \"$@\"; do\n\
                   case \"$a\" in\n\
                     sparse|bookmark|fetch|rebase|log) SUB=\"$a\" ;;\n\
                   esac\n\
                 done\n\
                 case \"$SUB\" in\n\
                   {fail_on}) exit 1 ;;\n\
                   log)\n\
                     case \"$JJ_FAKE_RESOLVE_MODE\" in\n\
                       empty) exit 0 ;;\n\
                       fail) exit 1 ;;\n\
                       *) echo \"${{JJ_FAKE_RESOLVE_OUTPUT:-1111111111111111111111111111111111111111}}\" ;;\n\
                     esac\n\
                     exit 0 ;;\n\
                 esac\n\
                 exit 0\n"
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
        if let Some(output) = resolve_output {
            command.env("JJ_FAKE_RESOLVE_OUTPUT", output);
        }
        let output = command
            .current_dir(root.join("dest"))
            .env("JJ_FAKE_LOG", &jj_log)
            .env("JJ_FAKE_RESOLVE_MODE", resolve_mode)
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
        (output, jj_lines, finish_lines, main_root)
    }

    #[test]
    #[cfg(unix)]
    fn setup_runs_materialize_bookmark_fetch_rebase_in_order() {
        let (output, log, finish, main_root) =
            run_setup_script("plain-name", "trunk()", "__never__", &[], "ok", None);
        assert!(output.status.success());
        assert_eq!(
            log,
            vec![
                "sparse set --clear --add .".to_string(),
                "bookmark create plain-name -r @".to_string(),
                "git fetch".to_string(),
                format!(
                    "-R {main_root} --ignore-working-copy log -r trunk() --no-graph -T commit_id ++ \"\\n\""
                ),
                "rebase -s @ -d 1111111111111111111111111111111111111111".to_string(),
            ]
        );
        // The plugin exe was self-located via the script's `$0` layout.
        assert_eq!(finish, vec!["finish-tab w t p"]);
    }

    #[test]
    #[cfg(unix)]
    fn setup_derives_main_root_from_repo_pointer() {
        // The relative `.jj/repo` pointer (`../../main/.jj/repo` from
        // `dest/.jj`) must resolve to the canonical `<root>/main` path used
        // as the resolve call's `-R` argument — the same mechanism the
        // plugin's repo_root() relies on.
        let (output, log, _, main_root) =
            run_setup_script("plain-name", "trunk()", "__never__", &[], "ok", None);
        assert!(output.status.success());
        assert!(
            log.iter()
                .any(|line| line.starts_with(&format!("-R {main_root} "))),
            "resolve call must target the derived main root: {log:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn setup_materialization_failure_stops_the_chain() {
        let (output, log, _, _) =
            run_setup_script("workspace/x", "trunk()", "sparse", &[], "ok", None);
        assert!(!output.status.success());
        assert_eq!(log, vec!["sparse set --clear --add ."]);
    }

    #[test]
    #[cfg(unix)]
    fn setup_bookmark_failure_warns_and_keeps_updating() {
        let (output, log, _, main_root) =
            run_setup_script("workspace/fix-api", "trunk()", "bookmark", &[], "ok", None);
        // The `|| printf` fallback makes the bookmark step succeed, so
        // fetch/resolve/rebase still run and the whole script exits 0.
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
                "sparse set --clear --add .".to_string(),
                "bookmark create workspace/fix-api -r @".to_string(),
                "git fetch".to_string(),
                format!(
                    "-R {main_root} --ignore-working-copy log -r trunk() --no-graph -T commit_id ++ \"\\n\""
                ),
                "rebase -s @ -d 1111111111111111111111111111111111111111".to_string(),
            ]
        );
    }

    #[test]
    #[cfg(unix)]
    fn setup_preserves_single_quotes_in_bookmark_names() {
        let (output, log, _, main_root) =
            run_setup_script("it's-final", "trunk()", "__never__", &[], "ok", None);
        assert!(output.status.success());
        assert_eq!(
            log,
            vec![
                "sparse set --clear --add .".to_string(),
                "bookmark create it's-final -r @".to_string(),
                "git fetch".to_string(),
                format!(
                    "-R {main_root} --ignore-working-copy log -r trunk() --no-graph -T commit_id ++ \"\\n\""
                ),
                "rebase -s @ -d 1111111111111111111111111111111111111111".to_string(),
            ]
        );
    }

    #[test]
    #[cfg(unix)]
    fn setup_propagates_leading_args_before_each_subcommand() {
        // argv-form `jj.command` leading arguments ride as trailing script
        // arguments and must be inserted before every jj subcommand — the
        // resolve call gets them before its `-R` global option.
        let (output, log, _, main_root) =
            run_setup_script("plain-name", "trunk()", "__never__", &["--at-op", "@-"], "ok", None);
        assert!(output.status.success());
        assert_eq!(
            log,
            vec![
                "--at-op @- sparse set --clear --add .".to_string(),
                "--at-op @- bookmark create plain-name -r @".to_string(),
                "--at-op @- git fetch".to_string(),
                format!(
                    "--at-op @- -R {main_root} --ignore-working-copy log -r trunk() --no-graph -T commit_id ++ \"\\n\""
                ),
                "--at-op @- rebase -s @ -d 1111111111111111111111111111111111111111".to_string(),
            ]
        );
    }

    #[test]
    #[cfg(unix)]
    fn setup_passes_base_rev_to_resolution() {
        // The base-rev argument flows into the MAIN-context resolve call; the
        // rebase destination is the resolved commit id, not the raw revset.
        let (output, log, _, _) = run_setup_script("w", "dev@origin", "__never__", &[], "ok", None);
        assert!(output.status.success());
        assert!(
            log.iter().any(|line| line.contains("log -r dev@origin")),
            "resolve call must receive the full base revset: {log:?}"
        );
        assert!(
            log.iter()
                .any(|line| line == "rebase -s @ -d 1111111111111111111111111111111111111111"),
            "rebase must target the resolved commit id: {log:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn setup_passes_revsets_with_spaces_and_quotes_to_resolution() {
        // Revsets are shell-quoted per argument by the wizard, so spaces and
        // single quotes survive the pane shell and arrive as one argv word —
        // the MAIN-context resolve call must receive the complete revset.
        let (output, log, _, _) = run_setup_script(
            "w",
            "trunk() | remote_bookmark(dev)",
            "__never__",
            &[],
            "ok",
            None,
        );
        assert!(output.status.success());
        assert!(
            log.iter()
                .any(|line| line.contains("log -r trunk() | remote_bookmark(dev)")),
            "space-containing revset must reach the resolve call: {log:?}"
        );
        assert!(
            log.iter()
                .any(|line| line == "rebase -s @ -d 1111111111111111111111111111111111111111"),
            "rebase must target the resolved commit id: {log:?}"
        );

        let (output, log, _, _) =
            run_setup_script("w", "description(\"fix'it\")", "__never__", &[], "ok", None);
        assert!(output.status.success());
        assert!(
            log.iter()
                .any(|line| line.contains("log -r description(\"fix'it\")")),
            "quote-containing revset must reach the resolve call: {log:?}"
        );
        assert!(
            log.iter()
                .any(|line| line == "rebase -s @ -d 1111111111111111111111111111111111111111"),
            "rebase must target the resolved commit id: {log:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn setup_resolution_failure_warns_and_skips_rebase() {
        // A non-zero resolve call is a real reachable state (e.g. a remote
        // branch deleted after fetch): warn on stderr, skip rebase, exit 0.
        let (output, log, _, _) = run_setup_script("w", "dev@origin", "__never__", &[], "fail", None);
        assert!(output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("warning: could not resolve base revset 'dev@origin'"),
            "stderr should carry the resolve warning: {stderr}"
        );
        assert_eq!(log.len(), 4, "no rebase call after a failed resolve: {log:?}");
        assert!(!log.iter().any(|line| line.starts_with("rebase")), "{log:?}");
    }

    #[test]
    #[cfg(unix)]
    fn setup_empty_resolution_warns_and_skips_rebase() {
        // Exit 0 with empty stdout = the revset resolved to no commits: same
        // degradation path (warn, skip rebase, exit 0).
        let (output, log, _, _) = run_setup_script("w", "none()", "__never__", &[], "empty", None);
        assert!(output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("warning: could not resolve base revset 'none()'"),
            "stderr should carry the resolve warning: {stderr}"
        );
        assert_eq!(log.len(), 4, "no rebase call after an empty resolve: {log:?}");
        assert!(!log.iter().any(|line| line.starts_with("rebase")), "{log:?}");
    }

    #[test]
    #[cfg(unix)]
    fn setup_union_resolution_rebases_to_single_union_arg() {
        // Multiple commits resolve to one commit id per line (as real jj
        // renders with the `commit_id ++ "\n"` template); the script joins
        // them into a SINGLE ` | `-separated `-d` argument (merge-parents
        // semantics preserved).
        let (output, log, _, _) = run_setup_script(
            "w",
            "main@origin | dev@origin",
            "__never__",
            &[],
            "ok",
            Some(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\nbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            ),
        );
        assert!(output.status.success());
        assert!(
            log.iter().any(|line| {
                line == "rebase -s @ -d aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa | bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            }),
            "union must arrive as one -d argument: {log:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn setup_rebase_failure_still_fails() {
        // Rebase failing keeps the script's existing hard-fail semantics.
        let (output, log, _, _) = run_setup_script("w", "trunk()", "rebase", &[], "ok", None);
        assert!(!output.status.success());
        assert_eq!(log.len(), 5, "all five steps ran before the rebase failure: {log:?}");
        assert!(log.last().unwrap().starts_with("rebase"), "{log:?}");
    }

    // --- shared single-line cursor editing (wizard-field-cursor) ---------

    /// A `LineEdit` with an explicit cursor for boundary cases.
    fn line_edit(text: &str, cursor: usize) -> LineEdit {
        LineEdit {
            text: text.to_string(),
            cursor,
        }
    }

    #[test]
    fn line_edit_inserts_at_start_middle_and_end() {
        let mut start = line_edit("bc", 0);
        apply_line_key(&mut start, EditKey::Char('a'));
        assert_eq!((start.text.as_str(), start.cursor), ("abc", 1));

        let mut middle = line_edit("workspace/fx", 11);
        apply_line_key(&mut middle, EditKey::Char('i'));
        assert_eq!((middle.text.as_str(), middle.cursor), ("workspace/fix", 12));

        let mut end = LineEdit::new("ab".to_string());
        apply_line_key(&mut end, EditKey::Char('c'));
        assert_eq!((end.text.as_str(), end.cursor), ("abc", 3));
    }

    #[test]
    fn line_edit_backspace_deletes_before_the_cursor() {
        let mut empty = LineEdit::new(String::new());
        apply_line_key(&mut empty, EditKey::Backspace);
        assert_eq!((empty.text.as_str(), empty.cursor), ("", 0));

        let mut start = line_edit("ab", 0);
        apply_line_key(&mut start, EditKey::Backspace);
        assert_eq!((start.text.as_str(), start.cursor), ("ab", 0));

        let mut middle = line_edit("abc", 1);
        apply_line_key(&mut middle, EditKey::Backspace);
        assert_eq!((middle.text.as_str(), middle.cursor), ("bc", 0));

        let mut end = LineEdit::new("abc".to_string());
        apply_line_key(&mut end, EditKey::Backspace);
        assert_eq!((end.text.as_str(), end.cursor), ("ab", 2));
    }

    #[test]
    fn line_edit_delete_removes_at_the_cursor() {
        let mut empty = LineEdit::new(String::new());
        apply_line_key(&mut empty, EditKey::Delete);
        assert_eq!((empty.text.as_str(), empty.cursor), ("", 0));

        let mut end = LineEdit::new("ab".to_string());
        apply_line_key(&mut end, EditKey::Delete);
        assert_eq!((end.text.as_str(), end.cursor), ("ab", 2));

        let mut middle = line_edit("abc", 1);
        apply_line_key(&mut middle, EditKey::Delete);
        assert_eq!((middle.text.as_str(), middle.cursor), ("ac", 1));
    }

    #[test]
    fn line_edit_movement_clamps_at_the_boundaries() {
        let mut edit = line_edit("ab", 1);
        apply_line_key(&mut edit, EditKey::Left);
        assert_eq!(edit.cursor, 0);
        apply_line_key(&mut edit, EditKey::Left);
        assert_eq!(edit.cursor, 0, "left at the start stays put");
        apply_line_key(&mut edit, EditKey::End);
        assert_eq!(edit.cursor, 2);
        apply_line_key(&mut edit, EditKey::Right);
        assert_eq!(edit.cursor, 2, "right at the end stays put");
        apply_line_key(&mut edit, EditKey::Home);
        assert_eq!(edit.cursor, 0);
    }

    #[test]
    fn line_edit_multibyte_chars_never_split_utf8() {
        // CJK: 3 chars, 9 bytes. Char-index insert/delete must stay on
        // boundaries (byte-index editing would panic here).
        let mut edit = line_edit("工作区", 1);
        apply_line_key(&mut edit, EditKey::Char('x'));
        assert_eq!((edit.text.as_str(), edit.cursor), ("工x作区", 2));
        apply_line_key(&mut edit, EditKey::Backspace);
        assert_eq!((edit.text.as_str(), edit.cursor), ("工作区", 1));
        apply_line_key(&mut edit, EditKey::Delete);
        assert_eq!((edit.text.as_str(), edit.cursor), ("工区", 1));
        apply_line_key(&mut edit, EditKey::Home);
        apply_line_key(&mut edit, EditKey::Delete);
        assert_eq!((edit.text.as_str(), edit.cursor), ("区", 0));

        // An astral-plane emoji is one `char` too.
        let mut emoji = line_edit("a😀b", 1);
        apply_line_key(&mut emoji, EditKey::Delete);
        assert_eq!(emoji.text, "ab");
    }

    #[test]
    fn cursor_render_inserts_the_block_at_the_cursor_boundary() {
        assert_eq!(
            render_line_with_cursor("workspace/fix", 12, true),
            "workspace/fi█x"
        );
        assert_eq!(
            render_line_with_cursor("workspace/fix", 0, true),
            "█workspace/fix"
        );
        assert_eq!(
            render_line_with_cursor("workspace/fix", 13, true),
            "workspace/fix█"
        );
    }

    #[test]
    fn cursor_render_is_plain_when_unfocused() {
        let rendered = render_line_with_cursor("workspace/fix", 12, false);
        assert_eq!(rendered, "workspace/fix");
        assert!(!rendered.contains('█'), "unfocused fields render no block");
    }

    // --- name field component-level editing (workspace-wizard: name 字段组件级编辑)

    fn name_char(name: &str, state: NameEditState, c: char) -> (String, NameEditState) {
        let mut n = LineEdit::new(name.to_string());
        let mut s = state;
        apply_name_key(&mut n, &mut s, EditKey::Char(c));
        (n.text, s)
    }

    fn name_backspace(name: &str, state: NameEditState) -> (String, NameEditState) {
        let mut n = LineEdit::new(name.to_string());
        let mut s = state;
        apply_name_key(&mut n, &mut s, EditKey::Backspace);
        (n.text, s)
    }

    /// Applies one key to a name field with an explicit cursor; returns
    /// `(text, cursor, state)`.
    fn name_key(
        name: &str,
        cursor: usize,
        state: NameEditState,
        key: EditKey,
    ) -> (String, usize, NameEditState) {
        let mut n = line_edit(name, cursor);
        let mut s = state;
        apply_name_key(&mut n, &mut s, key);
        (n.text, n.cursor, s)
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
    fn name_navigation_exits_fresh_without_touching_the_default_name() {
        for key in [
            EditKey::Left,
            EditKey::Right,
            EditKey::Home,
            EditKey::End,
            EditKey::Delete,
        ] {
            let (n, _, s) = name_key("workspace/brave-river-0000", 26, NameEditState::Fresh, key);
            assert_eq!(n, "workspace/brave-river-0000", "{key:?} must not edit text");
            assert_eq!(s, NameEditState::Free, "{key:?} exits the Fresh anchor");
        }
    }

    #[test]
    fn name_navigation_then_backspace_deletes_one_char_only() {
        // The spec's safety case: leaving the Fresh anchor by navigation
        // disarms the whole-slug delete for good.
        let (n, c, s) =
            name_key("workspace/brave-river-0000", 26, NameEditState::Fresh, EditKey::Left);
        assert_eq!(
            (n.as_str(), c, s),
            ("workspace/brave-river-0000", 25, NameEditState::Free)
        );
        let (n, c, s) = name_key(&n, c, s, EditKey::Backspace);
        assert_eq!(
            (n.as_str(), c, s),
            ("workspace/brave-river-000", 24, NameEditState::Free)
        );
    }

    #[test]
    fn name_navigation_exits_prefixed_without_touching_the_prefix() {
        let (n, _, s) = name_key("workspace/", 10, NameEditState::Prefixed, EditKey::Delete);
        assert_eq!(n, "workspace/");
        assert_eq!(s, NameEditState::Free);
    }

    #[test]
    fn name_free_mid_cursor_inserts_at_the_cursor() {
        let (n, c, s) = name_key("workspace/fx", 11, NameEditState::Free, EditKey::Char('i'));
        assert_eq!((n.as_str(), c, s), ("workspace/fix", 12, NameEditState::Free));
    }

    #[test]
    fn name_free_delete_removes_the_char_at_the_cursor() {
        // Cursor before 'x' (index 12): Delete drops that 'x'.
        let (n, c, s) = name_key("workspace/fix", 12, NameEditState::Free, EditKey::Delete);
        assert_eq!((n.as_str(), c, s), ("workspace/fi", 12, NameEditState::Free));
    }

    #[test]
    fn name_free_home_then_insert_prepends() {
        let (n, c, s) = name_key("workspace/fix", 13, NameEditState::Free, EditKey::Home);
        assert_eq!((n.as_str(), c), ("workspace/fix", 0));
        let (n, c, s) = name_key(&n, c, s, EditKey::Char('a'));
        assert_eq!((n.as_str(), c, s), ("aworkspace/fix", 1, NameEditState::Free));
    }

    // --- base field cursor editing (workspace-wizard: base 字段光标编辑) ---

    #[test]
    fn base_first_keystroke_replaces_the_prefilled_value() {
        let mut base = LineEdit::new("trunk()".to_string());
        let mut replace = true;
        let dirty = apply_base_key(&mut base, &mut replace, EditKey::Char('d'));
        assert_eq!((base.text.as_str(), base.cursor), ("d", 1));
        assert!(dirty, "typing dirties the base field");
        assert!(!replace, "the replace anchor is consumed");
    }

    #[test]
    fn base_navigation_cancels_replace_and_keeps_the_text() {
        let mut base = LineEdit::new("trunk()".to_string());
        let mut replace = true;
        let dirty = apply_base_key(&mut base, &mut replace, EditKey::Left);
        assert_eq!(base.text, "trunk()", "navigation must not edit the text");
        assert_eq!(base.cursor, 6);
        assert!(!dirty, "navigation must not dirty the base field");
        assert!(!replace, "navigation cancels the replace anchor");

        // The next keystroke inserts at the cursor instead of clearing.
        let dirty = apply_base_key(&mut base, &mut replace, EditKey::Char('d'));
        assert_eq!((base.text.as_str(), base.cursor), ("trunk(d)", 7));
        assert!(dirty);
    }

    #[test]
    fn base_navigation_then_backspace_deletes_one_char() {
        // One Left parks the cursor before the final ')' — Backspace drops
        // only '(' instead of clearing the whole prefilled value.
        let mut base = LineEdit::new("trunk()".to_string());
        let mut replace = true;
        apply_base_key(&mut base, &mut replace, EditKey::Left);
        apply_base_key(&mut base, &mut replace, EditKey::Backspace);
        assert_eq!(base.text, "trunk)");
        assert!(!replace);
    }

    #[test]
    fn base_two_lefts_then_backspace_deletes_the_char_before_the_cursor() {
        // Two Lefts park the cursor before 'k' (tasks.md's `trun()`
        // illustration); the delete is exactly one char either way.
        let mut base = LineEdit::new("trunk()".to_string());
        let mut replace = true;
        apply_base_key(&mut base, &mut replace, EditKey::Left);
        apply_base_key(&mut base, &mut replace, EditKey::Left);
        assert_eq!(base.cursor, 5);
        apply_base_key(&mut base, &mut replace, EditKey::Backspace);
        assert_eq!(base.text, "trun()");
        assert!(!replace);
    }

    #[test]
    fn base_home_then_delete_removes_the_first_char() {
        let mut base = LineEdit::new("trunk()".to_string());
        let mut replace = true;
        apply_base_key(&mut base, &mut replace, EditKey::Home);
        apply_base_key(&mut base, &mut replace, EditKey::Delete);
        assert_eq!(base.text, "runk()");
        assert!(!replace);
    }

    #[test]
    #[cfg(unix)]
    fn base_navigation_only_still_resolves_via_the_chain() {
        // Navigation cancels replace-on-type without dirtying the field, so
        // submit re-evaluates the resolution chain.
        let mut base = LineEdit::new("trunk()".to_string());
        let mut replace = true;
        assert!(!apply_base_key(&mut base, &mut replace, EditKey::Left));
        assert!(!apply_base_key(&mut base, &mut replace, EditKey::Right));
        assert_eq!(base.text, "trunk()");

        let dir = TempDir::new();
        let jj = make_fake_jj(dir.path(), "jj", "echo repo-value\n");
        let config = Config::default();
        assert_eq!(
            wizard_final_base_rev(&config, &jj, dir.path(), &base.text, false)
                .expect("untouched field resolves lazily"),
            "repo-value"
        );
    }

    // --- remove dialog commit message cursor editing (workspace-removal-dialog)

    #[test]
    fn commit_message_inserts_at_the_cursor() {
        let mut message = line_edit("fx", 1);
        apply_line_key(&mut message, EditKey::Char('i'));
        assert_eq!((message.text.as_str(), message.cursor), ("fix", 2));
    }

    #[test]
    fn commit_message_delete_removes_the_char_at_the_cursor() {
        let mut message = line_edit("fixx", 3);
        apply_line_key(&mut message, EditKey::Delete);
        assert_eq!((message.text.as_str(), message.cursor), ("fix", 3));
    }

    #[test]
    fn commit_message_home_then_insert_prepends() {
        let mut message = LineEdit::new("fix bug".to_string());
        apply_line_key(&mut message, EditKey::Home);
        for c in "wip: ".chars() {
            apply_line_key(&mut message, EditKey::Char(c));
        }
        assert_eq!(
            (message.text.as_str(), message.cursor),
            ("wip: fix bug", 5)
        );
    }

    #[test]
    fn commit_substate_clears_the_message_and_parks_the_cursor_at_the_start() {
        // Entering the sub-state resets the line (the event loop calls
        // `clear()`), so typing starts from the first character.
        let mut message = LineEdit::new("stale".to_string());
        message.clear();
        assert_eq!((message.text.as_str(), message.cursor), ("", 0));
        apply_line_key(&mut message, EditKey::Char('f'));
        assert_eq!((message.text.as_str(), message.cursor), ("f", 1));
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

    #[test]
    fn resolve_source_from_ctx_resolves_main_repo_root() {
        let dir = TempDir::new();
        let repo = dir.path().join("main");
        fs::create_dir_all(repo.join(".jj")).expect("create .jj");
        let ctx = format!(r#"{{"workspace_id":"w1","focused_pane_cwd":"{}"}}"#, repo.display());
        let source = resolve_source_from_ctx(&ctx).expect("main repo resolves");
        assert_eq!(source.id, "w1");
        assert_eq!(source.path, repo.display().to_string());
    }

    #[test]
    fn resolve_source_from_ctx_resolves_secondary_workspace_to_main_root() {
        let dir = TempDir::new();
        let main = dir.path().join("main");
        fs::create_dir_all(main.join(".jj/repo")).expect("create main .jj store");
        let secondary = dir.path().join("secondary");
        fs::create_dir_all(secondary.join(".jj")).expect("create secondary .jj");
        fs::write(secondary.join(".jj/repo"), "../../main/.jj/repo\n").expect("write pointer");
        let ctx = format!(
            r#"{{"workspace_id":"w1","focused_pane_cwd":"{}"}}"#,
            secondary.display()
        );
        let source = resolve_source_from_ctx(&ctx).expect("secondary workspace resolves");
        assert_eq!(source.path, main.display().to_string());
    }

    #[test]
    fn resolve_source_from_ctx_walks_subdirectory_up_to_root() {
        let dir = TempDir::new();
        let repo = dir.path().join("repo");
        let nested = repo.join("a/b/c");
        fs::create_dir_all(&nested).expect("create nested dirs");
        fs::create_dir_all(repo.join(".jj")).expect("create .jj");
        let ctx = format!(
            r#"{{"workspace_id":"w1","focused_pane_cwd":"{}"}}"#,
            nested.display()
        );
        let source = resolve_source_from_ctx(&ctx).expect("subdirectory resolves");
        assert_eq!(source.path, repo.display().to_string());
    }

    #[test]
    fn resolve_source_from_ctx_non_jj_directory_is_an_error() {
        let dir = TempDir::new();
        let plain = dir.path().join("plain");
        fs::create_dir_all(&plain).expect("create plain dir");
        let ctx = format!(r#"{{"workspace_id":"w1","focused_pane_cwd":"{}"}}"#, plain.display());
        let err = resolve_source_from_ctx(&ctx).expect_err("non-jj must fail");
        assert!(err.contains("not inside a jj workspace"), "{err}");
    }

    #[test]
    fn resolve_source_from_ctx_nonexistent_cwd_is_an_error() {
        let dir = TempDir::new();
        let missing = dir.path().join("missing");
        let ctx = format!(r#"{{"workspace_id":"w1","focused_pane_cwd":"{}"}}"#, missing.display());
        let err = resolve_source_from_ctx(&ctx).expect_err("nonexistent cwd must fail");
        assert!(err.contains("does not exist"), "{err}");
    }

    #[test]
    fn resolve_source_from_ctx_missing_focused_pane_cwd_is_an_error() {
        let ctx = r#"{"workspace_id":"w1"}"#;
        let err = resolve_source_from_ctx(ctx).expect_err("missing cwd must fail");
        assert!(err.contains("no focused pane cwd"), "{err}");
    }

    #[test]
    fn resolve_source_from_ctx_empty_context_is_an_error() {
        let err = resolve_source_from_ctx("").expect_err("empty context must fail");
        assert!(err.contains("workspace id"), "{err}");
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
        // The ok fake jj must emit SOMETHING on the log call: exit 0 with
        // empty stdout now means "resolves to no commits" and is rejected.
        let ok = make_fake_jj(
            dir.path(),
            "ok-jj",
            "echo 1111111111111111111111111111111111111111\n",
        );
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
    fn wizard_final_base_rev_rejects_empty_resolution() {
        // Exit 0 with empty stdout = the revset resolved to no commits (e.g.
        // `none()`): must be rejected so `jj workspace add` never registers a
        // half-finished workspace.
        let dir = TempDir::new();
        let jj = make_fake_jj(dir.path(), "empty-jj", "exit 0\n");
        let config = Config::default();
        let err = wizard_final_base_rev(&config, &jj, dir.path(), "none()", true)
            .expect_err("empty resolution must fail");
        assert!(err.contains("base revset"), "{err}");
        assert!(err.contains("no commits"), "{err}");
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
        assert!(
            matches!(outcome, AgentWaitOutcome::NotDetected),
            "{outcome:?}"
        );
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
        let (outcome, keys) = run_wait(&["blocked claude"; 200], false, 10, 3, 10);
        assert!(
            matches!(outcome, AgentWaitOutcome::NeedsAttention { ref label } if label == "claude"),
            "{outcome:?}"
        );
        assert!(keys.is_empty(), "{keys:?}");
    }

    #[test]
    #[cfg(unix)]
    fn auto_trust_off_never_enters() {
        let (outcome, keys) = run_wait(&["blocked codex"; 200], false, 10, 3, 10);
        assert!(
            matches!(outcome, AgentWaitOutcome::NeedsAttention { ref label } if label == "codex"),
            "{outcome:?}"
        );
        assert!(keys.is_empty(), "{keys:?}");
    }

    #[test]
    #[cfg(unix)]
    fn auto_trust_non_codex_never_enters() {
        let (outcome, keys) = run_wait(&["blocked claude"; 200], true, 10, 3, 10);
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
        let (outcome, keys) = run_wait(&["blocked codex"; 200], true, 10, 3, 10);
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
        assert!(
            err.to_string().contains("agent.startup_timeout_secs"),
            "{err}"
        );
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
        let changes = workspace_change_lines(&jj, dir.path()).expect("dirty changes");
        assert_eq!(changes, vec!["A new-file.txt"]);

        // The review screen renders the block reason and both exit paths.
        let mut data = known_review_data(dir.path().to_path_buf());
        data.clean = Some(Ok(changes));
        let rendered: String = build_review_rows(&data)
            .iter()
            .map(|row| row.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("uncommitted change(s)"), "{rendered}");
        assert!(rendered.contains("jj commit"), "{rendered}");
        assert!(rendered.contains("jj restore"), "{rendered}");
        assert!(rendered.contains("bookmarks stay"), "{rendered}");
        let blocked = review_blocking_reason(&data).expect("dirty blocks authorization");
        assert!(blocked.contains("uncommitted change(s)"), "{blocked}");
    }

    #[test]
    #[cfg(unix)]
    fn remove_clean_workspace_is_allowed() {
        let dir = TempDir::new();
        let jj = make_fake_jj(dir.path(), "jj", "");
        assert!(
            workspace_change_lines(&jj, dir.path())
                .expect("clean check")
                .is_empty(),
            "clean workspace must pass the check"
        );
    }

    #[test]
    #[cfg(unix)]
    fn remove_check_failure_is_fail_closed() {
        let dir = TempDir::new();
        let jj = make_fake_jj(dir.path(), "jj", "exit 3\n");
        let err = workspace_change_lines(&jj, dir.path())
            .expect_err("a failed check must refuse, not pass");
        assert!(err.contains("cannot check"), "{err}");
        assert!(err.contains("refusing to remove"), "{err}");
    }

    fn remove_plan(dir: Option<PathBuf>, name: Option<&str>, main_repo: PathBuf) -> RemovePlan {
        RemovePlan {
            dir,
            main_repo,
            workspace_name: name.map(str::to_string),
            session_ids: Vec::new(),
            pane_ids: Vec::new(),
        }
    }

    #[test]
    #[cfg(unix)]
    fn resolve_forget_name_prefers_the_plan_name() {
        let dir = TempDir::new();
        // A failing jj proves the picker-supplied name needs no lookup.
        let jj = make_fake_jj(dir.path(), "jj", "exit 1\n");
        let plan = remove_plan(
            Some(dir.path().to_path_buf()),
            Some("from-picker"),
            dir.path().to_path_buf(),
        );
        assert_eq!(
            resolve_forget_name(&jj, &plan).as_deref(),
            Some("from-picker")
        );
    }

    #[test]
    #[cfg(unix)]
    fn resolve_forget_name_matches_root_then_falls_back_to_basename() {
        let dir = TempDir::new();
        let main = dir.path().join("main");
        std::fs::create_dir_all(&main).expect("create main repo dir");
        let ws = dir.path().join("workspace/checkout");
        std::fs::create_dir_all(&ws).expect("create workspace dir");

        // `jj workspace list` output: an unrelated entry plus the one whose
        // canonical root matches the target.
        let jj = make_fake_jj(
            dir.path(),
            "jj",
            r#"D=$(dirname "$0")
printf 'other\t%s\nfound\t%s\n' "$D/unrelated" "$D/workspace/checkout"
"#,
        );
        let plan = remove_plan(Some(ws.clone()), None, main.clone());
        assert_eq!(resolve_forget_name(&jj, &plan).as_deref(), Some("found"));

        // No matching entry → directory basename.
        let empty = make_fake_jj(dir.path(), "empty-jj", "exit 0\n");
        assert_eq!(
            resolve_forget_name(&empty, &plan).as_deref(),
            Some("checkout")
        );

        // Unknown path and no picker name → nothing to forget.
        let unknown = remove_plan(None, None, main);
        assert_eq!(resolve_forget_name(&jj, &unknown), None);
    }

    #[test]
    fn close_summary_reports_warnings_without_failing() {
        assert_eq!(close_summary(3, 0), "3 pane(s) closed");
        assert_eq!(close_summary(3, 1), "3 closed, 1 failed (warning)");
        assert_eq!(close_summary(0, 2), "0 closed, 2 failed (warning)");
    }

    // --- remove dialog data sources -------------------------------------

    /// Writes a fake herdr whose `body` answers the dialog's read commands,
    /// returning its executable path. Separate from `make_fake_herdr` (which
    /// serves the agent-readiness schedule) so neither fixture grows cases
    /// the other does not need.
    #[cfg(unix)]
    fn make_fake_herdr_dialog(dir: &Path, body: &str) -> String {
        use std::os::unix::fs::PermissionsExt;

        let bin = dir.join("herdr-dialog");
        std::fs::write(&bin, format!("#!/bin/sh\n{body}")).expect("write fake herdr");
        std::fs::File::open(&bin)
            .and_then(|file| file.sync_all())
            .expect("sync fake herdr");
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755))
            .expect("make fake herdr executable");
        bin.display().to_string()
    }

    fn pane_info(pane_id: &str, cwd: Option<&str>, foreground_cwd: Option<&str>) -> PaneInfo {
        PaneInfo {
            pane_id: pane_id.to_string(),
            tab_id: "t1".to_string(),
            workspace_id: "w1".to_string(),
            label: Some("shell".to_string()),
            agent: None,
            agent_status: None,
            cwd: cwd.map(str::to_string),
            foreground_cwd: foreground_cwd.map(str::to_string),
        }
    }

    /// Creates `main/.jj/repo` (directory) and `ws/.jj/repo` (file pointer to
    /// main), returning the canonical `(root, main, ws)` paths.
    #[cfg(unix)]
    fn make_repo_layout(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let main = dir.join("main");
        std::fs::create_dir_all(main.join(".jj/repo")).expect("create main repo");
        let ws = dir.join("ws");
        std::fs::create_dir_all(ws.join(".jj")).expect("create secondary workspace");
        std::fs::write(ws.join(".jj/repo"), "../../main/.jj/repo").expect("write repo pointer");
        (
            fs::canonicalize(dir).expect("canonical root"),
            fs::canonicalize(&main).expect("canonical main"),
            fs::canonicalize(&ws).expect("canonical workspace"),
        )
    }

    #[test]
    fn parse_workspace_list_marks_stale_rows_and_sorts_by_name() {
        let entries = parse_workspace_list(
            "stale\t\n\
             main\t/repos/main\n\
             malformed line without a tab\n\
             \t/repos/no-name\n\
             ws-b\t/repos/ws-b\n\
             ws-a\t/repos/ws-a\n",
        );
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, vec!["main", "stale", "ws-a", "ws-b"], "{entries:?}");
        assert_eq!(entries[0].root.as_deref(), Some(Path::new("/repos/main")));
        assert_eq!(entries[1].root, None, "empty root means stale on disk");
    }

    #[test]
    #[cfg(unix)]
    fn list_secondary_workspaces_excludes_the_main_workspace() {
        let dir = TempDir::new();
        let main = dir.path().join("main");
        std::fs::create_dir_all(main.join(".jj/repo")).expect("create main repo");
        let other = dir.path().join("other");
        std::fs::create_dir_all(&other).expect("create secondary dir");
        let jj = make_fake_jj(
            dir.path(),
            "jj",
            r#"D=$(dirname "$0")
printf '%s\n' "$@" > "$D/argv"
printf 'main\t%s\nstale\t\nother\t%s\n' "$D/main" "$D/other"
"#,
        );

        let entries = list_secondary_workspaces(&jj, &main).expect("list workspaces");
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, vec!["other", "stale"], "{entries:?}");
        assert_eq!(entries[0].root.as_deref(), Some(other.as_path()));
        assert_eq!(entries[1].root, None);

        // The invocation is argv-only: -R <main> --ignore-working-copy, and
        // the template is one argv item.
        let argv = fs::read_to_string(dir.path().join("argv")).expect("read argv");
        let argv: Vec<&str> = argv.lines().collect();
        assert_eq!(argv[0], "-R", "{argv:?}");
        assert_eq!(argv[1], main.display().to_string(), "{argv:?}");
        assert_eq!(argv[2], "--ignore-working-copy", "{argv:?}");
        assert_eq!(&argv[3..6], ["workspace", "list", "-T"], "{argv:?}");
        assert!(
            argv[6].contains("name ++ \"\\t\" ++ root ++ \"\\n\""),
            "{argv:?}"
        );
        assert_eq!(argv.len(), 7, "{argv:?}");
    }

    #[test]
    #[cfg(unix)]
    fn list_secondary_workspaces_reports_the_first_stderr_line() {
        let dir = TempDir::new();
        let jj = make_fake_jj(
            dir.path(),
            "jj",
            "echo 'no jj repo here' >&2\necho 'second line' >&2\nexit 2\n",
        );
        let err = list_secondary_workspaces(&jj, dir.path()).expect_err("must fail");
        assert!(err.contains("no jj repo here"), "{err}");
        assert!(!err.contains("second line"), "{err}");
    }

    #[test]
    #[cfg(unix)]
    fn resolve_remove_target_walks_up_from_a_subdirectory() {
        let dir = TempDir::new();
        let (_root, main, ws) = make_repo_layout(dir.path());
        let sub = ws.join("src/deep");
        std::fs::create_dir_all(&sub).expect("create subdir");
        let ctx = format!(r#"{{"focused_pane_cwd":"{}"}}"#, sub.display());
        match resolve_remove_target(&ctx).expect("secondary target") {
            RemoveTarget::Secondary { target, main_repo } => {
                assert_eq!(target, ws);
                assert_eq!(main_repo, main);
            }
            other => panic!("expected Secondary, got {other:?}"),
        }
    }

    #[test]
    #[cfg(unix)]
    fn resolve_remove_target_falls_back_to_workspace_cwd() {
        let dir = TempDir::new();
        let (_root, _main, ws) = make_repo_layout(dir.path());
        let ctx = format!(r#"{{"workspace_cwd":"{}"}}"#, ws.display());
        match resolve_remove_target(&ctx).expect("secondary target") {
            RemoveTarget::Secondary { target, .. } => assert_eq!(target, ws),
            other => panic!("expected Secondary, got {other:?}"),
        }
    }

    #[test]
    #[cfg(unix)]
    fn resolve_remove_target_detects_the_main_workspace() {
        let dir = TempDir::new();
        let (_root, main, _ws) = make_repo_layout(dir.path());
        let sub = main.join("src");
        std::fs::create_dir_all(&sub).expect("create subdir");
        let ctx = format!(r#"{{"focused_pane_cwd":"{}"}}"#, sub.display());
        match resolve_remove_target(&ctx).expect("main target") {
            RemoveTarget::Main { main_root } => assert_eq!(main_root, main),
            other => panic!("expected Main, got {other:?}"),
        }
    }

    #[test]
    fn resolve_remove_target_refuses_unsafe_paths() {
        assert!(unsafe_remove_path(Path::new("/")));
        assert!(!unsafe_remove_path(Path::new("/tmp/ws")));

        let err = resolve_remove_target(r#"{"focused_pane_cwd":"/"}"#)
            .expect_err("the filesystem root is never a removal target");
        assert!(err.contains("unsafe path"), "{err}");
    }

    #[test]
    #[cfg(unix)]
    fn resolve_remove_target_fails_closed_without_a_repo_pointer() {
        // `.jj` exists but `.jj/repo` does not: a secondary workspace whose
        // pointer cannot be read must not be reported as removable.
        let dir = TempDir::new();
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(ws.join(".jj")).expect("create .jj");
        let ctx = format!(r#"{{"focused_pane_cwd":"{}"}}"#, ws.display());
        let err = resolve_remove_target(&ctx).expect_err("missing pointer must fail closed");
        assert!(err.contains("cannot resolve the main repo"), "{err}");
        assert!(err.contains("refusing to remove"), "{err}");

        // A self-referencing pointer resolves back to the workspace itself.
        std::fs::write(ws.join(".jj/repo"), "repo").expect("write self pointer");
        let err = resolve_remove_target(&ctx).expect_err("self pointer must fail closed");
        assert!(err.contains("cannot resolve the main repo"), "{err}");
    }

    #[test]
    #[cfg(unix)]
    fn resolve_remove_target_rejects_missing_cwd_and_non_jj_dirs() {
        let err = resolve_remove_target("{}").expect_err("missing cwd");
        assert!(err.contains("no focused pane cwd"), "{err}");

        let dir = TempDir::new();
        let ctx = format!(r#"{{"focused_pane_cwd":"{}"}}"#, dir.path().display());
        let err = resolve_remove_target(&ctx).expect_err("no .jj marker");
        assert!(err.contains("not inside a jj workspace"), "{err}");

        let ctx = format!(
            r#"{{"focused_pane_cwd":"{}"}}"#,
            dir.path().join("gone").display()
        );
        let err = resolve_remove_target(&ctx).expect_err("nonexistent cwd");
        assert!(err.contains("cannot resolve focused pane cwd"), "{err}");
    }

    #[test]
    fn path_within_requires_a_component_boundary() {
        let base = Path::new("/a/b");
        assert!(path_within(base, "/a/b"));
        assert!(path_within(base, "/a/b/"));
        assert!(path_within(base, "/a/b/c"));
        assert!(!path_within(base, "/a/bc"));
        assert!(!path_within(base, "/a"));
        assert!(!path_within(base, ""));
    }

    #[test]
    #[cfg(unix)]
    fn path_within_follows_symlinks_for_existing_dirs() {
        let dir = TempDir::new();
        let target = dir.path().join("target");
        std::fs::create_dir_all(target.join("sub")).expect("create target");
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).expect("create symlink");

        assert!(path_within(&target, &link.display().to_string()));
        assert!(path_within(
            &target,
            &link.join("sub").display().to_string()
        ));
        assert!(!path_within(
            &target,
            &dir.path().join("other").display().to_string()
        ));
    }

    #[test]
    fn pane_matches_target_prefers_cwd_and_accepts_foreground_only() {
        let target = Path::new("/a/b");
        assert_eq!(
            pane_matches_target(target, &pane_info("p1", Some("/a/b/sub"), None)),
            Some(PaneMatch::Cwd)
        );
        assert_eq!(
            pane_matches_target(target, &pane_info("p2", Some("/elsewhere"), None)),
            None
        );
        assert_eq!(
            pane_matches_target(
                target,
                &pane_info("p3", Some("/elsewhere"), Some("/a/b/sub"))
            ),
            Some(PaneMatch::ForegroundOnly)
        );
        assert_eq!(
            pane_matches_target(target, &pane_info("p4", Some("/a/bc"), None)),
            None
        );
        // cwd wins when both cwds are inside.
        assert_eq!(
            pane_matches_target(target, &pane_info("p5", Some("/a/b/x"), Some("/a/b/y"))),
            Some(PaneMatch::Cwd)
        );
    }

    #[test]
    #[cfg(unix)]
    fn is_plugin_own_pane_requires_root_cwd_and_a_plugin_title() {
        let dir = TempDir::new();
        let root = fs::canonicalize(dir.path()).expect("canonical plugin root");
        let cwd = root.display().to_string();
        let mut pane = pane_info("p1", Some(&cwd), None);

        pane.label = Some("Remove jj workspace".into());
        assert!(is_plugin_own_pane(&pane, &root));
        pane.label = Some("New jj workspace".into());
        assert!(is_plugin_own_pane(&pane, &root));
        pane.label = Some("shell".into());
        assert!(!is_plugin_own_pane(&pane, &root));
        pane.label = Some("Remove jj workspace".into());
        pane.cwd = Some("/elsewhere".into());
        assert!(!is_plugin_own_pane(&pane, &root));
        pane.cwd = None;
        assert!(!is_plugin_own_pane(&pane, &root));
    }

    #[test]
    fn parse_panes_tolerates_missing_fields_and_skips_entries_without_ids() {
        let json = serde_json::json!({
            "result": {
                "panes": [
                    {
                        "pane_id": "p1",
                        "tab_id": "t1",
                        "workspace_id": "w1",
                        "label": "shell",
                        "agent": "codex",
                        "agent_status": "working",
                        "cwd": "/a",
                        "foreground_cwd": "/a/sub"
                    },
                    {"tab_id": "t2"},
                    {"pane_id": ""},
                    {"pane_id": "p2", "cwd": "/b"},
                    "not an object"
                ]
            }
        });
        let panes = parse_panes(&json);
        assert_eq!(panes.len(), 2, "{panes:?}");
        assert_eq!(panes[0].pane_id, "p1");
        assert_eq!(panes[0].foreground_cwd.as_deref(), Some("/a/sub"));
        assert_eq!(panes[1].pane_id, "p2");
        assert_eq!(panes[1].tab_id, "");
        assert_eq!(panes[1].workspace_id, "");
        assert_eq!(panes[1].label, None);

        assert!(parse_panes(&serde_json::json!({"result": {}})).is_empty());
        assert!(parse_panes(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn relative_to_target_reports_dot_and_nested_suffixes() {
        let dir = TempDir::new();
        let root = fs::canonicalize(dir.path()).expect("canonical root");
        std::fs::create_dir_all(root.join("src/deep")).expect("create subdir");

        assert_eq!(relative_to_target(&root, &root.display().to_string()), ".");
        assert_eq!(
            relative_to_target(&root, &root.join("src/deep").display().to_string()),
            "src/deep"
        );
        // Deleted / never-created subpaths still render lexically.
        assert_eq!(
            relative_to_target(&root, &root.join("gone").display().to_string()),
            "gone"
        );
        let outside = TempDir::new();
        let outside_path = outside.path().join("other");
        assert_eq!(
            relative_to_target(&root, &outside_path.display().to_string()),
            outside_path.display().to_string()
        );
    }

    #[test]
    fn format_age_ms_buckets_ages_and_falls_back_to_a_date() {
        // 2026-09-17 00:00:00 UTC (a week after the known 2026-09-09 epoch).
        let now = 1_788_912_000_000 + 8 * 86_400_000;
        assert_eq!(format_age_ms(now, now), "just now");
        assert_eq!(format_age_ms(now - 30_000, now), "just now");
        assert_eq!(format_age_ms(now - 5 * 60_000, now), "5m ago");
        assert_eq!(format_age_ms(now - 3 * 3_600_000, now), "3h ago");
        assert_eq!(format_age_ms(now - 6 * 86_400_000, now), "6d ago");
        assert_eq!(format_age_ms(now - 8 * 86_400_000, now), "2026-09-09");
        // Future timestamps clamp instead of rendering negative ages.
        assert_eq!(format_age_ms(now + 60_000, now), "just now");
    }

    #[test]
    #[cfg(unix)]
    fn workspace_change_lines_reports_changes_and_is_fail_closed() {
        let dir = TempDir::new();
        let clean = make_fake_jj(dir.path(), "clean-jj", "");
        assert!(workspace_change_lines(&clean, dir.path())
            .expect("clean check")
            .is_empty());

        let dirty = make_fake_jj(
            dir.path(),
            "dirty-jj",
            "echo 'A new-file.txt'\necho '   '\necho 'M modified.txt'\n",
        );
        let lines = workspace_change_lines(&dirty, dir.path()).expect("dirty lines");
        assert_eq!(lines, vec!["A new-file.txt", "M modified.txt"]);

        let failed = make_fake_jj(dir.path(), "failed-jj", "echo 'broken store' >&2\nexit 3\n");
        let err = workspace_change_lines(&failed, dir.path()).expect_err("must refuse");
        assert!(err.contains("cannot check"), "{err}");
        assert!(err.contains("refusing to remove"), "{err}");

        let missing = ResolvedJj {
            executable: dir.path().join("does-not-exist"),
            extra_args: Vec::new(),
        };
        let err = workspace_change_lines(&missing, dir.path()).expect_err("spawn failure");
        assert!(err.contains("cannot check"), "{err}");
        assert!(err.contains("refusing to remove"), "{err}");
    }

    #[test]
    #[cfg(unix)]
    fn scan_pane_candidates_filters_and_excludes_own_overlay_panes() {
        let dir = TempDir::new();
        let target_dir = dir.path().join("target");
        std::fs::create_dir_all(&target_dir).expect("create target");
        let target = fs::canonicalize(&target_dir).expect("canonical target");
        let plugin_dir = dir.path().join("plugin");
        std::fs::create_dir_all(&plugin_dir).expect("create plugin root");
        let plugin = fs::canonicalize(&plugin_dir).expect("canonical plugin root");
        let sibling = format!("{}bc", target.display());
        let target_s = target.display().to_string();
        let plugin_s = plugin.display().to_string();
        let body = format!(
            r#"case "$1 $2" in
  "pane list")
    printf '%s' '{{"result":{{"panes":[
      {{"pane_id":"inside","tab_id":"t1","workspace_id":"w1","label":"shell","cwd":"{target_s}"}},
      {{"pane_id":"fg","tab_id":"t1","workspace_id":"w1","label":"shell","cwd":"/elsewhere","foreground_cwd":"{target_s}/src"}},
      {{"pane_id":"sibling","tab_id":"t2","workspace_id":"w2","label":"shell","cwd":"{sibling}"}},
      {{"pane_id":"own","tab_id":"t3","workspace_id":"w3","label":"Remove jj workspace","cwd":"{plugin_s}"}},
      {{"tab_id":"t4"}}
    ]}}}}'
    ;;
esac
"#
        );
        let herdr = make_fake_herdr_dialog(dir.path(), &body);
        let candidates = scan_pane_candidates(&herdr, &target, &plugin);
        let ids: Vec<&str> = candidates
            .iter()
            .map(|candidate| candidate.info.pane_id.as_str())
            .collect();
        assert_eq!(ids, vec!["inside", "fg"], "{candidates:?}");
        assert_eq!(candidates[0].matched_via, PaneMatch::Cwd);
        assert_eq!(candidates[1].matched_via, PaneMatch::ForegroundOnly);
    }

    #[test]
    #[cfg(unix)]
    fn scan_pane_candidates_degrades_to_empty_when_pane_list_fails() {
        let dir = TempDir::new();
        let herdr = make_fake_herdr_dialog(dir.path(), "echo 'server down' >&2\nexit 1\n");
        let candidates = scan_pane_candidates(&herdr, dir.path(), dir.path());
        assert!(candidates.is_empty());
    }

    #[test]
    #[cfg(unix)]
    fn pane_group_labels_maps_ids_to_labels_and_degrades_to_empty() {
        let dir = TempDir::new();
        let herdr = make_fake_herdr_dialog(
            dir.path(),
            r#"case "$1 $2" in
  "workspace list")
    echo '{"result":{"workspaces":[{"workspace_id":"w1","label":"alpha"},{"workspace_id":"w2"}]}}'
    ;;
  "tab list")
    echo '{"result":{"tabs":[{"tab_id":"t1","label":"main"}]}}'
    ;;
esac
"#,
        );
        let (workspaces, tabs) = pane_group_labels(&herdr);
        assert_eq!(workspaces.get("w1").map(String::as_str), Some("alpha"));
        assert!(!workspaces.contains_key("w2"), "{workspaces:?}");
        assert_eq!(tabs.get("t1").map(String::as_str), Some("main"));

        // One command failing empties only its map; the other survives.
        let dir2 = TempDir::new();
        let herdr2 = make_fake_herdr_dialog(
            dir2.path(),
            r#"case "$1 $2" in
  "workspace list") exit 1 ;;
  "tab list") echo '{"result":{"tabs":[{"tab_id":"t1","label":"main"}]}}' ;;
esac
"#,
        );
        let (workspaces, tabs) = pane_group_labels(&herdr2);
        assert!(workspaces.is_empty());
        assert_eq!(tabs.get("t1").map(String::as_str), Some("main"));
    }

    #[test]
    #[cfg(unix)]
    fn jj_commit_runs_in_the_workspace_and_reports_stderr() {
        let dir = TempDir::new();
        let workspace = dir.path().join("ws");
        std::fs::create_dir_all(&workspace).expect("create workspace dir");
        let jj = make_fake_jj(
            dir.path(),
            "jj",
            r#"D=$(dirname "$0")
printf '%s\n' "$@" > "$D/argv"
pwd > "$D/cwd"
"#,
        );
        jj_commit(&jj, &workspace, "fix the thing").expect("commit succeeds");
        let argv = fs::read_to_string(dir.path().join("argv")).expect("read argv");
        assert_eq!(
            argv.lines().collect::<Vec<_>>(),
            vec!["commit", "-m", "fix the thing"]
        );
        let cwd = fs::read_to_string(dir.path().join("cwd")).expect("read cwd");
        let expected = fs::canonicalize(&workspace).expect("canonical workspace");
        assert_eq!(cwd.trim(), expected.display().to_string());

        // argv-form jj.command leading args ride before the subcommand.
        let mut extra = make_fake_jj(
            dir.path(),
            "extra-jj",
            "D=$(dirname \"$0\")\nprintf '%s\\n' \"$@\" > \"$D/argv-extra\"\n",
        );
        extra.extra_args = vec!["--at-op".into(), "@-".into()];
        jj_commit(&extra, &workspace, "m").expect("commit with extra args");
        let argv = fs::read_to_string(dir.path().join("argv-extra")).expect("read extra argv");
        assert_eq!(
            argv.lines().collect::<Vec<_>>(),
            vec!["--at-op", "@-", "commit", "-m", "m"]
        );

        let failing = make_fake_jj(
            dir.path(),
            "fail-jj",
            "echo 'error: conflicted commit' >&2\nexit 1\n",
        );
        let err = jj_commit(&failing, &workspace, "m").expect_err("must fail");
        assert!(err.contains("jj commit failed"), "{err}");
        assert!(err.contains("conflicted commit"), "{err}");
    }

    #[test]
    #[cfg(unix)]
    fn jj_forget_workspace_targets_the_main_repo_by_argv() {
        let dir = TempDir::new();
        let main = dir.path().join("main");
        std::fs::create_dir_all(&main).expect("create main dir");
        let jj = make_fake_jj(
            dir.path(),
            "jj",
            "D=$(dirname \"$0\")\nprintf '%s\\n' \"$@\" > \"$D/argv\"\n",
        );
        jj_forget_workspace(&jj, &main, "ws-a").expect("forget works");
        let argv = fs::read_to_string(dir.path().join("argv")).expect("read argv");
        let argv: Vec<&str> = argv.lines().collect();
        assert_eq!(argv[0], "-R", "{argv:?}");
        assert_eq!(argv[1], main.display().to_string(), "{argv:?}");
        assert_eq!(&argv[2..], ["workspace", "forget", "ws-a"], "{argv:?}");

        let failing = make_fake_jj(
            dir.path(),
            "fail-jj",
            "echo 'no such workspace' >&2\nexit 1\n",
        );
        let err = jj_forget_workspace(&failing, &main, "ws-a").expect_err("must fail");
        assert!(err.contains("jj workspace forget failed"), "{err}");
        assert!(err.contains("no such workspace"), "{err}");
    }

    #[test]
    #[cfg(unix)]
    fn close_herdr_pane_passes_the_pane_id_and_reports_stderr() {
        let dir = TempDir::new();
        let herdr = make_fake_herdr_dialog(
            dir.path(),
            "D=$(dirname \"$0\")\nprintf '%s\\n' \"$@\" > \"$D/argv\"\n",
        );
        close_herdr_pane(&herdr, "pane-1").expect("close succeeds");
        let argv = fs::read_to_string(dir.path().join("argv")).expect("read argv");
        assert_eq!(
            argv.lines().collect::<Vec<_>>(),
            vec!["pane", "close", "pane-1"]
        );

        let dir2 = TempDir::new();
        let failing = make_fake_herdr_dialog(dir2.path(), "echo 'pane not found' >&2\nexit 1\n");
        let err = close_herdr_pane(&failing, "pane-1").expect_err("must fail");
        assert!(err.contains("herdr pane close failed"), "{err}");
        assert!(err.contains("pane not found"), "{err}");
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
        assert!(
            lines[0].starts_with('[') && lines[0].contains("UTC"),
            "{content}"
        );
        assert!(lines[0].ends_with("first failure"), "{content}");
        assert!(
            lines[1].starts_with('[') && lines[1].contains("UTC"),
            "{content}"
        );
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
        assert_eq!(
            format_unix_timestamp(1_788_912_000),
            "2026-09-09 00:00:00 UTC"
        );
        // Leap-year day: 2024-02-29 12:34:56 UTC = 1709210096.
        assert_eq!(
            format_unix_timestamp(1_709_210_096),
            "2024-02-29 12:34:56 UTC"
        );
    }

    // --- remove dialog selection model ----------------------------------

    fn sample_review_rows() -> Vec<ReviewRow> {
        vec![
            ReviewRow::section("Plan"),
            ReviewRow::task("1. migrate opencode sessions", "migrate"),
            ReviewRow::session("s1".into(), "one".into()),
            ReviewRow::session("s2".into(), "two".into()),
            ReviewRow::task("2. jj workspace forget", "forget"),
            ReviewRow::task("4. close panes", "close"),
            ReviewRow::group("ws-a", 2),
            ReviewRow::group("tab-1", 3),
            ReviewRow::pane("p1".into(), "first".into()),
            ReviewRow::pane("p2".into(), "second".into()),
            ReviewRow::group("ws-b", 2),
            ReviewRow::pane("p3".into(), "third".into()),
            ReviewRow::warning(),
        ]
    }

    fn row_index(rows: &[ReviewRow], needle: &str) -> usize {
        rows.iter()
            .position(|row| row.text == needle)
            .unwrap_or_else(|| panic!("no row {needle:?}"))
    }

    fn candidate(pane_id: &str, workspace_id: &str, tab_id: &str) -> PaneCandidate {
        PaneCandidate {
            info: PaneInfo {
                pane_id: pane_id.to_string(),
                tab_id: tab_id.to_string(),
                workspace_id: workspace_id.to_string(),
                label: None,
                agent: None,
                agent_status: None,
                cwd: None,
                foreground_cwd: None,
            },
            matched_via: PaneMatch::Cwd,
        }
    }

    fn stale_review_data() -> ReviewData {
        ReviewData {
            dir: None,
            main_repo: PathBuf::from("/main"),
            workspace_name: Some("ws-old".to_string()),
            target_label: "ws-old (missing on disk)".to_string(),
            availability: plan_availability(None),
            clean: None,
            sessions: SessionPreview::Skipped("workspace path unknown".to_string()),
            panes: Vec::new(),
            workspace_labels: HashMap::new(),
            tab_labels: HashMap::new(),
            triggered_pane: None,
            now_ms: 0,
        }
    }

    fn known_review_data(dir: PathBuf) -> ReviewData {
        let availability = plan_availability(Some(&dir));
        ReviewData {
            dir: Some(dir),
            main_repo: PathBuf::from("/main"),
            workspace_name: None,
            target_label: "/ws".to_string(),
            availability,
            clean: Some(Ok(Vec::new())),
            sessions: SessionPreview::Skipped("no sessions".to_string()),
            panes: Vec::new(),
            workspace_labels: HashMap::new(),
            tab_labels: HashMap::new(),
            triggered_pane: None,
            now_ms: 0,
        }
    }

    #[test]
    fn review_model_selects_all_leaves_by_default() {
        let model = ReviewModel::new(sample_review_rows());
        assert_eq!(model.session_count(), (2, 2));
        assert_eq!(model.pane_count(), (3, 3));
        assert!(model.pane_warning_text().is_none());
        // The cursor starts on the first toggleable row (the first session);
        // task rows are plain text and never cursor targets.
        assert_eq!(model.rows[model.cursor].id.as_deref(), Some("s1"));
        assert!(!model.is_toggleable(1), "migrate task row");
        assert!(!model.is_toggleable(5), "close-panes task row");
    }

    #[test]
    fn review_model_group_header_cascades_to_its_subtree() {
        let rows = sample_review_rows();
        let tab = row_index(&rows, "tab-1");
        let mut model = ReviewModel::new(rows);
        model.cursor = tab;
        assert_eq!(model.group_state(tab), GroupSelection::All);

        model.toggle_current();
        assert!(!model.is_selected(RowKind::Pane, "p1"));
        assert!(!model.is_selected(RowKind::Pane, "p2"));
        assert!(
            model.is_selected(RowKind::Pane, "p3"),
            "other groups untouched"
        );
        assert_eq!(model.group_state(tab), GroupSelection::None);
        assert_eq!(
            model.group_state(row_index(&model.rows, "ws-b")),
            GroupSelection::All
        );

        model.toggle_current();
        assert!(model.is_selected(RowKind::Pane, "p1"));

        // A leaf toggle leaves its group partially selected.
        model.cursor = row_index(&model.rows, "first");
        model.toggle_current();
        assert_eq!(model.group_state(tab), GroupSelection::Partial);
        assert_eq!(
            model.group_state(row_index(&model.rows, "ws-a")),
            GroupSelection::Partial
        );
    }

    #[test]
    fn review_model_cursor_skips_non_toggleable_rows() {
        let mut model = ReviewModel::new(sample_review_rows());
        // First toggleable row is the first session leaf.
        assert_eq!(model.cursor, 2);
        model.move_cursor(1);
        assert_eq!(model.rows[model.cursor].id.as_deref(), Some("s2"));
        // Both task rows ("2. jj workspace forget", "4. close panes") are
        // skipped; the next stop is the ws-a group header.
        model.move_cursor(1);
        assert_eq!(model.rows[model.cursor].text, "ws-a");
        model.move_cursor(1);
        assert_eq!(model.rows[model.cursor].text, "tab-1");
        model.move_cursor(-1);
        assert_eq!(model.rows[model.cursor].text, "ws-a");
        // Even with the cursor forced there, a task row cannot toggle.
        model.cursor = 5;
        model.toggle_current();
        assert_eq!(model.pane_count(), (3, 3), "space on a task row is inert");
    }

    #[test]
    fn review_model_global_toggle_clears_and_restores_everything() {
        let mut model = ReviewModel::new(sample_review_rows());
        model.toggle_all();
        assert_eq!(model.session_count(), (0, 2));
        assert_eq!(model.pane_count(), (0, 3));
        assert_eq!(
            model.pane_warning_text().as_deref(),
            Some("all 3 pane(s) keep running; their cwd will be deleted")
        );
        model.toggle_all();
        assert_eq!(model.session_count(), (2, 2));
        assert_eq!(model.pane_count(), (3, 3));
        assert!(model.pane_warning_text().is_none());
    }

    #[test]
    fn review_model_counts_and_warning_detect_unchecked_panes() {
        let rows = sample_review_rows();
        let first_pane = row_index(&rows, "first");
        let mut model = ReviewModel::new(rows);
        model.cursor = first_pane;
        model.toggle_current();
        assert_eq!(model.pane_count(), (2, 3));
        assert_eq!(model.unselected_pane_count(), 1);
        assert_eq!(
            model.pane_warning_text().as_deref(),
            Some("1 pane(s) keep running; their cwd will be deleted")
        );
        assert_eq!(model.session_count(), (2, 2), "sessions untouched");
    }

    #[test]
    fn review_model_selected_ids_follow_row_order() {
        let rows = sample_review_rows();
        let second_session = row_index(&rows, "two");
        let mut model = ReviewModel::new(rows);
        model.cursor = second_session;
        model.toggle_current();
        assert_eq!(model.selected_session_ids(), vec!["s1"]);
        assert_eq!(model.selected_pane_ids(), vec!["p1", "p2", "p3"]);
    }

    #[test]
    fn review_model_scroll_offset_keeps_cursor_visible() {
        let mut model = ReviewModel::new(sample_review_rows());
        assert_eq!(model.scroll_offset(10), 0);
        assert_eq!(model.scroll_offset(0), 0);
        // The cursor no longer derives the offset: a manual scroll sticks...
        model.scroll_by(5, 5, model.rows.len());
        assert_eq!(model.scroll_offset(5), 5);
        // ...and moving the cursor re-follows it minimally.
        model.cursor = 12;
        model.follow_cursor(5);
        assert_eq!(model.scroll_offset(5), 8);
    }

    #[test]
    fn review_model_scroll_by_clamps_to_content_bounds() {
        let mut model = ReviewModel::new(sample_review_rows());
        let rows = model.rows.len();
        assert_eq!(rows, 13);

        model.scroll_by(3, 5, rows);
        assert_eq!(model.scroll_offset(5), 3);
        // Bottom clamp: 13 rows, 5 visible → max offset 8.
        model.scroll_by(100, 5, rows);
        assert_eq!(model.scroll_offset(5), 8);
        // Top clamp.
        model.scroll_by(-100, 5, rows);
        assert_eq!(model.scroll_offset(5), 0);
        // No overflow: everything fits, offset stays 0.
        model.scroll_by(5, 20, rows);
        assert_eq!(model.scroll_offset(20), 0);
        // The getter also clamps a stale stored offset.
        model.scroll_by(8, 5, rows);
        assert_eq!(model.scroll_offset(20), 0);
    }

    #[test]
    fn review_model_cursor_move_refollows_after_manual_scroll() {
        let mut model = ReviewModel::new(sample_review_rows());
        let rows = model.rows.len();

        // Scroll to the bottom, then move the cursor back above the viewport.
        model.scroll_by(100, 5, rows);
        assert_eq!(model.scroll_offset(5), 8);
        model.cursor = 2;
        model.move_cursor(-1); // 2 is the first toggleable row; cursor stays
        model.follow_cursor(5);
        assert_eq!(model.scroll_offset(5), 2);

        // Moving down past the viewport follows the cursor by one step.
        model.cursor = 7;
        model.follow_cursor(5);
        assert_eq!(model.scroll_offset(5), 3);
    }

    #[test]
    fn review_rows_render_scrollbar_only_when_content_overflows() {
        // Fits: 13 rows in a 40-line terminal → no scrollbar arrows.
        let fits = ReviewModel::new(sample_review_rows());
        let backend = TestBackend::new(100, 40);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| draw_review_dialog(frame, &fits, DialogMode::Review, None, "", 0))
            .expect("draw review dialog");
        let buffer = terminal.backend().buffer();
        assert!(
            !buffer_contains(buffer, '▲') && !buffer_contains(buffer, '▼'),
            "no scrollbar without overflow"
        );

        // Overflow: 60 rows in the same viewport → scrollbar arrows render.
        let many = ReviewModel::new(
            (0..60)
                .map(|index| ReviewRow::note(format!("line {index}"), 0, RowTone::Dim))
                .collect(),
        );
        let backend = TestBackend::new(100, 40);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| draw_review_dialog(frame, &many, DialogMode::Review, None, "", 0))
            .expect("draw review dialog");
        let buffer = terminal.backend().buffer();
        assert!(
            buffer_contains(buffer, '▲') && buffer_contains(buffer, '▼'),
            "scrollbar arrows must render when the rows overflow"
        );
    }

    #[test]
    fn plan_availability_skips_path_scoped_tasks() {
        assert_eq!(
            plan_availability(Some(Path::new("/ws"))),
            PlanAvailability {
                migrate: true,
                forget: true,
                delete: true,
                close_panes: true,
            }
        );
        let stale = plan_availability(None);
        assert!(!stale.migrate && stale.forget && !stale.delete && !stale.close_panes);
    }

    #[test]
    fn review_rows_mark_unknown_path_targets_as_skipped() {
        let rows = build_review_rows(&stale_review_data());
        let skipped = rows
            .iter()
            .filter(|row| row.text == "skipped (workspace path unknown)")
            .count();
        assert_eq!(skipped, 3, "migrate, delete and close panes: {rows:#?}");
        assert!(
            !rows
                .iter()
                .any(|row| matches!(row.kind, RowKind::Session | RowKind::Pane)),
            "no session/pane rows can be scoped without a path"
        );
        assert!(rows
            .iter()
            .any(|row| row.kind == RowKind::Note && row.text.contains("missing on disk")));
        assert!(rows
            .iter()
            .any(|row| row.kind == RowKind::Check
                && row.text.contains("clean working copy: skipped")));
        assert!(rows.iter().any(
            |row| row.kind == RowKind::Check && row.text.contains("opencode sessions: skipped")
        ));
        // Forget is always available and keeps its shared-store note.
        assert!(rows
            .iter()
            .any(|row| row.kind == RowKind::Note && row.text.contains("bookmarks stay")));
    }

    #[test]
    fn review_plan_omits_path_scoped_selections_for_unknown_paths() {
        let model = ReviewModel::new(sample_review_rows());
        let stale = model.to_plan(None, PathBuf::from("/main"), Some("ws-old".into()));
        assert_eq!(stale.dir, None);
        assert!(stale.session_ids.is_empty());
        assert!(stale.pane_ids.is_empty());

        let known = model.to_plan(Some(PathBuf::from("/ws")), PathBuf::from("/main"), None);
        assert_eq!(known.workspace_name, None);
        assert_eq!(known.session_ids, vec!["s1", "s2"]);
        assert_eq!(known.pane_ids, vec!["p1", "p2", "p3"]);
    }

    #[test]
    fn review_blocking_reason_reports_dirty_and_refused_checks() {
        let mut data = known_review_data(PathBuf::from("/ws"));
        assert!(review_blocking_reason(&data).is_none());

        data.clean = Some(Ok(vec!["M a.txt".to_string()]));
        assert!(review_blocking_reason(&data)
            .expect("dirty blocks")
            .contains("uncommitted change(s)"));

        data.clean = Some(Err(
            "cannot check workspace 'ws' (refusing to remove): boom".to_string(),
        ));
        let reason = review_blocking_reason(&data).expect("check failure blocks");
        assert!(reason.starts_with("blocked: cannot check"), "{reason}");

        data.clean = Some(Ok(Vec::new()));
        data.sessions = SessionPreview::Refused("schema mismatch".to_string());
        let reason = review_blocking_reason(&data).expect("refused preview blocks");
        assert!(reason.contains("schema mismatch"), "{reason}");

        // A stale target skips the clean check and has no sessions to block.
        assert!(review_blocking_reason(&stale_review_data()).is_none());
    }

    #[test]
    fn review_data_offers_commit_only_for_a_dirty_known_path() {
        let mut data = known_review_data(PathBuf::from("/ws"));
        assert!(!data.can_commit(), "clean check is not a commit case");
        data.clean = Some(Ok(vec!["M a.txt".to_string()]));
        assert!(data.can_commit());
        data.clean = Some(Err("boom".to_string()));
        assert!(
            !data.can_commit(),
            "a failed check is fail-closed, not a commit case"
        );
        data.dir = None;
        assert!(!data.can_commit(), "no path means no commit cwd");
    }

    #[test]
    fn group_panes_preserves_first_seen_hierarchy() {
        let panes = vec![
            candidate("p1", "w1", "t1"),
            candidate("p2", "w1", "t1"),
            candidate("p3", "w1", "t2"),
            candidate("p4", "w2", "t3"),
        ];
        let groups = group_panes(&panes);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0, "w1");
        assert_eq!(groups[0].1.len(), 2);
        assert_eq!(groups[0].1[0].0, "t1");
        assert_eq!(
            groups[0].1[0]
                .1
                .iter()
                .map(|pane| pane.info.pane_id.as_str())
                .collect::<Vec<_>>(),
            vec!["p1", "p2"]
        );
        assert_eq!(groups[0].1[1].0, "t2");
        assert_eq!(groups[1].0, "w2");
    }

    #[test]
    fn status_state_skips_path_scoped_tasks_for_unknown_paths() {
        let stale = StatusState::new(None);
        let state_of = |state: &StatusState, task: StatusTask| {
            state
                .items
                .iter()
                .find(|item| item.task == task)
                .map(|item| item.state.clone())
                .expect("task present")
        };
        assert!(matches!(
            state_of(&stale, StatusTask::Migrate),
            StatusItemState::Skipped(_)
        ));
        assert_eq!(
            state_of(&stale, StatusTask::Forget),
            StatusItemState::Pending
        );
        assert!(matches!(
            state_of(&stale, StatusTask::Delete),
            StatusItemState::Skipped(_)
        ));
        assert!(matches!(
            state_of(&stale, StatusTask::ClosePanes),
            StatusItemState::Skipped(_)
        ));

        let known = StatusState::new(Some(Path::new("/ws")));
        assert!(known
            .items
            .iter()
            .all(|item| item.state == StatusItemState::Pending));
        assert!(!known.any_failure());
    }

    #[test]
    fn status_state_tracks_progress_and_failures() {
        let mut state = StatusState::new(Some(Path::new("/ws")));
        state.running(StatusTask::Migrate);
        state.done(StatusTask::Migrate, "2 session(s) migrated");
        state.skipped(StatusTask::ClosePanes, "no panes selected");
        assert!(!state.any_failure());

        state.failed(StatusTask::Forget, "jj workspace forget failed (exit 1)");
        assert!(state.any_failure());
        let (task, reason) = state.first_failure().expect("failure recorded");
        assert_eq!(task, StatusTask::Forget);
        assert!(reason.contains("forget failed"));

        let migrate = state
            .items
            .iter()
            .find(|item| item.task == StatusTask::Migrate)
            .expect("migrate item");
        let (text, tone) = status_row_text(migrate);
        assert_eq!(tone, RowTone::Ok);
        assert!(
            text.starts_with('✓') && text.contains("2 session(s)"),
            "{text}"
        );

        state.set_error_log("/state/error.log");
        assert_eq!(state.error_log.as_deref(), Some("/state/error.log"));
    }

    #[test]
    fn review_dialog_renders_sections_and_action_buttons() {
        let mut data = known_review_data(PathBuf::from("/ws"));
        data.sessions = SessionPreview::Ready {
            rows: vec![SessionDisplay {
                id: "s1".into(),
                title: "fix the thing".into(),
                directory: "/ws/src".into(),
                time_updated: data.now_ms,
            }],
            main_repo: data.main_repo.clone(),
        };
        data.panes = vec![candidate("pane-1", "w1", "t1")];
        let model = ReviewModel::new(build_review_rows(&data));
        let backend = TestBackend::new(100, 40);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| draw_review_dialog(frame, &model, DialogMode::Review, None, "", 0))
            .expect("draw review dialog");
        let buffer = terminal.backend().buffer();
        for needle in [
            "Remove jj workspace",
            "Workspace",
            "Plan",
            "Checks",
            "1. migrate opencode sessions",
            "2. jj workspace forget",
            "3. delete directory",
            "4. close panes",
            "✓ opencode sessions: 1 bound",
            "migrate to /main",
            "remove",
            "cancel",
        ] {
            assert!(
                !lines_containing(buffer, needle).is_empty(),
                "review dialog must render {needle:?}"
            );
        }
    }

    #[test]
    fn review_rows_render_flush_sections_and_plain_task_rows() {
        let model = ReviewModel::new(sample_review_rows());
        let backend = TestBackend::new(100, 40);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| draw_review_dialog(frame, &model, DialogMode::Review, None, "", 0))
            .expect("draw review dialog");
        let buffer = terminal.backend().buffer();

        // Section titles are flush with the panel content edge (wizard style).
        let plan_y = *lines_containing(buffer, "Plan").first().expect("Plan row");
        let plan_line = line_text(buffer, plan_y);
        // `find` returns byte offsets and the panel border is 3 UTF-8 bytes,
        // so the content edge is border + border char width.
        let border_x = plan_line.find('│').expect("panel border");
        let content_x = border_x + '│'.len_utf8();
        assert_eq!(
            plan_line.find("Plan"),
            Some(content_x),
            "section title must be flush: {plan_line:?}"
        );

        // Task rows are plain bold text: no cursor slot and no checkbox.
        let task_y = *lines_containing(buffer, "1. migrate opencode sessions")
            .first()
            .expect("task row");
        let task_line = line_text(buffer, task_y);
        assert!(
            !task_line.contains("[x]") && !task_line.contains("[ ]") && !task_line.contains("[-]"),
            "task row must not render a checkbox: {task_line:?}"
        );
        assert!(
            task_line.find("1. migrate").expect("task text") > content_x,
            "task rows keep their indentation: {task_line:?}"
        );

        // Group headers and leaves still carry their checkboxes.
        for needle in ["ws-a", "first"] {
            let y = *lines_containing(buffer, needle).first().expect("row");
            let line = line_text(buffer, y);
            assert!(line.contains("[x] "), "{needle}: {line:?}");
        }
    }

    #[test]
    fn display_home_path_abbreviates_home_at_component_boundaries() {
        let home = Path::new("/home/cyc");
        assert_eq!(
            display_home_path_with(Path::new("/home/cyc"), Some(home)),
            "~"
        );
        assert_eq!(
            display_home_path_with(Path::new("/home/cyc/ws"), Some(home)),
            "~/ws"
        );
        assert_eq!(
            display_home_path_with(Path::new("/home/cyc/ws/src"), Some(home)),
            "~/ws/src"
        );
        assert_eq!(
            display_home_path_with(Path::new("/home/cycx/ws"), Some(home)),
            "/home/cycx/ws",
            "a sibling with the same prefix must stay verbatim"
        );
        assert_eq!(
            display_home_path_with(Path::new("/tmp/ws"), Some(home)),
            "/tmp/ws"
        );
        assert_eq!(
            display_home_path_with(Path::new("/home/cyc/ws"), None),
            "/home/cyc/ws",
            "HOME unset degrades to the plain path"
        );
        assert_eq!(
            display_home_path_with(Path::new("/home/cyc/ws"), Some(Path::new(""))),
            "/home/cyc/ws",
            "empty HOME degrades to the plain path"
        );
    }

    #[test]
    fn review_rows_use_dotted_steps_and_a_two_line_opencode_check() {
        let mut data = known_review_data(PathBuf::from("/ws"));
        data.sessions = SessionPreview::Ready {
            rows: vec![SessionDisplay {
                id: "s1".into(),
                title: "fix".into(),
                directory: "/ws".into(),
                time_updated: data.now_ms,
            }],
            main_repo: PathBuf::from("/main"),
        };
        let rows = build_review_rows(&data);

        let steps: Vec<&str> = rows
            .iter()
            .filter(|row| row.kind == RowKind::Task)
            .map(|row| row.text.as_str())
            .collect();
        assert_eq!(
            steps,
            vec![
                "1. migrate opencode sessions",
                "2. jj workspace forget",
                "3. delete directory",
                "4. close panes",
            ]
        );

        // The opencode check is a one-line verdict plus a dim destination
        // note on the next row (same convention as preserve:/discard:).
        let check_index = rows
            .iter()
            .position(|row| row.text == "✓ opencode sessions: 1 bound")
            .expect("one-line check without the destination");
        assert_eq!(rows[check_index].kind, RowKind::Check);
        assert_eq!(rows[check_index].tone, RowTone::Ok);
        let detail = &rows[check_index + 1];
        assert_eq!(detail.kind, RowKind::Note);
        assert_eq!(detail.tone, RowTone::Dim);
        assert_eq!(detail.depth, 2);
        assert_eq!(detail.text, "migrate to /main");
    }

    #[test]
    fn review_rows_render_live_selection_counts() {
        let mut data = known_review_data(PathBuf::from("/ws"));
        data.sessions = SessionPreview::Ready {
            rows: vec![
                SessionDisplay {
                    id: "s1".into(),
                    title: "one".into(),
                    directory: "/ws".into(),
                    time_updated: data.now_ms,
                },
                SessionDisplay {
                    id: "s2".into(),
                    title: "two".into(),
                    directory: "/ws".into(),
                    time_updated: data.now_ms,
                },
            ],
            main_repo: data.main_repo.clone(),
        };
        data.panes = vec![
            candidate("pane-1", "w1", "t1"),
            candidate("pane-2", "w1", "t1"),
            candidate("pane-3", "w2", "t2"),
        ];
        let rows = build_review_rows(&data);
        let session_count = rows
            .iter()
            .position(|row| row.id.as_deref() == Some(SESSION_COUNT_ID))
            .expect("session count row");
        let pane_count = rows
            .iter()
            .position(|row| row.id.as_deref() == Some(PANE_COUNT_ID))
            .expect("pane count row");
        assert!(
            session_count
                < rows
                    .iter()
                    .position(|row| row.kind == RowKind::Session)
                    .unwrap(),
            "count sits above the session leaves"
        );
        assert!(
            pane_count
                < rows
                    .iter()
                    .position(|row| row.kind == RowKind::Pane)
                    .unwrap(),
            "count sits above the pane tree"
        );

        let mut model = ReviewModel::new(rows);
        assert_eq!(
            model.session_count_text().as_deref(),
            Some("2 of 2 selected")
        );
        assert_eq!(model.pane_count_text().as_deref(), Some("3 of 3 selected"));

        // Live: a leaf toggle changes the text without rebuilding the rows.
        model.cursor = model
            .rows
            .iter()
            .position(|row| row.id.as_deref() == Some("s1"))
            .expect("s1 row");
        model.toggle_current();
        assert_eq!(
            model.session_count_text().as_deref(),
            Some("1 of 2 selected")
        );
        model.cursor = model
            .rows
            .iter()
            .position(|row| row.id.as_deref() == Some("pane-1"))
            .expect("pane row");
        model.toggle_current();
        assert_eq!(model.pane_count_text().as_deref(), Some("2 of 3 selected"));

        // The rendered dialog shows both live counts.
        let backend = TestBackend::new(100, 40);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| draw_review_dialog(frame, &model, DialogMode::Review, None, "", 0))
            .expect("draw review dialog");
        let buffer = terminal.backend().buffer();
        assert!(
            !lines_containing(buffer, "1 of 2 selected").is_empty(),
            "session count must render"
        );
        assert!(
            !lines_containing(buffer, "2 of 3 selected").is_empty(),
            "pane count must render"
        );
    }

    #[test]
    fn review_rows_omit_counts_without_leaves() {
        let stale = build_review_rows(&stale_review_data());
        assert!(
            !stale.iter().any(|row| matches!(
                row.id.as_deref(),
                Some(SESSION_COUNT_ID) | Some(PANE_COUNT_ID)
            )),
            "stale targets have no leaves to count"
        );

        // Known path but no sessions and no panes: still no count rows.
        let empty = build_review_rows(&known_review_data(PathBuf::from("/ws")));
        assert!(!empty.iter().any(|row| matches!(
            row.id.as_deref(),
            Some(SESSION_COUNT_ID) | Some(PANE_COUNT_ID)
        )));
        let model = ReviewModel::new(empty);
        assert!(model.session_count_text().is_none());
        assert!(model.pane_count_text().is_none());
    }

    #[test]
    fn picker_scrollbar_renders_only_on_overflow() {
        let entries = |count: usize| -> Vec<WorkspaceEntry> {
            (0..count)
                .map(|index| WorkspaceEntry {
                    name: format!("ws-{index}"),
                    root: Some(PathBuf::from(format!("/tmp/ws-{index}"))),
                })
                .collect()
        };

        let fits = PickerState {
            entries: entries(3),
            cursor: 0,
            scroll: 0,
            error: None,
        };
        let backend = TestBackend::new(100, 40);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| draw_picker(frame, &fits))
            .expect("draw picker");
        let buffer = terminal.backend().buffer();
        assert!(
            !buffer_contains(buffer, '▲') && !buffer_contains(buffer, '▼'),
            "no picker scrollbar without overflow"
        );

        let many = PickerState {
            entries: entries(60),
            cursor: 0,
            scroll: 0,
            error: None,
        };
        let backend = TestBackend::new(100, 40);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| draw_picker(frame, &many))
            .expect("draw picker");
        let buffer = terminal.backend().buffer();
        assert!(
            buffer_contains(buffer, '▲') && buffer_contains(buffer, '▼'),
            "picker scrollbar must render when entries overflow"
        );
    }

    #[test]
    fn status_view_renders_failure_reason_and_log_pointer() {
        let mut state = StatusState::new(Some(Path::new("/ws")));
        state.done(StatusTask::Migrate, "1 session migrated");
        state.failed(StatusTask::Forget, "forget exploded");
        state.set_error_log("/state/error.log");
        let backend = TestBackend::new(100, 40);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| draw_status(frame, &state, true))
            .expect("draw status view");
        let buffer = terminal.backend().buffer();
        for needle in [
            "migrate opencode sessions",
            "1 session migrated",
            "forget exploded",
            "/state/error.log",
        ] {
            assert!(
                !lines_containing(buffer, needle).is_empty(),
                "status view must render {needle:?}"
            );
        }
    }

    // --- wizard render tests --------------------------------------------
    //
    // draw_workspace_wizard is a pure function over a Frame; a TestBackend
    // captures the frame buffer so the section layout, title grammar, shared
    // content indent, hint line and the read-only source section are
    // assertable without a TTY.

    use ratatui::backend::TestBackend;

    fn wizard_view<'a>(
        source: &'a str,
        field: WizardField,
        name: &'a str,
        base: &'a str,
        error: Option<&'a str>,
    ) -> WizardView<'a> {
        WizardView {
            source,
            field,
            name,
            name_cursor: name.chars().count(),
            base,
            base_cursor: base.chars().count(),
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

    fn buffer_contains(buffer: &ratatui::buffer::Buffer, needle: char) -> bool {
        buffer
            .content
            .iter()
            .any(|cell| cell.symbol() == needle.to_string())
    }

    #[test]
    fn wizard_renders_sectioned_layout_with_unified_titles_and_indent() {
        let buffer = render_wizard(wizard_view(
            "/home/nathan/agent-os",
            WizardField::Name,
            "ws/alpha",
            "trunk()",
            None,
        ));

        // Modal header + single static hint line at the top.
        let hint = lines_containing(&buffer, "type to edit · tab switch · ↵ create · esc cancel");
        assert_eq!(hint.len(), 1, "hint must appear exactly once: {hint:?}");
        assert!(hint[0] > 0, "hint sits below the modal header");

        // Section titles each on their own line, no operation-hint suffixes.
        let name = lines_containing(&buffer, "New Workspace Name");
        assert_eq!(name.len(), 1, "{name:?}");
        let base = lines_containing(&buffer, "Base · jj revset");
        assert_eq!(base.len(), 1, "{base:?}");
        let source = lines_containing(&buffer, "Source Workspace");
        assert_eq!(source.len(), 1, "{source:?}");
        let checkout = lines_containing(&buffer, "Checkout");
        assert_eq!(checkout.len(), 1, "{checkout:?}");
        // No stray per-section hints, and no list-selection wording anywhere.
        assert!(lines_containing(&buffer, "tab to edit").is_empty());
        assert!(lines_containing(&buffer, "type to filter").is_empty());
        assert!(lines_containing(&buffer, "select").is_empty());

        // Titles render bold: assert on the first glyph column of each title line.
        for (y, title_text) in [
            (name[0], "New Workspace Name"),
            (base[0], "Base · jj revset"),
            (source[0], "Source Workspace"),
            (checkout[0], "Checkout"),
        ] {
            let title_start = line_text(&buffer, y).find(title_text).unwrap_or(0) as u16;
            let cell = &buffer[(title_start, y)];
            assert!(
                cell.style().add_modifier.contains(Modifier::BOLD),
                "title line {y} ({title_text:?}) must be bold"
            );
        }

        // Content lines share the 3-column indent: name/base values, the
        // source path and the checkout path all start at the same column.
        let name_value_y = name[0] + 1;
        assert!(
            line_text(&buffer, name_value_y).contains("   ws/alpha"),
            "name value indent: {:?}",
            line_text(&buffer, name_value_y)
        );
        let base_value_y = base[0] + 1;
        assert!(
            line_text(&buffer, base_value_y).contains("   trunk()"),
            "base value indent: {:?}",
            line_text(&buffer, base_value_y)
        );
        let source_value_y = source[0] + 1;
        assert!(
            line_text(&buffer, source_value_y).contains("   /home/nathan/agent-os"),
            "source value indent: {:?}",
            line_text(&buffer, source_value_y)
        );
        let checkout_value_y = checkout[0] + 1;
        assert!(
            line_text(&buffer, checkout_value_y).contains("   /tmp/wizard-root/agent-os/ws-alpha"),
            "checkout value indent: {:?}",
            line_text(&buffer, checkout_value_y)
        );
    }

    #[test]
    fn wizard_renders_sections_in_name_base_source_checkout_order() {
        let buffer = render_wizard(wizard_view(
            "/tmp/alpha",
            WizardField::Name,
            "ws/alpha",
            "trunk()",
            None,
        ));
        let ys = [
            lines_containing(&buffer, "New Workspace Name")[0],
            lines_containing(&buffer, "Base · jj revset")[0],
            lines_containing(&buffer, "Source Workspace")[0],
            lines_containing(&buffer, "Checkout")[0],
        ];
        assert!(
            ys.windows(2).all(|pair| pair[0] < pair[1]),
            "sections must render in fixed order, got y positions {ys:?}"
        );
    }

    #[test]
    fn wizard_focus_switches_title_color_only() {
        // Name focused: its title is accent, every other title subtext0 —
        // including the read-only Source/Checkout — and all stay bold.
        let focused_name = render_wizard(wizard_view(
            "/tmp/alpha",
            WizardField::Name,
            "ws/alpha",
            "trunk()",
            None,
        ));
        let name_y = lines_containing(&focused_name, "New Workspace Name")[0];
        let base_y = lines_containing(&focused_name, "Base · jj revset")[0];
        let source_y = lines_containing(&focused_name, "Source Workspace")[0];
        let checkout_y = lines_containing(&focused_name, "Checkout")[0];
        for (y, text) in [
            (name_y, "New Workspace Name"),
            (base_y, "Base · jj revset"),
            (source_y, "Source Workspace"),
            (checkout_y, "Checkout"),
        ] {
            let x = line_text(&focused_name, y).find(text).unwrap_or(0) as u16;
            let cell = &focused_name[(x, y)];
            let expected = if y == name_y {
                palette_accent()
            } else {
                palette_subtext0()
            };
            assert_eq!(cell.style().fg, Some(expected), "title {text:?}");
            assert!(
                cell.style().add_modifier.contains(Modifier::BOLD),
                "title {text:?} stays bold"
            );
        }

        // Base focused: name falls back to subtext0, base turns accent, and
        // the read-only titles never change color.
        let focused_base = render_wizard(wizard_view(
            "/tmp/alpha",
            WizardField::Base,
            "ws/alpha",
            "trunk()",
            None,
        ));
        let name_y = lines_containing(&focused_base, "New Workspace Name")[0];
        let base_y = lines_containing(&focused_base, "Base · jj revset")[0];
        let name_x = line_text(&focused_base, name_y)
            .find("New Workspace Name")
            .unwrap_or(0) as u16;
        let base_x = line_text(&focused_base, base_y)
            .find("Base · jj revset")
            .unwrap_or(0) as u16;
        let name_cell = &focused_base[(name_x, name_y)];
        let base_cell = &focused_base[(base_x, base_y)];
        assert_eq!(name_cell.style().fg, Some(palette_subtext0()));
        assert_eq!(base_cell.style().fg, Some(palette_accent()));
        assert!(base_cell.style().add_modifier.contains(Modifier::BOLD));
        // Read-only titles stay subtext0 no matter which field is focused.
        for (y, text) in [
            (source_y, "Source Workspace"),
            (checkout_y, "Checkout"),
        ] {
            let x = line_text(&focused_base, y).find(text).unwrap_or(0) as u16;
            assert_eq!(
                focused_base[(x, y)].style().fg,
                Some(palette_subtext0()),
                "read-only title {text:?} must stay subtext0"
            );
        }
    }

    /// Renders the wizard with explicit field cursors for block-placement
    /// assertions on the focused field.
    fn render_wizard_with_cursors(
        field: WizardField,
        name: &str,
        name_cursor: usize,
        base: &str,
        base_cursor: usize,
    ) -> ratatui::buffer::Buffer {
        render_wizard(WizardView {
            source: "/tmp/alpha",
            field,
            name,
            name_cursor,
            base,
            base_cursor,
            root: Path::new("/tmp/wizard-root"),
            error: None,
        })
    }

    #[test]
    fn wizard_renders_name_cursor_block_at_the_cursor_position() {
        let buffer =
            render_wizard_with_cursors(WizardField::Name, "workspace/fix", 12, "trunk()", 7);
        let name_y = lines_containing(&buffer, "New Workspace Name")[0] + 1;
        assert!(
            line_text(&buffer, name_y).contains("   workspace/fi█x"),
            "name cursor block: {:?}",
            line_text(&buffer, name_y)
        );
        // The unfocused base field renders plain, without a block.
        let base_y = lines_containing(&buffer, "Base · jj revset")[0] + 1;
        assert!(!line_text(&buffer, base_y).contains('█'));
    }

    #[test]
    fn wizard_renders_base_cursor_block_at_the_cursor_position() {
        let buffer =
            render_wizard_with_cursors(WizardField::Base, "workspace/fix", 13, "trunk()", 5);
        let base_y = lines_containing(&buffer, "Base · jj revset")[0] + 1;
        assert!(
            line_text(&buffer, base_y).contains("   trunk█()"),
            "base cursor block: {:?}",
            line_text(&buffer, base_y)
        );
        // The unfocused name field renders plain, without a block.
        let name_y = lines_containing(&buffer, "New Workspace Name")[0] + 1;
        assert!(!line_text(&buffer, name_y).contains('█'));
    }

    #[test]
    fn review_dialog_renders_commit_cursor_at_the_cursor_position() {
        let model = ReviewModel::new(sample_review_rows());
        let backend = TestBackend::new(100, 40);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| draw_review_dialog(frame, &model, DialogMode::Commit, None, "fix", 2))
            .expect("draw review dialog");
        let buffer = terminal.backend().buffer();
        let y = *lines_containing(buffer, "c commit>")
            .first()
            .expect("commit status line");
        assert!(
            line_text(buffer, y).contains("c commit> fi█x"),
            "commit cursor block: {:?}",
            line_text(buffer, y)
        );
    }

    /// Palette probes for render assertions (mirrors `catppuccin`).
    fn palette_accent() -> Color {
        Color::Rgb(137, 180, 250)
    }
    fn palette_subtext0() -> Color {
        Color::Rgb(166, 173, 200)
    }
}
