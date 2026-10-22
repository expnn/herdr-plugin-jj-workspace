//! opencode session migration for `remove`.
//!
//! opencode binds every session to a directory (`directory`) and a project
//! (`project_id`). When a jj workspace directory is deleted, those rows become
//! invisible forever, so `cmd_remove` migrates them to the main repo first,
//! mirroring opencode's own `SessionEvent.Moved` projection plus its project
//! adoption: `project_id` is rewritten to the main repo's project, `directory`
//! to the main repo root, `path` flattened to `''`, `workspace_id` released to
//! NULL and `time_updated` stamped now.
//!
//! The live session table is selected by schema presence: opencode v2 writes
//! `session_v2` (the v1 `session` table it leaves behind is history only), so a
//! DB containing `session_v2` migrates that table exclusively; a v1-only DB
//! keeps using `session`. Version strings are never consulted, and the table
//! name is an internal constant — never user input.
//!
//! The removal dialog previews the rows a migration would rewrite with the
//! read-only [`inspect_opencode_sessions`] (same discovery chain and range
//! predicate, SELECTs only) and then migrates an arbitrary subset of the
//! previewed ids via [`migrate_selected_opencode_sessions`]. An empty
//! selection skips; ids that disappeared between preview and execution are
//! reported by the actual updated count (drift is not a failure).
//!
//! The DB is located CLI-first (`opencode db path`; the binary is resolved on
//! PATH plus the well-known install dirs `~/.opencode/bin`, `~/.local/bin`,
//! `/usr/local/bin`) and falls back to the standard data locations
//! `$XDG_DATA_HOME/opencode/opencode.db` /
//! `~/.local/share/opencode/opencode.db`. Preview and execution share that
//! chain (D12); when neither yields an existing DB the step skips with a
//! reason naming the tried locations.
//!
//! Every failure that could leave data half-written is fail-closed: the
//! caller refuses to remove the workspace and the user can retry. A missing
//! opencode binary or an empty match set is a silent skip — the plugin stays
//! agent-agnostic (design D8).
//!
//! Design decisions D1–D10 live in
//! `openspec/changes/migrate-opencode-sessions-on-remove/design.md`; D6 and
//! D12 of `openspec/changes/remove-workspace-dialog/design.md` add preview,
//! subset selection and PATH-independent DB discovery.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, TransactionBehavior};

/// Result of a migration attempt, mapped by `cmd_remove` to continue /
/// refuse / report.
#[derive(Debug)]
pub enum Outcome {
    /// Nothing to migrate (opencode absent, DB absent, or no matching
    /// sessions). The caller proceeds with the removal.
    Skipped(String),
    /// Fail-closed: the caller must abort the removal (data untouched,
    /// retryable). The string is the user-facing reason.
    Refused(String),
    /// N sessions were migrated to `main_repo`.
    Migrated { count: usize, main_repo: PathBuf },
}

/// One preview row bound to the workspace, in the shape the removal dialog
/// renders (`title` is `''` when the schema predates the column).
#[derive(Debug)]
pub struct SessionRow {
    pub id: String,
    pub title: String,
    pub directory: String,
    pub time_updated: i64,
}

/// Read-only preview of the migration, mapped by the dialog to
/// skip / block / list rows.
#[derive(Debug)]
pub enum Inspection {
    /// opencode absent / DB absent / no matching rows — nothing to migrate.
    Skipped(String),
    /// DB open, schema probe or target project resolution failed
    /// (fail-closed).
    Refused(String),
    /// Rows bound to the workspace, newest first, plus the resolved
    /// destination.
    Ready {
        rows: Vec<SessionRow>,
        main_repo: PathBuf,
    },
}

/// The live opencode session table. opencode v2 writes `session_v2`; the v1
/// `session` table is kept as history only. Selected by schema presence (never
/// by a version string) and used verbatim in generated SQL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionStore {
    V1,
    V2,
}

impl SessionStore {
    /// Physical table name (internal constant — never user input).
    fn table(self) -> &'static str {
        match self {
            SessionStore::V1 => "session",
            SessionStore::V2 => "session_v2",
        }
    }

    fn is_v2(self) -> bool {
        matches!(self, SessionStore::V2)
    }
}

/// Columns the migration reads or writes; probed before any write (D7).
const SESSION_REQUIRED_COLUMNS: &[&str] = &[
    "id",
    "project_id",
    "directory",
    "path",
    "workspace_id",
    "time_updated",
];
const PROJECT_REQUIRED_COLUMNS: &[&str] = &["id", "worktree"];

/// Range predicate matching a workspace root and everything beneath it.
/// `'/'` is 0x2F and `'0'` is 0x30, so `[ws + '/', ws + '0')` frames exactly
/// `ws/**`. Never `LIKE`: `_` is a single-character wildcard and herdr
/// workspace names routinely contain underscores (D4).
const WORKSPACE_RANGE: &str =
    "directory = ?1 OR (directory >= ?1 || '/' AND directory < ?1 || '0')";

/// Read-only preview of the migration: enumerate the rows a migration would
/// rewrite, of which the dialog then migrates a selected subset via
/// [`migrate_selected_opencode_sessions`]. SELECTs only — never a write.
/// Discovery absences and an empty match set are skips; DB, schema and target
/// resolution failures are refusals (the dialog renders them as blocking
/// checks).
pub fn inspect_opencode_sessions(ws: &Path, main_repo: &Path) -> Inspection {
    let db_path = match locate_opencode_db() {
        Ok(path) => path,
        Err(reason) => return Inspection::Skipped(reason),
    };
    inspection_from_db(inspect_db(&db_path, ws, main_repo), main_repo)
}

/// Map an [`inspect_db`] result to the public preview shape. `main_repo` is
/// already canonical at the call site (`resolve_main_repo`); echoing it keeps
/// preview and execution pointed at the same destination.
fn inspection_from_db(
    result: Result<Option<Vec<SessionRow>>, String>,
    main_repo: &Path,
) -> Inspection {
    match result {
        Ok(Some(rows)) => Inspection::Ready {
            rows,
            main_repo: main_repo.to_path_buf(),
        },
        Ok(None) => Inspection::Skipped("no opencode sessions bound to this workspace".into()),
        Err(message) => Inspection::Refused(message),
    }
}

/// Migrate only `selected` session ids (a subset of an [`Inspection::Ready`]
/// preview). An empty selection skips without touching the DB; a selection
/// with no rows left in range skips without failing (preview/execution
/// drift).
pub fn migrate_selected_opencode_sessions(
    ws: &Path,
    main_repo: &Path,
    selected: &[String],
) -> Outcome {
    if selected.is_empty() {
        return Outcome::Skipped("no sessions selected".into());
    }
    let db_path = match locate_opencode_db() {
        Ok(path) => path,
        Err(reason) => return Outcome::Skipped(reason),
    };
    selected_outcome_from_db(
        migrate_selected_db(&db_path, ws, main_repo, selected),
        main_repo,
    )
}

/// Map a selected-subset [`migrate_selected_db`] result to the public
/// outcome shape.
fn selected_outcome_from_db(result: Result<Option<usize>, String>, main_repo: &Path) -> Outcome {
    match result {
        Ok(Some(count)) => Outcome::Migrated {
            count,
            main_repo: main_repo.to_path_buf(),
        },
        Ok(None) => Outcome::Skipped("no opencode sessions matched the selection".into()),
        Err(message) => Outcome::Refused(message),
    }
}

/// Resolve the main repo root for a secondary workspace. `repo_root` falls
/// back to its input when the `.jj/repo` pointer is missing or unreadable;
/// here that fallback (or any result equal to the workspace itself) is a
/// failure, never a green light to treat the workspace as its own main repo
/// (D3 fail-closed).
pub fn resolve_main_repo(ws: &Path) -> Option<PathBuf> {
    let resolved = PathBuf::from(crate::repo_root(&ws.to_string_lossy()));
    match (fs::canonicalize(&resolved), fs::canonicalize(ws)) {
        (Ok(main), Ok(workspace)) if main != workspace => Some(main),
        _ => None,
    }
}

/// Environment-driven DB discovery shared by preview and execution (D12):
/// CLI first, standard locations second. `Err` carries the skip reason
/// naming every location tried, so a minimal server PATH is diagnosable.
fn locate_opencode_db() -> Result<PathBuf, String> {
    let home = home_dir();
    let path_dirs = crate::path_dirs();
    let opencode = find_opencode(&path_dirs);
    let candidates = db_candidates(home.as_deref(), xdg_data_home().as_deref());
    discover_db_path(opencode.as_deref(), &candidates).ok_or_else(|| {
        discovery_skip_reason(&path_dirs, opencode.is_some(), &candidates, home.as_deref())
    })
}

/// `HOME`, ignored when unset or empty.
fn home_dir() -> Option<PathBuf> {
    env_dir("HOME")
}

/// `XDG_DATA_HOME`, ignored when unset or empty (an empty value means
/// "unset" per the XDG basedir spec).
fn xdg_data_home() -> Option<PathBuf> {
    env_dir("XDG_DATA_HOME")
}

