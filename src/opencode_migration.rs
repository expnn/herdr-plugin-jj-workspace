//! opencode session migration for `remove`.
//!
//! opencode binds every session to a directory (`session.directory`) and a
//! project (`session.project_id`). When a jj workspace directory is deleted,
//! those rows become invisible forever, so `cmd_remove` migrates them to the
//! main repo first, mirroring opencode's own `SessionEvent.Moved` projection
//! plus its project adoption: `project_id` is rewritten to the main repo's
//! project, `directory` to the main repo root, `path` flattened to `''`,
//! `workspace_id` released to NULL and `time_updated` stamped now.
//!
//! Every failure that could leave data half-written is fail-closed: the
//! caller refuses to remove the workspace and the user can retry. A missing
//! opencode binary or an empty match set is a silent skip — the plugin stays
//! agent-agnostic (design D8).
//!
//! Design decisions D1–D10 live in
//! `openspec/changes/migrate-opencode-sessions-on-remove/design.md`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

/// Result of a migration attempt, mapped by `cmd_remove` to continue /
/// refuse / report.
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

/// Migrate every opencode session bound to `ws` (root or subdirectory) to
/// `main_repo`. Discovery failures and an empty match set are skips; DB,
/// schema, resolution and transaction failures are refusals.
pub fn migrate_opencode_sessions(ws: &Path, main_repo: &Path) -> Outcome {
    let opencode = match find_opencode(&crate::path_dirs()) {
        Some(path) => path,
        None => return Outcome::Skipped("opencode not found on PATH".into()),
    };
    let db_path = match discover_db_path(&opencode) {
        Some(path) => path,
        None => return Outcome::Skipped("opencode db path unavailable".into()),
    };
    match migrate_db(&db_path, ws, main_repo) {
        Ok(Some(count)) => Outcome::Migrated {
            count,
            main_repo: main_repo.to_path_buf(),
        },
        Ok(None) => Outcome::Skipped("no opencode sessions bound to this workspace".into()),
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

/// Resolve the `opencode` binary on PATH. A missing binary is a skip, not an
/// error: the plugin stays agent-agnostic (D8).
fn find_opencode(path_dirs: &[PathBuf]) -> Option<PathBuf> {
    path_dirs
        .iter()
        .map(|dir| dir.join("opencode"))
        .find(|candidate| crate::is_executable_file(candidate))
}

/// Ask opencode for its DB path (`opencode db path`). The CLI only reads the
/// path — it is never used to write (D1); discovery failures are skips.
fn discover_db_path(opencode: &Path) -> Option<PathBuf> {
    let output = Command::new(opencode).args(["db", "path"]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    db_path_from_output(&output.stdout)
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

/// Open the DB and run the read-only checks before the migration
/// transaction. `Ok(None)` = no matching sessions (skip); `Err` = fail-closed.
fn migrate_db(db_path: &Path, ws: &Path, main: &Path) -> Result<Option<usize>, String> {
    let mut conn = Connection::open(db_path).map_err(|err| {
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
    probe_schema(&conn)?;
    // Count before resolving the target: zero matching sessions is a skip
    // even when the main repo's project id cannot be resolved (D8).
    if count_sessions(&conn, ws)? == 0 {
        return Ok(None);
    }
    let target = resolve_target(&conn, main)?;
    let updated = migrate_in_tx(&mut conn, ws, main, &target)?;
    Ok(Some(updated))
}

/// Fail-closed schema probe (D7): every column the migration touches must
/// exist before the first write. opencode's history is ADD COLUMN-only for
/// these tables, but a future rewrite must stop us rather than corrupt data.
fn probe_schema(conn: &Connection) -> Result<(), String> {
    let session_columns = table_columns(conn, "session")?;
    for column in SESSION_REQUIRED_COLUMNS {
        if !session_columns.iter().any(|name| name == column) {
            return Err(format!(
                "opencode DB schema is missing session.{column} (refusing to remove)"
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
    Ok(())
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
fn count_sessions(conn: &Connection, ws: &Path) -> Result<usize, String> {
    let ws = ws.to_string_lossy();
    let count: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM session WHERE {WORKSPACE_RANGE}"),
            params![ws.as_ref()],
            |row| row.get(0),
        )
        .map_err(|err| format!("cannot enumerate opencode sessions (refusing to remove): {err}"))?;
    Ok(count.max(0) as usize)
}

/// Resolve the main repo's opencode project id without re-deriving any hash
/// (D3): pure-jj repo → `'global'`; else the `.git/opencode` memo; else the
/// `project` row keyed by worktree; else fail-closed with guidance.
fn resolve_target(conn: &Connection, main: &Path) -> Result<String, String> {
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

/// One immediate write transaction (D2): rewrite every matching session row,
/// drop stale `project_directory` rows, then commit. Any error rolls back
/// and the caller refuses the removal; nothing is half-written.
fn migrate_in_tx(
    conn: &mut Connection,
    ws: &Path,
    main: &Path,
    target: &str,
) -> Result<usize, String> {
    let ws = ws.to_string_lossy();
    let main = main.to_string_lossy();
    let now_ms = unix_millis();
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|err| {
            format!("cannot start the opencode migration transaction (refusing to remove): {err}")
        })?;
    let updated = tx
        .execute(
            &format!(
                "UPDATE session SET project_id = ?2, directory = ?3, path = '', \
                 workspace_id = NULL, time_updated = ?4 WHERE {WORKSPACE_RANGE}"
            ),
            params![ws.as_ref(), target, main.as_ref(), now_ms],
        )
        .map_err(|err| format!("cannot migrate opencode sessions (refusing to remove): {err}"))?;
    if table_exists(&tx, "project_directory")? {
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
        conn.execute(
            "INSERT INTO session (id, project_id, directory, path, workspace_id, time_updated) \
             VALUES (?1, ?2, ?3, ?4, ?5, 0)",
            params![id, project_id, directory, path, workspace_id],
        )
        .expect("insert session");
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
        assert_eq!(count_sessions(&conn, Path::new(ws)).expect("count"), 3);
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
        assert_eq!(resolve_target(&conn, &pure_jj).expect("global"), "global");

        // (1) Cache file + matching project row wins.
        let cached = dir.path().join("cached");
        fs::create_dir_all(cached.join(".git")).expect("create .git");
        fs::write(cached.join(".git").join("opencode"), "cache-id\n").expect("write cache");
        insert_project(&conn, "cache-id", "/somewhere/else");
        assert_eq!(
            resolve_target(&conn, &cached).expect("cache id"),
            "cache-id"
        );

        // (2) No usable cache → the project row keyed by worktree.
        let by_row = dir.path().join("by-row");
        fs::create_dir_all(by_row.join(".git")).expect("create .git");
        insert_project(&conn, "row-id", &by_row.to_string_lossy());
        assert_eq!(resolve_target(&conn, &by_row).expect("row id"), "row-id");

        // (3) Neither → fail-closed with actionable guidance.
        let missing = dir.path().join("missing");
        fs::create_dir_all(missing.join(".git")).expect("create .git");
        let err = resolve_target(&conn, &missing).expect_err("must refuse");
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
        assert_eq!(resolve_target(&conn, &main).expect("row id"), "row-id");
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
        let outcome = migrate_db(
            &db_path,
            &dir.path().join("workspace"),
            &dir.path().join("main"),
        )
        .expect("no error");
        assert!(outcome.is_none(), "count 0 must skip");
    }

    #[test]
    fn missing_opencode_binary_is_a_skip() {
        assert!(find_opencode(&[]).is_none());
        let dir = TempDir::new();
        // A non-executable file named opencode is not a usable binary.
        fs::write(dir.path().join("opencode"), "not a binary").expect("write decoy");
        assert!(find_opencode(&[dir.path().to_path_buf()]).is_none());
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
        let migrated = migrate_db(&db_path, Path::new(ws), Path::new(&main))
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
        let err = migrate_in_tx(&mut conn, Path::new(ws), Path::new(main), "no-such-project")
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
        let err = migrate_in_tx(&mut conn, Path::new(ws), Path::new(main), "target-id")
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
}