fn env_dir(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Resolve the `opencode` binary: PATH dirs first, then the well-known
/// install dirs (D12). A missing binary is a skip, not an error (D8).
fn find_opencode(path_dirs: &[PathBuf]) -> Option<PathBuf> {
    find_opencode_in(&opencode_search_dirs(path_dirs, home_dir().as_deref()))
}

/// Search `dirs` in order for an executable `opencode`. Injectable: tests
/// pass their own directory list and never read the process environment.
fn find_opencode_in(dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter()
        .map(|dir| dir.join("opencode"))
        .find(|candidate| crate::is_executable_file(candidate))
}

/// PATH dirs plus the well-known install dirs, deduplicated in order.
fn opencode_search_dirs(path_dirs: &[PathBuf], home: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = path_dirs.to_vec();
    dirs.extend(fallback_binary_dirs(home));
    dedupe_paths(&mut dirs);
    dirs
}

/// Non-PATH dirs that commonly hold the `opencode` binary. HOME-derived
/// entries are skipped when HOME is unset.
fn fallback_binary_dirs(home: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = home {
        dirs.push(home.join(".opencode").join("bin"));
        dirs.push(home.join(".local").join("bin"));
    }
    dirs.push(PathBuf::from("/usr/local/bin"));
    dirs
}

fn dedupe_paths(paths: &mut Vec<PathBuf>) {
    let mut unique: Vec<PathBuf> = Vec::with_capacity(paths.len());
    paths.retain(|path| {
        if unique.contains(path) {
            false
        } else {
            unique.push(path.clone());
            true
        }
    });
}

/// Standard opencode DB location: `$XDG_DATA_HOME/opencode/opencode.db` when
/// `XDG_DATA_HOME` is set, otherwise `$HOME/.local/share/opencode/opencode.db`
/// (D12). Only one location is consulted — a set-but-empty XDG tree must not
/// silently fall through to a possibly stale `~/.local/share` DB.
fn db_candidates(home: Option<&Path>, xdg_data_home: Option<&Path>) -> Vec<PathBuf> {
    if let Some(xdg) = xdg_data_home {
        return vec![xdg.join("opencode").join("opencode.db")];
    }
    home.map(|home| {
        vec![home
            .join(".local")
            .join("share")
            .join("opencode")
            .join("opencode.db")]
    })
    .unwrap_or_default()
}

/// Discover the opencode DB: the CLI's `opencode db path` answer first, then
/// the standard locations; the first existing file wins (D12). `opencode:
/// None` models a missing binary. The CLI only reads the path — it is never
/// used to write (D1).
fn discover_db_path(opencode: Option<&Path>, candidates: &[PathBuf]) -> Option<PathBuf> {
    if let Some(opencode) = opencode {
        if let Some(path) = cli_db_path(opencode) {
            return Some(path);
        }
    }
    candidates
        .iter()
        .find(|candidate| candidate.is_file())
        .cloned()
}

/// Ask opencode for its DB path (`opencode db path`). A missing binary, a
/// non-zero exit and output that is not an existing file are all skips.
fn cli_db_path(opencode: &Path) -> Option<PathBuf> {
    let output = Command::new(opencode).args(["db", "path"]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    db_path_from_output(&output.stdout)
}

/// Skip reason naming what discovery tried; `~` abbreviates a leading HOME.
fn discovery_skip_reason(
    path_dirs: &[PathBuf],
    binary_found: bool,
    candidates: &[PathBuf],
    home: Option<&Path>,
) -> String {
    let fallback = format_paths(&fallback_binary_dirs(home), home);
    let db = if candidates.is_empty() {
        "no standard DB location (HOME and XDG_DATA_HOME are unset)".to_string()
    } else {
        format!("no opencode DB at {}", format_paths(candidates, home))
    };
    if binary_found {
        format!("opencode db path gave no usable DB and {db}")
    } else {
        let searched = if path_dirs.is_empty() {
            fallback
        } else {
            format!("PATH + {fallback}")
        };
        format!("opencode not found (searched {searched}) and {db}")
    }
}

/// Render paths for a user-facing skip reason (`~` for a leading HOME).
fn format_paths(paths: &[PathBuf], home: Option<&Path>) -> String {
    paths
        .iter()
        .map(|path| display_path(path, home))
        .collect::<Vec<_>>()
        .join(", ")
}

fn display_path(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// Parse `opencode db path` stdout: empty output, or a path that is not an
/// existing file, means there is no DB to migrate (skip).
fn db_path_from_output(stdout: &[u8]) -> Option<PathBuf> {
    let raw = String::from_utf8_lossy(stdout);
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let path = PathBuf::from(trimmed);
    if path.is_file() {
        Some(path)
    } else {
        None
    }
}

/// Open the DB with the exact connection parameters the migration uses and
/// run the fail-closed schema probe. Shared by preview and migration so a
/// would-be-working migration is never refused merely because a read-only
/// open failed.
fn open_and_probe(db_path: &Path) -> Result<(Connection, SessionStore), String> {
    let conn = Connection::open(db_path).map_err(|err| {
        format!(
            "cannot open the opencode DB (refusing to remove): {err}\n\
             DB path: {}",
            db_path.display()
        )
    })?;
    conn.busy_timeout(Duration::from_millis(5000))
        .map_err(|err| {
            format!("cannot configure the opencode DB connection (refusing to remove): {err}")
        })?;
    // WAL is already persistent for opencode's DB; only the parameters
    // opencode itself sets need to be repeated here (D2).
    conn.execute_batch("PRAGMA foreign_keys=ON")
        .map_err(|err| {
            format!("cannot configure the opencode DB connection (refusing to remove): {err}")
        })?;
    let store = probe_schema(&conn)?;
    Ok((conn, store))
}

/// Read-only counterpart of the migration: same discovery, schema and
/// target checks, SELECTs only. `Ok(None)` = no matching sessions (skip);
/// `Err` = fail-closed (the dialog blocks).
fn inspect_db(db_path: &Path, ws: &Path, main: &Path) -> Result<Option<Vec<SessionRow>>, String> {
    let (conn, store) = open_and_probe(db_path)?;
    // Mirror the migration: an empty match set is a skip even when the target
    // project cannot be resolved.
    if count_sessions(&conn, store, ws)? == 0 {
        return Ok(None);
    }
    // A target the migration could not resolve must block the preview too,
    // so the dialog never offers a migration that will refuse later.
    resolve_target(&conn, store, main)?;
    Ok(Some(select_sessions(&conn, store, ws)?))
}

/// The rows a migration would rewrite, newest first. `session.title` is not
/// part of the fail-closed probe (opencode added it later), so a schema
/// without it selects a literal `''` — the dialog renders a placeholder.
fn select_sessions(
    conn: &Connection,
    store: SessionStore,
    ws: &Path,
) -> Result<Vec<SessionRow>, String> {
    let table = store.table();
    let has_title = table_columns(conn, table)?
        .iter()
        .any(|name| name == "title");
    let title = if has_title { "title" } else { "''" };
    let ws = ws.to_string_lossy();
    let mut stmt = conn
        .prepare(&format!(
            "SELECT id, {title}, directory, time_updated FROM {table} \
             WHERE {WORKSPACE_RANGE} ORDER BY time_updated DESC, id"
        ))
        .map_err(|err| format!("cannot enumerate opencode sessions (refusing to remove): {err}"))?;
    let rows = stmt
        .query_map(params![ws.as_ref()], |row| {
            Ok(SessionRow {
                id: row.get(0)?,
                title: row.get(1)?,
                directory: row.get(2)?,
                time_updated: row.get(3)?,
            })
        })
        .map_err(|err| format!("cannot enumerate opencode sessions (refusing to remove): {err}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("cannot enumerate opencode sessions (refusing to remove): {err}"))?;
    Ok(rows)
}

/// Migrate only the `selected` session ids: rows outside `selected` are
/// never touched. Zero selected rows left in range is a skip — drift between
/// preview and execution is not a failure.
fn migrate_selected_db(
    db_path: &Path,
    ws: &Path,
    main: &Path,
    selected: &[String],
) -> Result<Option<usize>, String> {
    let (mut conn, store) = open_and_probe(db_path)?;
    if count_selected_sessions(&conn, store, ws, selected)? == 0 {
        return Ok(None);
    }
    let target = resolve_target(&conn, store, main)?;
    let updated = run_migration_tx(&mut conn, store, ws, main, &target, Some(selected))?;
    Ok(Some(updated))
}

/// Select the live session table: `session_v2` when it exists (opencode v2
/// writes it), otherwise `session` (v1). A DB with neither table cannot be
/// reasoned about, so it refuses instead of matching zero rows.
fn select_session_store(conn: &Connection) -> Result<SessionStore, String> {
    if table_exists(conn, "session_v2")? {
        return Ok(SessionStore::V2);
    }
    if table_exists(conn, "session")? {
        return Ok(SessionStore::V1);
    }
    Err(
        "opencode DB has neither session_v2 nor session table (refusing to remove): \
         cannot enumerate opencode sessions"
            .to_string(),
    )
}

/// Fail-closed schema probe (D7): every column the migration touches on the
/// selected live table must exist before the first write. opencode's history is
/// ADD COLUMN-only for these tables, but a future rewrite must stop us rather
/// than corrupt data.
fn probe_schema(conn: &Connection) -> Result<SessionStore, String> {
    let store = select_session_store(conn)?;
    let table = store.table();
    let session_columns = table_columns(conn, table)?;
    for column in SESSION_REQUIRED_COLUMNS {
        if !session_columns.iter().any(|name| name == column) {
            return Err(format!(
                "opencode DB schema is missing {table}.{column} (refusing to remove)"
            ));
        }
    }
    let project_columns = table_columns(conn, "project")?;
    for column in PROJECT_REQUIRED_COLUMNS {
        if !project_columns.iter().any(|name| name == column) {
            return Err(format!(
                "opencode DB schema is missing project.{column} (refusing to remove)"
            ));
        }
    }
    Ok(store)
}

/// Column names of `table` (SQLite `PRAGMA table_info`). Table names come
/// from the constants above only, never from user input.
fn table_columns(conn: &Connection, table: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(schema_error)?;
    let names = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(schema_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(schema_error)?;
    Ok(names)
}

fn schema_error(err: rusqlite::Error) -> String {
    format!("cannot inspect the opencode DB schema (refusing to remove): {err}")
}

/// Number of session rows whose directory is `ws` or below it (D4 range
/// predicate — never LIKE).
fn count_sessions(conn: &Connection, store: SessionStore, ws: &Path) -> Result<usize, String> {
    let ws = ws.to_string_lossy();
    let table = store.table();
    let count: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE {WORKSPACE_RANGE}"),
            params![ws.as_ref()],
            |row| row.get(0),
        )
        .map_err(|err| format!("cannot enumerate opencode sessions (refusing to remove): {err}"))?;
    Ok(count.max(0) as usize)
}

/// Like [`count_sessions`], but only rows whose id is in `selected`. An empty
/// selection matches nothing.
fn count_selected_sessions(
    conn: &Connection,
    store: SessionStore,
    ws: &Path,
    selected: &[String],
) -> Result<usize, String> {
    if selected.is_empty() {
        return Ok(0);
    }
    let placeholders = id_placeholders(2, selected.len());
    let mut values = vec![Value::Text(ws.to_string_lossy().into_owned())];
    values.extend(selected.iter().cloned().map(Value::Text));
    let table = store.table();
    let count: i64 = conn
        .query_row(
            &format!(
                "SELECT COUNT(*) FROM {table} \
                 WHERE ({WORKSPACE_RANGE}) AND id IN ({placeholders})"
            ),
            params_from_iter(values),
            |row| row.get(0),
        )
        .map_err(|err| format!("cannot enumerate opencode sessions (refusing to remove): {err}"))?;
    Ok(count.max(0) as usize)
}

/// Numbered placeholders `?start, ?start+1, …` for a dynamically sized `IN`
/// list. Ids are always bound as parameters, never interpolated into SQL.
fn id_placeholders(start: usize, count: usize) -> String {
    (start..start + count)
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Resolve the main repo's opencode project id without re-deriving any hash.
/// The selected live table decides the chain: v2 trusts the durable
/// `project.worktree` answer only; v1 keeps its historical chain.
fn resolve_target(conn: &Connection, store: SessionStore, main: &Path) -> Result<String, String> {
    if store.is_v2() {
        return resolve_target_v2(conn, main);
    }
    resolve_target_v1(conn, main)
}

/// opencode v2 resolves non-git directories to `directory:<hash>` projects, so
/// the v1 `'global'` assumption no longer holds. The `project` row keyed by the
/// main repo's worktree is opencode's own durable answer; require exactly one.
fn resolve_target_v2(conn: &Connection, main: &Path) -> Result<String, String> {
    let main_str = main.to_string_lossy();
    let rows: Vec<String> = conn
        .prepare("SELECT id FROM project WHERE worktree = ?1")
        .map_err(|err| {
            format!("cannot query the opencode project table (refusing to remove): {err}")
        })?
        .query_map(params![main_str.as_ref()], |row| row.get(0))
        .map_err(|err| {
            format!("cannot query the opencode project table (refusing to remove): {err}")
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| {
            format!("cannot query the opencode project table (refusing to remove): {err}")
        })?;
    match rows.len() {
        1 => Ok(rows.into_iter().next().expect("one row")),
        0 => Err(format!(
            "open opencode in the main repo once, then retry remove\n\
             could not resolve the main repo's opencode project id for opencode v2 \
             (no project row has worktree = the main repo root)\n\
             main repo: {}\n\
             the workspace was not removed; opencode session data is unchanged",
            main.display()
        )),
        count => Err(format!(
            "open opencode in the main repo once, then retry remove\n\
             the main repo's opencode project id is ambiguous for opencode v2 \
             ({count} project rows have worktree = the main repo root)\n\
             main repo: {}\n\
             the workspace was not removed; opencode session data is unchanged",
            main.display()
        )),
    }
}

/// v1 chain (D3): pure-jj repo → `'global'`; else the `.git/opencode` memo;
/// else the `project` row keyed by worktree; else fail-closed with guidance.
fn resolve_target_v1(conn: &Connection, main: &Path) -> Result<String, String> {
    // (0) No `.git` at all: a pure jj main repo. opencode resolves such a
    // directory to the built-in 'global' project, whose row always exists.
    if !main.join(".git").exists() {
        return Ok("global".to_string());
    }
    // (1) opencode's own memo file (`<commonDir>/opencode`), written by
    // Project.resolve/commit. Trust it only when the project row exists.
    let cached = main.join(".git").join("opencode");
    if let Ok(raw) = fs::read_to_string(&cached) {
        let id = raw.trim();
        if !id.is_empty() && project_exists(conn, id)? {
            return Ok(id.to_string());
        }
    }
    // (2) The project row opencode created for this worktree.
    let main_str = main.to_string_lossy();
    let by_worktree: Option<String> = conn
        .query_row(
            "SELECT id FROM project WHERE worktree = ?1",
            params![main_str.as_ref()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|err| {
            format!("cannot query the opencode project table (refusing to remove): {err}")
        })?;
    if let Some(id) = by_worktree {
        return Ok(id);
    }
    // (3) No authoritative answer: refuse rather than invent an id. The
    // first line is the toast-visible one, so it carries the action.
    Err(format!(
        "open opencode in the main repo once, then retry remove\n\
         could not resolve the main repo's opencode project id \
         (no .git/opencode cache entry, no project row for this worktree)\n\
         main repo: {}\n\
         the workspace was not removed; opencode session data is unchanged",
        main.display()
    ))
}

fn project_exists(conn: &Connection, id: &str) -> Result<bool, String> {
    conn.query_row("SELECT 1 FROM project WHERE id = ?1", params![id], |_| {
        Ok(())
    })
    .optional()
    .map(|row| row.is_some())
    .map_err(|err| format!("cannot query the opencode project table (refusing to remove): {err}"))
}

/// One immediate write transaction (D2): rewrite the matching session rows,
/// then commit. Any error rolls back and the caller refuses the removal;
/// nothing is half-written. `selected` restricts the session UPDATE to those
/// ids (`None` = every row in range, `Some(&[])` = nothing to do).
///
/// v1 also drops stale `project_directory` rows for the workspace range. v2
/// leaves `project` / `worktree` / deprecated `project_directory` rows alone:
/// opencode's own `Project.sweep` archives projects whose directory is gone,
/// and deleting a `project` row would cascade into `session_v2`.
fn run_migration_tx(
    conn: &mut Connection,
    store: SessionStore,
    ws: &Path,
    main: &Path,
    target: &str,
    selected: Option<&[String]>,
) -> Result<usize, String> {
    let ws = ws.to_string_lossy();
    let main = main.to_string_lossy();
    let table = store.table();
    let now_ms = unix_millis();
    // Placeholders: ?1 range root, ?2 target project, ?3 main repo root,
    // ?4 timestamp, ?5+ selected ids (bound in this order).
    let (sql, values) = match selected {
        Some(ids) if ids.is_empty() => return Ok(0),
        Some(ids) => {
            let placeholders = id_placeholders(5, ids.len());
            let mut values = vec![
                Value::Text(ws.as_ref().to_string()),
                Value::Text(target.to_string()),
                Value::Text(main.as_ref().to_string()),
                Value::Integer(now_ms),
            ];
            values.extend(ids.iter().cloned().map(Value::Text));
            (
                format!(
                    "UPDATE {table} SET project_id = ?2, directory = ?3, path = '', \
                     workspace_id = NULL, time_updated = ?4 \
                     WHERE ({WORKSPACE_RANGE}) AND id IN ({placeholders})"
                ),
                values,
            )
        }
        None => (
            format!(
                "UPDATE {table} SET project_id = ?2, directory = ?3, path = '', \
                 workspace_id = NULL, time_updated = ?4 WHERE {WORKSPACE_RANGE}"
            ),
            vec![
                Value::Text(ws.as_ref().to_string()),
                Value::Text(target.to_string()),
                Value::Text(main.as_ref().to_string()),
                Value::Integer(now_ms),
            ],
        ),
    };
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|err| {
            format!("cannot start the opencode migration transaction (refusing to remove): {err}")
        })?;
    let updated = tx
        .execute(&sql, params_from_iter(values))
        .map_err(|err| format!("cannot migrate opencode sessions (refusing to remove): {err}"))?;
    if !store.is_v2() && table_exists(&tx, "project_directory")? {
        tx.execute(
            &format!("DELETE FROM project_directory WHERE {WORKSPACE_RANGE}"),
            params![ws.as_ref()],
        )
        .map_err(|err| {
            format!("cannot clean the opencode project_directory table (refusing to remove): {err}")
        })?;
    }
    tx.commit().map_err(|err| {
        format!("cannot commit the opencode migration (refusing to remove): {err}")
    })?;
    Ok(updated)
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
        params![name],
        |_| Ok(()),
    )
    .optional()
    .map(|row| row.is_some())
    .map_err(schema_error)
}

fn unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Unique per-test temp dir, removed on drop (same pattern as main.rs).
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> TempDir {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock before epoch")
                .as_nanos();
            let seq = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "jj-workspace-migration-{}-{nanos}-{seq}",
                std::process::id()
            ));
            fs::create_dir_all(&dir).expect("create temp dir");
            TempDir(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Minimal opencode v1.18-shaped schema: the tables and columns the
    /// migration touches, including project_id → project.id ON DELETE CASCADE.
    fn fixture_schema(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE project (
                 id TEXT PRIMARY KEY,
                 worktree TEXT NOT NULL DEFAULT ''
             );
             CREATE TABLE session (
                 id TEXT PRIMARY KEY,
                 title TEXT NOT NULL DEFAULT '',
                 project_id TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
                 directory TEXT NOT NULL,
                 path TEXT NOT NULL DEFAULT '',
                 workspace_id TEXT,
                 time_updated INTEGER NOT NULL DEFAULT 0,
                 parent_id TEXT
             );
             CREATE TABLE project_directory (
                 directory TEXT PRIMARY KEY,
                 project_id TEXT NOT NULL
             );",
        )
        .expect("create fixture schema");
    }

    /// opencode v2.0-shaped fixture: the live `session_v2` table, plus the v1
    /// `session` history table the v1→v2 migration leaves behind, the
    /// deprecated `project_directory` table and the v2 `worktree` table — the
    /// last two exist so tests can prove v2 leaves them untouched.
    fn fixture_schema_v2(conn: &Connection) {
        fixture_schema(conn);
        conn.execute_batch(
            "CREATE TABLE session_v2 (
                 id TEXT PRIMARY KEY,
                 title TEXT NOT NULL DEFAULT '',
                 project_id TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
                 directory TEXT NOT NULL,
                 path TEXT NOT NULL DEFAULT '',
                 workspace_id TEXT,
                 time_updated INTEGER NOT NULL DEFAULT 0,
                 parent_id TEXT
             );
             CREATE TABLE worktree (
                 project_id TEXT NOT NULL,
                 directory TEXT NOT NULL,
                 PRIMARY KEY (project_id, directory)
             );",
        )
        .expect("create v2 fixture schema");
    }

    fn insert_project(conn: &Connection, id: &str, worktree: &str) {
        conn.execute(
            "INSERT INTO project (id, worktree) VALUES (?1, ?2)",
            params![id, worktree],
        )
        .expect("insert project");
    }

    fn insert_session(
        conn: &Connection,
        id: &str,
        project_id: &str,
        directory: &str,
        path: &str,
        workspace_id: Option<&str>,
    ) {
        insert_session_row(conn, id, "", project_id, directory, path, workspace_id, 0);
    }

    /// Session insert with an explicit title and timestamp (preview tests
    /// need distinguishable newest-first order).
    #[allow(clippy::too_many_arguments)]
    fn insert_session_row(
        conn: &Connection,
        id: &str,
        title: &str,
        project_id: &str,
        directory: &str,
        path: &str,
        workspace_id: Option<&str>,
        time_updated: i64,
    ) {
        conn.execute(
            "INSERT INTO session \
             (id, title, project_id, directory, path, workspace_id, time_updated) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                title,
                project_id,
                directory,
                path,
                workspace_id,
                time_updated
            ],
        )
        .expect("insert session");
    }

    /// Session insert into the v2 live table (sub-agent rows carry a parent).
    #[allow(clippy::too_many_arguments)]
    fn insert_session_v2(
        conn: &Connection,
        id: &str,
        title: &str,
        project_id: &str,
        directory: &str,
        path: &str,
        workspace_id: Option<&str>,
        parent_id: Option<&str>,
        time_updated: i64,
    ) {
        conn.execute(
            "INSERT INTO session_v2 \
             (id, title, project_id, directory, path, workspace_id, parent_id, time_updated) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id,
                title,
                project_id,
                directory,
                path,
                workspace_id,
                parent_id,
                time_updated
            ],
        )
        .expect("insert session_v2");
    }

    /// The five migration-relevant columns of one session row.
    fn migration_row(
        conn: &Connection,
        table: &str,
        id: &str,
    ) -> (String, String, String, Option<String>, i64) {
        conn.query_row(
            &format!(
                "SELECT project_id, directory, path, workspace_id, time_updated \
                 FROM {table} WHERE id = ?1"
            ),
            params![id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .expect("session row")
    }

    /// Every session column, ordered by id, for before/after comparisons.
    type Snapshot = Vec<(String, String, String, String, String, Option<String>, i64)>;

    fn session_snapshot(conn: &Connection, table: &str) -> Snapshot {
        conn.prepare(&format!(
            "SELECT id, title, project_id, directory, path, workspace_id, time_updated \
             FROM {table} ORDER BY id"
        ))
        .expect("prepare snapshot")
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
            ))
        })
        .expect("query snapshot")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect snapshot")
    }

    // --- 6.1 enumeration boundaries ----------------------------------------

    #[test]
    fn count_matches_root_and_subdirs_but_not_siblings() {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        fixture_schema(&conn);
        insert_project(&conn, "global", "");
        let ws = "/tmp/jj-workspace-enum/workspace-test-proj-id";
        insert_session(&conn, "root", "global", ws, "", None);
        insert_session(&conn, "sub", "global", &format!("{ws}/sub"), "", None);
        insert_session(&conn, "deep", "global", &format!("{ws}/a/b"), "", None);
        // Underscore sibling: a LIKE predicate would match this, the range
        // predicate must not.
        insert_session(
            &conn,
            "underscore",
            "global",
            "/tmp/jj-workspace-enum/workspace-test-proj_id",
            "",
            None,
        );
        // Longer-prefix sibling: `...proj-id-longer` must not match either.
        insert_session(&conn, "longer", "global", &format!("{ws}-longer"), "", None);
        assert_eq!(
            count_sessions(&conn, SessionStore::V1, Path::new(ws)).expect("count"),
            3
        );
    }

    // --- 6.2 target resolution chain ---------------------------------------

    #[test]
    fn target_resolution_prefers_pure_jj_then_cache_then_worktree() {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        fixture_schema(&conn);
        insert_project(&conn, "global", "");
        let dir = TempDir::new();

        // (0) No .git at all → the built-in global project.
        let pure_jj = dir.path().join("pure-jj");
        fs::create_dir_all(&pure_jj).expect("create main dir");
        assert_eq!(
            resolve_target(&conn, SessionStore::V1, &pure_jj).expect("global"),
            "global"
        );

        // (1) Cache file + matching project row wins.
        let cached = dir.path().join("cached");
        fs::create_dir_all(cached.join(".git")).expect("create .git");
        fs::write(cached.join(".git").join("opencode"), "cache-id\n").expect("write cache");
        insert_project(&conn, "cache-id", "/somewhere/else");
        assert_eq!(
            resolve_target(&conn, SessionStore::V1, &cached).expect("cache id"),
            "cache-id"
        );

        // (2) No usable cache → the project row keyed by worktree.
        let by_row = dir.path().join("by-row");
        fs::create_dir_all(by_row.join(".git")).expect("create .git");
        insert_project(&conn, "row-id", &by_row.to_string_lossy());
        assert_eq!(
            resolve_target(&conn, SessionStore::V1, &by_row).expect("row id"),
            "row-id"
        );

        // (3) Neither → fail-closed with actionable guidance.
        let missing = dir.path().join("missing");
        fs::create_dir_all(missing.join(".git")).expect("create .git");
        let err = resolve_target(&conn, SessionStore::V1, &missing).expect_err("must refuse");
        assert!(err.contains("open opencode in the main repo once"), "{err}");
        assert!(err.contains("retry remove"), "{err}");
        // Task 5.2: the toast only shows the first line (truncated at 120 by
        // die_toast_body), so the actionable guidance must fit up front.
        let first_line = err.lines().next().expect("first line");
        assert!(first_line.chars().count() <= 120, "{first_line}");
    }

    #[test]
    fn stale_cache_file_falls_back_to_worktree_row() {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        fixture_schema(&conn);
        insert_project(&conn, "global", "");
        let dir = TempDir::new();
        let main = dir.path().join("stale-cache");
        fs::create_dir_all(main.join(".git")).expect("create .git");
        fs::write(main.join(".git").join("opencode"), "not-a-project\n").expect("write cache");
        insert_project(&conn, "row-id", &main.to_string_lossy());
        assert_eq!(
            resolve_target(&conn, SessionStore::V1, &main).expect("row id"),
            "row-id"
        );
    }

    // --- 6.3 probe refusal / skip paths ------------------------------------

    #[test]
    fn schema_probe_refuses_missing_columns() {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        conn.execute_batch(
            "CREATE TABLE project (id TEXT PRIMARY KEY, worktree TEXT);
             CREATE TABLE session (
                 id TEXT PRIMARY KEY,
                 project_id TEXT,
                 directory TEXT,
                 path TEXT,
                 time_updated INTEGER
             );",
        )
        .expect("create schema");
        let err = probe_schema(&conn).expect_err("missing session.workspace_id must refuse");
        assert!(err.contains("session.workspace_id"), "{err}");
        assert!(err.contains("refusing to remove"), "{err}");

        let conn = Connection::open_in_memory().expect("open in-memory db");
        conn.execute_batch(
            "CREATE TABLE project (id TEXT PRIMARY KEY);
             CREATE TABLE session (
                 id TEXT PRIMARY KEY,
                 project_id TEXT,
                 directory TEXT,
                 path TEXT,
                 workspace_id TEXT,
                 time_updated INTEGER
             );",
        )
        .expect("create schema");
        let err = probe_schema(&conn).expect_err("missing project.worktree must refuse");
        assert!(err.contains("project.worktree"), "{err}");

        // v2: the live table is session_v2 and the refusal must name it.
        let conn = Connection::open_in_memory().expect("open in-memory db");
        conn.execute_batch(
            "CREATE TABLE project (id TEXT PRIMARY KEY, worktree TEXT);
             CREATE TABLE session_v2 (
                 id TEXT PRIMARY KEY,
                 project_id TEXT,
                 directory TEXT,
                 path TEXT,
                 time_updated INTEGER
             );",
        )
        .expect("create v2 schema");
        let err = probe_schema(&conn).expect_err("missing session_v2.workspace_id must refuse");
        assert!(err.contains("session_v2.workspace_id"), "{err}");
        assert!(err.contains("refusing to remove"), "{err}");

        // Neither table: refuse, never treat it as an empty match set.
        let conn = Connection::open_in_memory().expect("open in-memory db");
        conn.execute_batch("CREATE TABLE project (id TEXT PRIMARY KEY, worktree TEXT);")
            .expect("create project-only schema");
        let err = probe_schema(&conn).expect_err("no session table must refuse");
        assert!(err.contains("neither session_v2 nor session"), "{err}");
        assert!(err.contains("refusing to remove"), "{err}");
    }

    #[test]
    fn no_matching_sessions_is_a_skip() {
        let dir = TempDir::new();
        let db_path = dir.path().join("opencode.db");
        let conn = Connection::open(&db_path).expect("create db");
        fixture_schema(&conn);
        insert_project(&conn, "global", "");
        insert_session(&conn, "other", "global", "/somewhere/else", "", None);
        drop(conn);
        // The only session is out of range, so nothing in the selection
        // matches and the target must not even be resolved.
        let outcome = migrate_selected_db(
            &db_path,
            &dir.path().join("workspace"),
            &dir.path().join("main"),
            &["other".to_string()],
        )
        .expect("no error");
        assert!(outcome.is_none(), "count 0 must skip");
    }

    #[test]
    fn missing_opencode_binary_is_a_skip() {
        assert!(find_opencode_in(&[]).is_none());
        let dir = TempDir::new();
        // A non-executable file named opencode is not a usable binary.
        fs::write(dir.path().join("opencode"), "not a binary").expect("write decoy");
        assert!(find_opencode_in(&[dir.path().to_path_buf()]).is_none());
    }

    #[test]
    fn db_path_discovery_skips_empty_and_missing_outputs() {
        assert!(db_path_from_output(b"").is_none());
        assert!(db_path_from_output(b"  \n").is_none());
        assert!(db_path_from_output(b"/no/such/opencode.db\n").is_none());
        let dir = TempDir::new();
        let db = dir.path().join("opencode.db");
        fs::write(&db, b"").expect("create db file");
        assert_eq!(
            db_path_from_output(format!("{}\n", db.display()).as_bytes()),
            Some(db)
        );
    }

    // --- 6.3b PATH-independent discovery (D12) -----------------------------

    /// Executable stub, used to fake the `opencode` CLI.
    fn write_executable(path: &Path, script: &str) {
        fs::write(path, script).expect("write stub");
        let mut perms = fs::metadata(path).expect("stat stub").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod stub");
    }

    #[test]
    fn search_dirs_put_path_first_and_dedupe_fallbacks() {
        let home = PathBuf::from("/home/tester");
        let on_path = home.join(".local").join("bin");
        let dirs = opencode_search_dirs(&[PathBuf::from("/usr/bin"), on_path.clone()], Some(&home));
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/usr/bin"),
                on_path,
                home.join(".opencode").join("bin"),
                PathBuf::from("/usr/local/bin"),
            ]
        );
        // HOME unset: PATH dirs plus the HOME-independent fallback only.
        assert_eq!(
            opencode_search_dirs(&[PathBuf::from("/usr/bin")], None),
            vec![PathBuf::from("/usr/bin"), PathBuf::from("/usr/local/bin")]
        );
    }

    #[test]
    fn find_opencode_prefers_path_dirs_over_fallback_dirs() {
        let home = TempDir::new();
        let path_dir = TempDir::new();
        let fallback = home.path().join(".opencode").join("bin");
        fs::create_dir_all(&fallback).expect("create fallback dir");
        write_executable(&path_dir.path().join("opencode"), "#!/bin/sh\nexit 0\n");
        write_executable(&fallback.join("opencode"), "#!/bin/sh\nexit 0\n");
        let dirs = opencode_search_dirs(&[path_dir.path().to_path_buf()], Some(home.path()));
        assert_eq!(
            find_opencode_in(&dirs),
            Some(path_dir.path().join("opencode"))
        );
        // Without the PATH dir, the well-known fallback dir takes over.
        let dirs = opencode_search_dirs(&[], Some(home.path()));
        assert_eq!(find_opencode_in(&dirs), Some(fallback.join("opencode")));
    }

    #[test]
    fn discovery_falls_back_to_standard_locations_when_cli_is_absent() {
        let home = TempDir::new();
        let xdg = TempDir::new();
        let xdg_db = xdg.path().join("opencode").join("opencode.db");
        let home_db = home
            .path()
            .join(".local")
            .join("share")
            .join("opencode")
            .join("opencode.db");
        fs::create_dir_all(xdg_db.parent().expect("xdg parent")).expect("create xdg dir");
        fs::create_dir_all(home_db.parent().expect("home parent")).expect("create home dir");
        fs::write(&xdg_db, b"").expect("create xdg db");
        fs::write(&home_db, b"").expect("create home db");

        // XDG_DATA_HOME set: only that location is consulted.
        let candidates = db_candidates(Some(home.path()), Some(xdg.path()));
        assert_eq!(candidates, vec![xdg_db.clone()]);
        assert_eq!(discover_db_path(None, &candidates), Some(xdg_db.clone()));

        // XDG_DATA_HOME unset: the HOME fallback is used.
        let candidates = db_candidates(Some(home.path()), None);
        assert_eq!(candidates, vec![home_db.clone()]);
        assert_eq!(discover_db_path(None, &candidates), Some(home_db));
    }

    #[test]
    fn discovery_falls_back_when_cli_is_unusable() {
        let dir = TempDir::new();
        let fallback_db = dir.path().join("fallback.db");
        fs::write(&fallback_db, b"").expect("create fallback db");
        let candidates = vec![fallback_db.clone()];

        // Non-zero exit status.
        let failing = dir.path().join("opencode-fail");
        write_executable(&failing, "#!/bin/sh\nexit 1\n");
        assert_eq!(
            discover_db_path(Some(&failing), &candidates),
            Some(fallback_db.clone())
        );

        // Output that does not name an existing file.
        let bogus = dir.path().join("opencode-bogus");
        write_executable(&bogus, "#!/bin/sh\necho /no/such/opencode.db\n");
        assert_eq!(
            discover_db_path(Some(&bogus), &candidates),
            Some(fallback_db)
        );
    }

    #[test]
    fn discovery_prefers_cli_reported_db_over_standard_locations() {
        let dir = TempDir::new();
        let cli_db = dir.path().join("cli.db");
        let candidate_db = dir.path().join("candidate.db");
        fs::write(&cli_db, b"").expect("create cli db");
        fs::write(&candidate_db, b"").expect("create candidate db");
        let cli = dir.path().join("opencode-ok");
        write_executable(&cli, &format!("#!/bin/sh\necho {}\n", cli_db.display()));
        assert_eq!(discover_db_path(Some(&cli), &[candidate_db]), Some(cli_db));
    }

    #[test]
    fn discovery_returns_none_when_no_location_exists() {
        let dir = TempDir::new();
        let missing = dir.path().join("missing").join("opencode.db");
        assert_eq!(discover_db_path(None, &[missing]), None);
        assert_eq!(discover_db_path(None, &[]), None);
    }

    #[test]
    fn skip_reason_names_tried_locations() {
        let home = PathBuf::from("/home/tester");
        let path_dirs = vec![PathBuf::from("/usr/bin")];
        let candidates = db_candidates(Some(&home), None);
        assert_eq!(
            discovery_skip_reason(&path_dirs, false, &candidates, Some(&home)),
            "opencode not found (searched PATH + ~/.opencode/bin, ~/.local/bin, /usr/local/bin) \
             and no opencode DB at ~/.local/share/opencode/opencode.db"
        );
        // A found CLI whose probe failed still names the DB locations tried.
        let reason = discovery_skip_reason(&path_dirs, true, &candidates, Some(&home));
        assert!(
            reason.contains("~/.local/share/opencode/opencode.db"),
            "{reason}"
        );
        // HOME unset: the reason degrades to the locations that remain.
        let reason = discovery_skip_reason(&[], false, &[], None);
        assert!(reason.contains("/usr/local/bin"), "{reason}");
        assert!(
            reason.contains("HOME and XDG_DATA_HOME are unset"),
            "{reason}"
        );
    }

    // --- 6.4 fixture-DB integration ----------------------------------------

    #[test]
    fn migration_updates_rows_and_cleans_project_directory() {
        let dir = TempDir::new();
        let db_path = dir.path().join("opencode.db");
        let ws = "/home/user/Workspace/main-repo/workspace-feature";
        // `main` must exist on disk with a `.git` dir: resolve_target step
        // (0) treats a directory without `.git` as a pure-jj repo (global).
        let main = dir.path().join("main-repo");
        fs::create_dir_all(main.join(".git")).expect("create main .git");
        let main = main.to_string_lossy().into_owned();
        let sibling = "/home/user/Workspace/main-repo/workspace-feature_id";
        let conn = Connection::open(&db_path).expect("create db");
        fixture_schema(&conn);
        insert_project(&conn, "global", "");
        insert_project(&conn, "target-id", &main);
        insert_session(&conn, "root", "global", ws, "", None);
        insert_session(
            &conn,
            "sub",
            "global",
            &format!("{ws}/pkg"),
            "pkg",
            Some("wrk_old"),
        );
        insert_session(&conn, "sibling", "global", sibling, "", None);
        conn.execute(
            "INSERT INTO project_directory (directory, project_id) VALUES (?1, 'global'), (?2, 'global'), (?3, 'global')",
            params![ws, format!("{ws}/pkg"), sibling],
        )
        .expect("seed project_directory");
        drop(conn);

        let before = unix_millis();
        let selected = vec!["root".to_string(), "sub".to_string()];
        let migrated = migrate_selected_db(&db_path, Path::new(ws), Path::new(&main), &selected)
            .expect("migration succeeds")
            .expect("sessions matched");
        assert_eq!(migrated, 2);

        let conn = Connection::open(&db_path).expect("reopen db");
        let row = |id: &str| {
            conn.query_row(
                "SELECT project_id, directory, path, workspace_id, time_updated \
                 FROM session WHERE id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .expect("session row")
        };
        let (project_id, directory, path, workspace_id, time_updated) = row("root");
        assert_eq!(project_id, "target-id");
        assert_eq!(directory, main);
        assert_eq!(path, "");
        assert_eq!(workspace_id, None);
        assert!(time_updated >= before, "{time_updated} < {before}");
        let (project_id, directory, path, workspace_id, _) = row("sub");
        assert_eq!(project_id, "target-id");
        assert_eq!(directory, main);
        assert_eq!(path, "");
        assert_eq!(workspace_id, None);
        // The underscore sibling stays untouched.
        let (project_id, directory, ..) = row("sibling");
        assert_eq!(project_id, "global");
        assert_eq!(directory, sibling);

        let remaining: Vec<String> = conn
            .prepare("SELECT directory FROM project_directory ORDER BY directory")
            .expect("prepare")
            .query_map([], |row| row.get(0))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("collect");
        assert_eq!(remaining, vec![sibling.to_string()]);
    }

    #[test]
    fn foreign_key_rejection_rolls_back_without_partial_writes() {
        let dir = TempDir::new();
        let db_path = dir.path().join("opencode.db");
        let ws = "/home/user/Workspace/main-repo/workspace-feature";
        let main = "/home/user/Sources/main-repo";
        let conn = Connection::open(&db_path).expect("create db");
        fixture_schema(&conn);
        insert_project(&conn, "global", "");
        insert_session(&conn, "root", "global", ws, "", None);
        conn.execute(
            "INSERT INTO project_directory (directory, project_id) VALUES (?1, 'global')",
            params![ws],
        )
        .expect("seed project_directory");
        // foreign_keys=ON exactly like the real migration path.
        conn.execute_batch("PRAGMA foreign_keys=ON")
            .expect("pragma");
        let mut conn = conn;
        let err = run_migration_tx(
            &mut conn,
            SessionStore::V1,
            Path::new(ws),
            Path::new(main),
            "no-such-project",
            None,
        )
        .expect_err("foreign key must reject an unknown project id");
        assert!(err.contains("cannot migrate opencode sessions"), "{err}");

        let (project_id, directory): (String, String) = conn
            .query_row(
                "SELECT project_id, directory FROM session WHERE id = 'root'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("session row");
        assert_eq!(project_id, "global");
        assert_eq!(directory, ws);
        let directories: i64 = conn
            .query_row("SELECT COUNT(*) FROM project_directory", [], |row| {
                row.get(0)
            })
            .expect("count");
        assert_eq!(directories, 1);
    }

    #[test]
    fn failure_after_session_update_rolls_back_everything() {
        let dir = TempDir::new();
        let db_path = dir.path().join("opencode.db");
        let ws = "/home/user/Workspace/main-repo/workspace-feature";
        let main = "/home/user/Sources/main-repo";
        let conn = Connection::open(&db_path).expect("create db");
        fixture_schema(&conn);
        insert_project(&conn, "global", "");
        insert_project(&conn, "target-id", main);
        insert_session(&conn, "root", "global", ws, "", None);
        conn.execute(
            "INSERT INTO project_directory (directory, project_id) VALUES (?1, 'global')",
            params![ws],
        )
        .expect("seed project_directory");
        // Make the cleanup fail after the session UPDATE already ran: the
        // whole transaction must roll back, leaving the UPDATE uncommitted.
        conn.execute_batch(
            "CREATE TRIGGER block_cleanup BEFORE DELETE ON project_directory
             BEGIN SELECT RAISE(ABORT, 'blocked'); END;",
        )
        .expect("create trigger");
        let mut conn = conn;
        let err = run_migration_tx(
            &mut conn,
            SessionStore::V1,
            Path::new(ws),
            Path::new(main),
            "target-id",
            None,
        )
        .expect_err("cleanup failure must refuse");
        assert!(err.contains("project_directory"), "{err}");

        let directory: String = conn
            .query_row(
                "SELECT directory FROM session WHERE id = 'root'",
                [],
                |row| row.get(0),
            )
            .expect("session row");
        assert_eq!(directory, ws, "the session UPDATE must roll back too");
    }

    // --- 6.5 inspect preview + subset migration ----------------------------

    /// Whole-DB content snapshot (fixture tables only) for write-freedom
    /// checks.
    /// Deprecated v1 mapping rows, ordered by directory.
    fn project_directory_snapshot(conn: &Connection) -> Vec<(String, String)> {
        conn.prepare("SELECT directory, project_id FROM project_directory ORDER BY directory")
            .expect("prepare project_directory")
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query project_directory")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect project_directory")
    }

    fn db_snapshot(conn: &Connection, table: &str) -> (Snapshot, Vec<(String, String)>) {
        (
            session_snapshot(conn, table),
            project_directory_snapshot(conn),
        )
    }

    #[test]
    fn inspect_lists_rows_newest_first_with_fields() {
        let dir = TempDir::new();
        let db_path = dir.path().join("opencode.db");
        let ws = "/home/user/Workspace/main-repo/workspace-feature";
        let main_path = dir.path().join("main-repo");
        fs::create_dir_all(main_path.join(".git")).expect("create main .git");
        let main = main_path.to_string_lossy().into_owned();
        let conn = Connection::open(&db_path).expect("create db");
        fixture_schema(&conn);
        insert_project(&conn, "global", "");
        insert_project(&conn, "target-id", &main);
        insert_session_row(&conn, "older", "Older session", "global", ws, "", None, 10);
        insert_session_row(&conn, "tie-b", "Tie B", "global", ws, "", None, 15);
        insert_session_row(&conn, "tie-a", "Tie A", "global", ws, "", None, 15);
        insert_session_row(
            &conn,
            "newer",
            "Newer session",
            "global",
            &format!("{ws}/pkg"),
            "",
            Some("wrk_old"),
            20,
        );
        insert_session_row(
            &conn,
            "elsewhere",
            "Elsewhere",
            "global",
            "/somewhere/else",
            "",
            None,
            30,
        );
        drop(conn);

        let rows = inspect_db(&db_path, Path::new(ws), &main_path)
            .expect("inspect succeeds")
            .expect("matching rows");
        assert_eq!(rows.len(), 4, "out-of-range rows must not match");
        assert_eq!(rows[0].id, "newer");
        assert_eq!(rows[0].title, "Newer session");
        assert_eq!(rows[0].directory, format!("{ws}/pkg"));
        assert_eq!(rows[0].time_updated, 20);
        assert_eq!(rows[1].id, "tie-a", "ties break by ascending id");
        assert_eq!(rows[2].id, "tie-b");
        assert_eq!(rows[3].id, "older");
        assert_eq!(rows[3].title, "Older session");
        assert_eq!(rows[3].directory, ws);
        assert_eq!(rows[3].time_updated, 10);
    }

    #[test]
    fn inspect_skips_empty_match_before_target_resolution() {
        let dir = TempDir::new();
        let db_path = dir.path().join("opencode.db");
        let conn = Connection::open(&db_path).expect("create db");
        fixture_schema(&conn);
        insert_project(&conn, "global", "");
        insert_session(&conn, "other", "global", "/somewhere/else", "", None);
        drop(conn);
        // `.git` with no cache or project row would make `resolve_target`
        // refuse, but an empty match set skips first — same order as the
        // migration, so the preview cannot block on a no-op removal.
        let main_path = dir.path().join("main-repo");
        fs::create_dir_all(main_path.join(".git")).expect("create main .git");
        let outcome = inspect_db(&db_path, &dir.path().join("workspace"), &main_path)
            .expect("inspect succeeds");
        assert!(
            outcome.is_none(),
            "zero rows must skip even when the target is unresolvable"
        );
    }

    #[test]
    fn inspect_refuses_on_schema_probe_failure() {
        let dir = TempDir::new();
        let db_path = dir.path().join("opencode.db");
        let conn = Connection::open(&db_path).expect("create db");
        conn.execute_batch(
            "CREATE TABLE project (id TEXT PRIMARY KEY, worktree TEXT);
             CREATE TABLE session (
                 id TEXT PRIMARY KEY,
                 title TEXT,
                 project_id TEXT,
                 directory TEXT,
                 path TEXT,
                 time_updated INTEGER
             );",
        )
        .expect("create schema");
        drop(conn);

        let err = inspect_db(
            &db_path,
            Path::new("/tmp/jj-workspace-inspect/ws"),
            &dir.path().join("main-repo"),
        )
        .expect_err("missing session.workspace_id must refuse");
        assert!(err.contains("session.workspace_id"), "{err}");
        assert!(err.contains("refusing to remove"), "{err}");
    }

    #[test]
    fn inspect_refuses_when_db_cannot_be_opened() {
        let dir = TempDir::new();
        let err = inspect_db(
            dir.path(),
            Path::new("/tmp/jj-workspace-inspect/ws"),
            Path::new("/tmp/jj-workspace-inspect/main"),
        )
        .expect_err("opening a directory as a DB must refuse");
        assert!(
            err.contains("cannot open the opencode DB (refusing to remove)"),
            "{err}"
        );
    }

    #[test]
    fn inspect_does_not_modify_rows() {
        let dir = TempDir::new();
        let db_path = dir.path().join("opencode.db");
        let ws = "/home/user/Workspace/main-repo/workspace-feature";
        let main_path = dir.path().join("main-repo");
        fs::create_dir_all(main_path.join(".git")).expect("create main .git");
        let main = main_path.to_string_lossy().into_owned();
        let conn = Connection::open(&db_path).expect("create db");
        fixture_schema(&conn);
        insert_project(&conn, "global", "");
        insert_project(&conn, "target-id", &main);
        insert_session_row(&conn, "root", "Root", "global", ws, "", None, 5);
        insert_session_row(
            &conn,
            "sub",
            "Sub",
            "global",
            &format!("{ws}/sub"),
            "sub",
            Some("wrk_old"),
            7,
        );
        conn.execute(
            "INSERT INTO project_directory (directory, project_id) VALUES (?1, 'global'), (?2, 'global')",
            params![ws, format!("{ws}/sub")],
        )
        .expect("seed project_directory");
        let before = db_snapshot(&conn, "session");
        drop(conn);

        let rows = inspect_db(&db_path, Path::new(ws), &main_path)
            .expect("inspect succeeds")
            .expect("matching rows");
        assert_eq!(rows.len(), 2);

        let conn = Connection::open(&db_path).expect("reopen db");
        assert_eq!(
            db_snapshot(&conn, "session"),
            before,
            "preview must not write"
        );
    }

    #[test]
    fn inspect_substitutes_empty_title_when_column_is_missing() {
        let dir = TempDir::new();
        let db_path = dir.path().join("opencode.db");
        let ws = "/home/user/Workspace/main-repo/workspace-feature";
        let main_path = dir.path().join("main-repo");
        fs::create_dir_all(main_path.join(".git")).expect("create main .git");
        let main = main_path.to_string_lossy().into_owned();
        let conn = Connection::open(&db_path).expect("create db");
        // Pre-title opencode schema: probe passes, the preview still answers.
        conn.execute_batch(
            "CREATE TABLE project (
                 id TEXT PRIMARY KEY,
                 worktree TEXT NOT NULL DEFAULT ''
             );
             CREATE TABLE session (
                 id TEXT PRIMARY KEY,
                 project_id TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
                 directory TEXT NOT NULL,
                 path TEXT NOT NULL DEFAULT '',
                 workspace_id TEXT,
                 time_updated INTEGER NOT NULL DEFAULT 0
             );",
        )
        .expect("create schema");
        insert_project(&conn, "global", "");
        insert_project(&conn, "target-id", &main);
        conn.execute(
            "INSERT INTO session (id, project_id, directory, path, workspace_id, time_updated) \
             VALUES ('root', 'global', ?1, '', NULL, 3)",
            params![ws],
        )
        .expect("insert session");
        drop(conn);

        let rows = inspect_db(&db_path, Path::new(ws), &main_path)
            .expect("inspect succeeds")
            .expect("matching rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "root");
        assert_eq!(
            rows[0].title, "",
            "a missing title column must select an empty string"
        );
    }

    #[test]
    fn inspection_from_db_builds_the_public_variants() {
        let rows = vec![SessionRow {
            id: "s1".into(),
            title: "T".into(),
            directory: "/ws".into(),
            time_updated: 7,
        }];
        match inspection_from_db(Ok(Some(rows)), Path::new("/main/repo")) {
            Inspection::Ready { rows, main_repo } => {
                assert_eq!(main_repo, PathBuf::from("/main/repo"));
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].id, "s1");
                assert_eq!(rows[0].title, "T");
                assert_eq!(rows[0].directory, "/ws");
                assert_eq!(rows[0].time_updated, 7);
            }
            other => panic!("expected Ready, got {other:?}"),
        }
        match inspection_from_db(Ok(None), Path::new("/main/repo")) {
            Inspection::Skipped(reason) => {
                assert_eq!(reason, "no opencode sessions bound to this workspace")
            }
            other => panic!("expected Skipped, got {other:?}"),
        }
        match inspection_from_db(Err("boom".into()), Path::new("/main/repo")) {
            Inspection::Refused(reason) => assert_eq!(reason, "boom"),
            other => panic!("expected Refused, got {other:?}"),
        }
    }

    #[test]
    fn selected_subset_migration_updates_only_selected_rows() {
        let dir = TempDir::new();
        let db_path = dir.path().join("opencode.db");
        let ws = "/home/user/Workspace/main-repo/workspace-feature";
        let main_path = dir.path().join("main-repo");
        fs::create_dir_all(main_path.join(".git")).expect("create main .git");
        let main = main_path.to_string_lossy().into_owned();
        let conn = Connection::open(&db_path).expect("create db");
        fixture_schema(&conn);
        insert_project(&conn, "global", "");
        insert_project(&conn, "target-id", &main);
        insert_session(&conn, "chosen-a", "global", ws, "", None);
        insert_session(
            &conn,
            "chosen-b",
            "global",
            &format!("{ws}/pkg"),
            "pkg",
            Some("wrk_old"),
        );
        insert_session(
            &conn,
            "left-out",
            "global",
            &format!("{ws}/other"),
            "",
            None,
        );
        insert_session(
            &conn,
            "sibling",
            "global",
            &format!("{ws}_sibling"),
            "",
            None,
        );
        conn.execute(
            "INSERT INTO project_directory (directory, project_id) VALUES (?1, 'global'), (?2, 'global'), (?3, 'global'), (?4, 'global')",
            params![ws, format!("{ws}/pkg"), format!("{ws}/other"), format!("{ws}_sibling")],
        )
        .expect("seed project_directory");
        drop(conn);

        let before = unix_millis();
        let selected = vec!["chosen-a".to_string(), "chosen-b".to_string()];
        let migrated = migrate_selected_db(&db_path, Path::new(ws), &main_path, &selected)
            .expect("migration succeeds")
            .expect("selected rows matched");
        assert_eq!(migrated, 2);

        let conn = Connection::open(&db_path).expect("reopen db");
        let (project_id, directory, path, workspace_id, time_updated) =
            migration_row(&conn, "session", "chosen-a");
        assert_eq!(project_id, "target-id");
        assert_eq!(directory, main);
        assert_eq!(path, "");
        assert_eq!(workspace_id, None);
        assert!(time_updated >= before, "{time_updated} < {before}");
        let (project_id, directory, path, workspace_id, _) =
            migration_row(&conn, "session", "chosen-b");
        assert_eq!(project_id, "target-id");
        assert_eq!(directory, main);
        assert_eq!(path, "");
        assert_eq!(workspace_id, None);
        // Unselected rows keep every column.
        let (project_id, directory, path, workspace_id, time_updated) =
            migration_row(&conn, "session", "left-out");
        assert_eq!(project_id, "global");
        assert_eq!(directory, format!("{ws}/other"));
        assert_eq!(path, "");
        assert_eq!(workspace_id, None);
        assert_eq!(time_updated, 0);
        let (project_id, directory, ..) = migration_row(&conn, "session", "sibling");
        assert_eq!(project_id, "global");
        assert_eq!(directory, format!("{ws}_sibling"));

        // project_directory cleanup still covers the whole workspace range.
        let remaining: Vec<String> = conn
            .prepare("SELECT directory FROM project_directory ORDER BY directory")
            .expect("prepare")
            .query_map([], |row| row.get(0))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("collect");
        assert_eq!(remaining, vec![format!("{ws}_sibling")]);
    }

    #[test]
    fn empty_selection_skips_without_touching_the_db() {
        let dir = TempDir::new();
        let db_path = dir.path().join("opencode.db");
        let ws = "/home/user/Workspace/main-repo/workspace-feature";
        let main_path = dir.path().join("main-repo");
        fs::create_dir_all(main_path.join(".git")).expect("create main .git");
        let main = main_path.to_string_lossy().into_owned();
        let conn = Connection::open(&db_path).expect("create db");
        fixture_schema(&conn);
        insert_project(&conn, "global", "");
        insert_project(&conn, "target-id", &main);
        insert_session(&conn, "root", "global", ws, "", None);
        let before = db_snapshot(&conn, "session");
        drop(conn);

        // The empty selection short-circuits before PATH discovery, so this
        // cannot depend on a hosted opencode install either.
        let outcome = migrate_selected_opencode_sessions(Path::new(ws), &main_path, &[]);
        match outcome {
            Outcome::Skipped(reason) => assert_eq!(reason, "no sessions selected"),
            other => panic!("expected Skipped, got {other:?}"),
        }

        let conn = Connection::open(&db_path).expect("reopen db");
        assert_eq!(db_snapshot(&conn, "session"), before, "skip must not write");
    }

    #[test]
    fn selection_drift_skips_without_failing() {
        let dir = TempDir::new();
        let db_path = dir.path().join("opencode.db");
        let ws = "/home/user/Workspace/main-repo/workspace-feature";
        let main_path = dir.path().join("main-repo");
        fs::create_dir_all(main_path.join(".git")).expect("create main .git");
        let main = main_path.to_string_lossy().into_owned();
        let conn = Connection::open(&db_path).expect("create db");
        fixture_schema(&conn);
        insert_project(&conn, "global", "");
        insert_project(&conn, "target-id", &main);
        insert_session(&conn, "still-here", "global", ws, "", None);
        let before = db_snapshot(&conn, "session");
        drop(conn);

        // The preview listed ids that are gone (or out of range) by the time
        // the dialog executes: skip, never fail.
        let selected = vec!["gone-1".to_string(), "elsewhere".to_string()];
        let outcome = migrate_selected_db(&db_path, Path::new(ws), &main_path, &selected)
            .expect("drift is not a failure");
        assert!(outcome.is_none(), "no selected rows left must skip");

        let conn = Connection::open(&db_path).expect("reopen db");
        assert_eq!(db_snapshot(&conn, "session"), before);
    }

    #[test]
    fn selected_outcome_from_db_reports_counts_and_selection_drift() {
        match selected_outcome_from_db(Ok(Some(3)), Path::new("/main/repo")) {
            Outcome::Migrated { count, main_repo } => {
                assert_eq!(count, 3);
                assert_eq!(main_repo, PathBuf::from("/main/repo"));
            }
            other => panic!("expected Migrated, got {other:?}"),
        }
        match selected_outcome_from_db(Ok(None), Path::new("/main/repo")) {
            Outcome::Skipped(reason) => {
                assert_eq!(reason, "no opencode sessions matched the selection")
            }
            other => panic!("expected Skipped, got {other:?}"),
        }
        match selected_outcome_from_db(Err("boom".into()), Path::new("/main/repo")) {
            Outcome::Refused(reason) => assert_eq!(reason, "boom"),
            other => panic!("expected Refused, got {other:?}"),
        }
    }

    // --- 6.7 main repo root resolution -------------------------------------

    #[test]
    fn resolve_main_repo_follows_pointer_and_rejects_fallbacks() {
        let dir = TempDir::new();
        let main = dir.path().join("main-repo");
        fs::create_dir_all(main.join(".jj").join("repo")).expect("create main store");
        let ws = dir.path().join("workspace-feature");
        fs::create_dir_all(ws.join(".jj")).expect("create ws .jj");
        // The pointer is relative to `.jj/` and ends at the main store.
        fs::write(ws.join(".jj").join("repo"), "../../main-repo/.jj/repo").expect("write pointer");
        let resolved = resolve_main_repo(&ws).expect("secondary workspace resolves");
        assert_eq!(resolved, fs::canonicalize(&main).expect("canonical main"));

        // Missing pointer → repo_root falls back → refuse.
        let orphan = dir.path().join("orphan");
        fs::create_dir_all(orphan.join(".jj")).expect("create .jj");
        assert!(resolve_main_repo(&orphan).is_none());

        // Pointer into nowhere → canonicalize failure → refuse.
        let dangling = dir.path().join("dangling");
        fs::create_dir_all(dangling.join(".jj")).expect("create .jj");
        fs::write(dangling.join(".jj").join("repo"), "../../nope/.jj/repo").expect("write pointer");
        assert!(resolve_main_repo(&dangling).is_none());

        // Pointer resolving back to the workspace itself → refuse.
        let self_ptr = dir.path().join("self");
        fs::create_dir_all(self_ptr.join(".jj")).expect("create .jj");
        fs::write(self_ptr.join(".jj").join("repo"), "../self/.jj/repo").expect("write pointer");
        assert!(resolve_main_repo(&self_ptr).is_none());
    }

    // --- v2 live-table migration (opencode-v2-session-migration) ------------

    #[test]
    fn session_store_selection_prefers_v2_then_v1_and_refuses_neither() {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        conn.execute_batch(
            "CREATE TABLE project (id TEXT PRIMARY KEY, worktree TEXT);
             CREATE TABLE session (
                 id TEXT PRIMARY KEY,
                 project_id TEXT,
                 directory TEXT,
                 path TEXT,
                 workspace_id TEXT,
                 time_updated INTEGER
             );",
        )
        .expect("create v1 schema");
        assert_eq!(
            select_session_store(&conn).expect("v1 only"),
            SessionStore::V1
        );

        conn.execute_batch(
            "CREATE TABLE session_v2 (
                 id TEXT PRIMARY KEY,
                 project_id TEXT,
                 directory TEXT,
                 path TEXT,
                 workspace_id TEXT,
                 time_updated INTEGER
             );",
        )
        .expect("create v2 table");
        assert_eq!(
            select_session_store(&conn).expect("both tables prefer v2"),
            SessionStore::V2
        );

        conn.execute_batch("DROP TABLE session;")
            .expect("drop v1 table");
        assert_eq!(
            select_session_store(&conn).expect("v2 only"),
            SessionStore::V2
        );

        conn.execute_batch("DROP TABLE session_v2;")
            .expect("drop v2 table");
        let err = select_session_store(&conn).expect_err("neither table must refuse");
        assert!(err.contains("neither session_v2 nor session"), "{err}");
        assert!(err.contains("refusing to remove"), "{err}");
    }

    #[test]
    fn v2_migration_updates_session_v2_only_and_leaves_legacy_rows_alone() {
        let dir = TempDir::new();
        let db_path = dir.path().join("opencode.db");
        let ws = "/home/user/Workspace/main-repo/workspace-feature";
        let main_path = dir.path().join("main-repo");
        fs::create_dir_all(main_path.join(".git")).expect("create main .git");
        let main = main_path.to_string_lossy().into_owned();
        let conn = Connection::open(&db_path).expect("create db");
        fixture_schema_v2(&conn);
        insert_project(&conn, "ws-proj", ws);
        insert_project(&conn, "target-id", &main);
        // v2 live rows: root, a sub-agent row under a subdirectory, a sibling.
        insert_session_v2(&conn, "root", "Root", "ws-proj", ws, "", None, None, 5);
        insert_session_v2(
            &conn,
            "sub",
            "Sub agent",
            "ws-proj",
            &format!("{ws}/pkg"),
            "pkg",
            Some("wrk_old"),
            Some("root"),
            7,
        );
        insert_session_v2(
            &conn,
            "sibling",
            "Sibling",
            "ws-proj",
            &format!("{ws}_sibling"),
            "",
            None,
            None,
            9,
        );
        // v1 history rows for the same workspace: v2 must never touch them.
        insert_session_row(
            &conn,
            "legacy-root",
            "Legacy root",
            "ws-proj",
            ws,
            "",
            None,
            3,
        );
        insert_session_row(
            &conn,
            "legacy-sub",
            "Legacy sub",
            "ws-proj",
            &format!("{ws}/pkg"),
            "pkg",
            None,
            4,
        );
        conn.execute(
            "INSERT INTO project_directory (directory, project_id) \
             VALUES (?1, 'ws-proj'), (?2, 'ws-proj'), (?3, 'ws-proj')",
            params![ws, format!("{ws}/pkg"), format!("{ws}_sibling")],
        )
        .expect("seed project_directory");
        conn.execute(
            "INSERT INTO worktree (project_id, directory) VALUES ('ws-proj', ?1)",
            params![ws],
        )
        .expect("seed worktree");
        let legacy_before = session_snapshot(&conn, "session");
        let project_dirs_before = project_directory_snapshot(&conn);
        drop(conn);

        let before = unix_millis();
        let selected = vec![
            "root".to_string(),
            "sub".to_string(),
            "legacy-root".to_string(),
        ];
        let migrated = migrate_selected_db(&db_path, Path::new(ws), &main_path, &selected)
            .expect("migration succeeds")
            .expect("v2 rows matched");
        assert_eq!(migrated, 2, "only session_v2 rows count");

        let conn = Connection::open(&db_path).expect("reopen db");
        let (project_id, directory, path, workspace_id, time_updated) =
            migration_row(&conn, "session_v2", "root");
        assert_eq!(project_id, "target-id");
        assert_eq!(directory, main);
        assert_eq!(path, "");
        assert_eq!(workspace_id, None);
        assert!(time_updated >= before, "{time_updated} < {before}");

        // The sub-agent row (parent_id non-null) migrated too.
        let (project_id, directory, path, workspace_id, _) =
            migration_row(&conn, "session_v2", "sub");
        assert_eq!(project_id, "target-id");
        assert_eq!(directory, main);
        assert_eq!(path, "");
        assert_eq!(workspace_id, None);

        // The underscore sibling and every v1 history row stay untouched.
        let (project_id, directory, ..) = migration_row(&conn, "session_v2", "sibling");
        assert_eq!(project_id, "ws-proj");
        assert_eq!(directory, format!("{ws}_sibling"));
        assert_eq!(session_snapshot(&conn, "session"), legacy_before);
        // v2 leaves the deprecated mapping and worktree rows alone.
        assert_eq!(project_directory_snapshot(&conn), project_dirs_before);
        let worktrees: i64 = conn
            .query_row("SELECT COUNT(*) FROM worktree", [], |row| row.get(0))
            .expect("count worktree");
        assert_eq!(worktrees, 1);
    }

    #[test]
    fn inspect_and_migrate_use_session_v2_when_both_tables_exist() {
        let dir = TempDir::new();
        let db_path = dir.path().join("opencode.db");
        let ws = "/home/user/Workspace/main-repo/workspace-feature";
        let main_path = dir.path().join("main-repo");
        fs::create_dir_all(main_path.join(".git")).expect("create main .git");
        let main = main_path.to_string_lossy().into_owned();
        let conn = Connection::open(&db_path).expect("create db");
        fixture_schema_v2(&conn);
        insert_project(&conn, "ws-proj", ws);
        insert_project(&conn, "target-id", &main);
        insert_session_v2(
            &conn, "v2-root", "V2 root", "ws-proj", ws, "", None, None, 10,
        );
        insert_session_v2(
            &conn,
            "v2-sub",
            "V2 sub",
            "ws-proj",
            &format!("{ws}/sub"),
            "sub",
            None,
            Some("v2-root"),
            20,
        );
        insert_session_row(&conn, "v1-root", "V1 root", "ws-proj", ws, "", None, 30);
        let v2_before = session_snapshot(&conn, "session_v2");
        let v1_before = session_snapshot(&conn, "session");
        drop(conn);

        let rows = inspect_db(&db_path, Path::new(ws), &main_path)
            .expect("inspect succeeds")
            .expect("v2 rows");
        assert_eq!(
            rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            vec!["v2-sub", "v2-root"],
            "preview must list the live v2 table only"
        );

        // An id that exists only in the v1 table is drift for a v2 DB: skip,
        // and write nothing.
        let outcome = migrate_selected_db(
            &db_path,
            Path::new(ws),
            &main_path,
            &["v1-root".to_string()],
        )
        .expect("drift is not a failure");
        assert!(outcome.is_none(), "v1-only id must not match");

        let conn = Connection::open(&db_path).expect("reopen db");
        assert_eq!(session_snapshot(&conn, "session_v2"), v2_before);
        assert_eq!(session_snapshot(&conn, "session"), v1_before);
    }

    #[test]
    fn v2_target_resolution_uses_worktree_only_and_refuses_ambiguity() {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        fixture_schema_v2(&conn);
        insert_project(&conn, "global", "");
        insert_project(&conn, "cache-id", "/somewhere/else");
        let dir = TempDir::new();

        // A `.git/opencode` memo and the built-in global row must not be used
        // by v2: only `project.worktree` answers.
        let main = dir.path().join("main");
        fs::create_dir_all(main.join(".git")).expect("create .git");
        fs::write(main.join(".git").join("opencode"), "cache-id\n").expect("write cache");
        let err = resolve_target(&conn, SessionStore::V2, &main).expect_err("no row must refuse");
        assert!(err.contains("open opencode in the main repo once"), "{err}");
        assert!(err.contains("opencode v2"), "{err}");

        // Pure jj (no .git) without a worktree row refuses too — never 'global'.
        let pure_jj = dir.path().join("pure-jj");
        fs::create_dir_all(&pure_jj).expect("create pure jj dir");
        resolve_target(&conn, SessionStore::V2, &pure_jj)
            .expect_err("must not fall back to global");

        // Exactly one matching row wins.
        insert_project(&conn, "target-id", &main.to_string_lossy());
        assert_eq!(
            resolve_target(&conn, SessionStore::V2, &main).expect("row id"),
            "target-id"
        );

        // A second row sharing the worktree is ambiguous and must refuse.
        insert_project(&conn, "second-id", &main.to_string_lossy());
        let err = resolve_target(&conn, SessionStore::V2, &main).expect_err("ambiguous");
        assert!(err.contains("ambiguous"), "{err}");
        assert!(err.contains("2 project rows"), "{err}");
    }

    #[test]
    fn inspect_refuses_v2_schema_gap_and_missing_session_tables() {
        // v2 table present but missing a required column.
        let dir = TempDir::new();
        let db_path = dir.path().join("opencode.db");
        let conn = Connection::open(&db_path).expect("create db");
        conn.execute_batch(
            "CREATE TABLE project (id TEXT PRIMARY KEY, worktree TEXT);
             CREATE TABLE session_v2 (
                 id TEXT PRIMARY KEY,
                 project_id TEXT,
                 directory TEXT,
                 path TEXT,
                 time_updated INTEGER
             );",
        )
        .expect("create v2 schema");
        drop(conn);
        let err = inspect_db(
            &db_path,
            Path::new("/tmp/jj-workspace-inspect/ws"),
            Path::new("/tmp/jj-workspace-inspect/main"),
        )
        .expect_err("missing session_v2.workspace_id must refuse");
        assert!(err.contains("session_v2.workspace_id"), "{err}");

        // Neither session table: refuse instead of reporting zero matches.
        let dir = TempDir::new();
        let db_path = dir.path().join("opencode.db");
        let conn = Connection::open(&db_path).expect("create db");
        conn.execute_batch("CREATE TABLE project (id TEXT PRIMARY KEY, worktree TEXT);")
            .expect("create project-only schema");
        drop(conn);
        let err = inspect_db(
            &db_path,
            Path::new("/tmp/jj-workspace-inspect/ws"),
            Path::new("/tmp/jj-workspace-inspect/main"),
        )
        .expect_err("no session table must refuse");
        assert!(err.contains("neither session_v2 nor session"), "{err}");
    }

    #[test]
    fn inspect_refuses_ambiguous_v2_target_after_matching_rows() {
        let dir = TempDir::new();
        let db_path = dir.path().join("opencode.db");
        let ws = "/home/user/Workspace/main-repo/workspace-feature";
        let main_path = dir.path().join("main-repo");
        let main = main_path.to_string_lossy().into_owned();
        let conn = Connection::open(&db_path).expect("create db");
        fixture_schema_v2(&conn);
        insert_project(&conn, "ws-proj", ws);
        insert_project(&conn, "target-id", &main);
        insert_project(&conn, "second-id", &main);
        insert_session_v2(&conn, "root", "Root", "ws-proj", ws, "", None, None, 1);
        drop(conn);

        let err = inspect_db(&db_path, Path::new(ws), &main_path)
            .expect_err("ambiguous target must block the preview");
        assert!(err.contains("ambiguous"), "{err}");
        assert!(err.contains("workspace was not removed"), "{err}");
    }

    /// Manual verification harness (ignored by default): runs the production
    /// preview + migration against a copied real DB. Set:
    ///   JJ_VERIFY_XDG=/tmp/... (contains opencode/opencode.db)
    ///   JJ_VERIFY_WS=<jj workspace path>
    ///   JJ_VERIFY_MAIN=<main repo path>
    /// and run:
    ///   cargo test manual_v2_real_db_copy_verification -- --ignored --nocapture
    #[test]
    #[ignore]
    fn manual_v2_real_db_copy_verification() {
        let (Ok(xdg), Ok(ws), Ok(main)) = (
            std::env::var("JJ_VERIFY_XDG"),
            std::env::var("JJ_VERIFY_WS"),
            std::env::var("JJ_VERIFY_MAIN"),
        ) else {
            eprintln!("skipping: set JJ_VERIFY_XDG / JJ_VERIFY_WS / JJ_VERIFY_MAIN");
            return;
        };
        let ws = PathBuf::from(ws);
        let main = PathBuf::from(main);
        std::env::set_var("XDG_DATA_HOME", &xdg);

        let rows = match inspect_opencode_sessions(&ws, &main) {
            Inspection::Ready { rows, .. } => rows,
            other => panic!("expected Ready, got {other:?}"),
        };
        assert!(!rows.is_empty(), "the copy must have rows for {ws:?}");
        let ids: Vec<String> = rows.iter().map(|row| row.id.clone()).collect();

        match migrate_selected_opencode_sessions(&ws, &main, &ids) {
            Outcome::Migrated { count, .. } => assert_eq!(count, ids.len()),
            other => panic!("expected Migrated, got {other:?}"),
        }
        match inspect_opencode_sessions(&ws, &main) {
            Inspection::Skipped(_) => {}
            other => panic!("expected no rows after migration, got {other:?}"),
        }

        // Direct column check on the copy: every selected row landed on the
        // main repo project with the official Moved projection values.
        let main_str = main.to_string_lossy();
        let db = Path::new(&xdg).join("opencode").join("opencode.db");
        let conn = Connection::open(&db).expect("open copy");
        let target: String = conn
            .query_row(
                "SELECT id FROM project WHERE worktree = ?1",
                params![main_str.as_ref()],
                |row| row.get(0),
            )
            .expect("target project");
        for id in &ids {
            let (project_id, directory, path, workspace_id) = conn
                .query_row(
                    "SELECT project_id, directory, path, workspace_id \
                     FROM session_v2 WHERE id = ?1",
                    params![id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, Option<String>>(3)?,
                        ))
                    },
                )
                .expect("migrated row");
            assert_eq!(project_id, target, "{id}");
            assert_eq!(directory, main_str, "{id}");
            assert_eq!(path, "", "{id}");
            assert_eq!(workspace_id, None, "{id}");
        }
    }
}
