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
    widgets::{Block, Borders, Clear, Paragraph},
    Frame, Terminal,
};
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
}

#[derive(Clone, Copy)]
struct WizardView<'a> {
    choices: &'a [WorkspaceChoice],
    filtered: &'a [usize],
    selected: usize,
    field: WizardField,
    query: &'a str,
    name: &'a str,
    root: &'a Path,
    error: Option<&'a str>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WizardField {
    WorkspaceSearch,
    Name,
}

const JJ_UPDATE_COMMAND: &str = "jj git fetch && jj rebase -s @ -d 'trunk()'";

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

/// Action (headless): capture the calling workspace, then open the wizard pane.
fn cmd_open(_mode: &str) -> ! {
    let ctx = env::var("HERDR_PLUGIN_CONTEXT_JSON").unwrap_or_default();
    let cwd = json_string_field(&ctx, "focused_pane_cwd")
        .or_else(|| json_string_field(&ctx, "workspace_cwd"))
        .unwrap_or_default();
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
    .arg(format!("JJ_CURRENT_CWD={cwd}"))
    .arg("--env")
    .arg(format!("JJ_CURRENT_WORKSPACE={workspace_id}"))
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
/// Codex-left / terminal-right Herdr workspace.
fn cmd_wizard() -> ! {
    let current_cwd = env::var("JJ_CURRENT_CWD").unwrap_or_default();
    let current_workspace = env::var("JJ_CURRENT_WORKSPACE").unwrap_or_default();
    let choices = match load_workspace_choices(&current_workspace, &current_cwd) {
        Ok(choices) if !choices.is_empty() => choices,
        Ok(_) => fail("Herdr has no workspaces to select"),
        Err(err) => fail(&err),
    };
    let selected = choices
        .iter()
        .position(|choice| choice.id == current_workspace)
        .unwrap_or(0);
    let root = workspaces_root();

    let selection = match run_workspace_wizard(&choices, selected, &root, generated_name(seed())) {
        Ok(Some(selection)) => selection,
        Ok(None) => process::exit(0),
        Err(err) => fail(&format!("terminal error: {err}")),
    };
    let source = selection.source.path.trim_end_matches('/').to_string();
    if source.is_empty() || !Path::new(&source).is_dir() {
        fail(&format!("workspace folder does not exist: {source}"));
    }

    let is_jj = is_jj_workspace(&source);
    let destination = if is_jj {
        if which("jj").is_none() {
            fail("jj not found on PATH");
        }
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

        // Create from local state. Fetch + rebase run later in the right pane,
        // after the new Herdr workspace and Codex are already available.
        let base = config_value("JJ_BASE_REV").unwrap_or_else(|| "trunk()".into());
        eprintln!(
            "+ jj workspace add --name {} -r {base} {dest}",
            selection.name
        );
        let mut add = Command::new("jj");
        add.current_dir(&repo).args([
            "workspace",
            "add",
            "--name",
            &selection.name,
            "-r",
            &base,
            &dest,
        ]);
        run_or(add, "jj workspace add", fail);

        let mut bookmark = Command::new("jj");
        bookmark
            .current_dir(&dest)
            .args(["bookmark", "create", &selection.name, "-r", "@"]);
        if !run(bookmark) {
            eprintln!(
                "warning: could not create bookmark {} (workspace still created)",
                selection.name
            );
        }
        dest
    } else {
        source
    };

    open_tab_layout(&selection.source.id, &destination, &selection.name, is_jj);
    process::exit(0);
}

fn load_workspace_choices(
    current_workspace: &str,
    current_cwd: &str,
) -> Result<Vec<WorkspaceChoice>, String> {
    let workspaces = herdr_json(&["workspace", "list"])?;
    let panes = herdr_json(&["pane", "list"])?;
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

        let path = if id == current_workspace && !current_cwd.is_empty() {
            Some(current_cwd.to_string())
        } else if let Some(path) = workspace
            .pointer("/worktree/checkout_path")
            .and_then(Value::as_str)
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

        if let Some(path) = path.filter(|path| !path.is_empty()) {
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

fn herdr_json(args: &[&str]) -> Result<Value, String> {
    let output = Command::new(herdr_bin())
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

fn open_tab_layout(workspace_id: &str, cwd: &str, label: &str, is_jj: bool) {
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

    // This plugin owns Codex startup for the tab it creates. Keeping this out
    // of the global tab.created hook prevents duplicate launches and lets the
    // same flow work for non-jj source folders.
    let mut start_codex = Command::new(&herdr);
    start_codex.args(["pane", "run", &left_pane, "co"]);
    run_or(start_codex, "start Codex in left pane", fail);

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
    let finish = finish_tab_shell_command(workspace_id, &tab_id, &left_pane);
    let right_command = if is_jj {
        format!("nohup {finish} >/dev/null 2>&1 </dev/null & {JJ_UPDATE_COMMAND}")
    } else {
        finish
    };
    let mut run_right = Command::new(&herdr);
    run_right.args(["pane", "run", &right_pane, &right_command]);
    run_or(run_right, "start right-pane setup", fail);

    if !is_jj {
        let body = format!(
            "{} is not a jj workspace; opened the same folder without creating a checkout.",
            cwd
        );
        let mut toast = Command::new(&herdr);
        toast.args([
            "notification",
            "show",
            "No jj workspace created",
            "--body",
            &body,
            "--position",
            "top-right",
            "--sound",
            "none",
        ]);
        if !run(toast) {
            eprintln!("warning: could not show the non-jj workspace notification");
        }
    }
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

fn wait_for_codex_and_accept_trust(pane_id: &str, timeout: Duration) -> bool {
    let started = std::time::Instant::now();
    let mut trust_attempts = 0;
    let mut blocked_without_trust_since: Option<std::time::Instant> = None;
    let mut ready_since: Option<std::time::Instant> = None;
    while started.elapsed() < timeout {
        let pane_text = read_pane_text(pane_id);
        let normalized = pane_text.split_whitespace().collect::<Vec<_>>().join(" ");
        let trust_prompt = normalized.contains("Do you trust the contents of this directory?")
            && normalized.contains("1. Yes, continue")
            && normalized.contains("2. No, quit");
        if trust_prompt {
            if trust_attempts >= 5 {
                return false;
            }
            let mut accept = Command::new(herdr_bin());
            accept.args(["pane", "send-keys", pane_id, "enter"]);
            if !run(accept) {
                return false;
            }
            trust_attempts += 1;
            blocked_without_trust_since = None;
            ready_since = None;
            thread::sleep(Duration::from_millis(200));
            continue;
        }
        if normalized.contains("OpenAI Codex") && normalized.contains("Ask Codex to do anything") {
            let ready = ready_since.get_or_insert_with(std::time::Instant::now);
            if ready.elapsed() >= Duration::from_secs(1) {
                return true;
            }
        } else {
            ready_since = None;
        }

        if let Ok(agents) = herdr_json(&["agent", "list"]) {
            let status = agents
                .pointer("/result/agents")
                .and_then(Value::as_array)
                .and_then(|agents| {
                    agents
                        .iter()
                        .find(|agent| {
                            agent.get("pane_id").and_then(Value::as_str) == Some(pane_id)
                                && agent.get("agent").and_then(Value::as_str) == Some("codex")
                        })
                        .and_then(|agent| agent.get("agent_status").and_then(Value::as_str))
                });
            match status {
                Some("blocked") => {
                    let blocked_since =
                        blocked_without_trust_since.get_or_insert_with(std::time::Instant::now);
                    if blocked_since.elapsed() >= Duration::from_secs(1) {
                        return false;
                    }
                }
                _ => blocked_without_trust_since = None,
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
    false
}

fn read_pane_text(pane_id: &str) -> String {
    Command::new(herdr_bin())
        .args([
            "pane", "read", pane_id, "--source", "visible", "--lines", "80",
        ])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default()
}

fn finish_tab_shell_command(workspace_id: &str, tab_id: &str, left_pane: &str) -> String {
    let executable = env::current_exe()
        .unwrap_or_else(|err| fail(&format!("cannot resolve jj-workspace executable: {err}")));
    format!(
        "{} finish-tab {} {} {}",
        shell_quote(&executable.display().to_string()),
        shell_quote(workspace_id),
        shell_quote(tab_id),
        shell_quote(left_pane),
    )
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn cmd_finish_tab(args: &[String]) -> ! {
    let workspace_id = args.get(2).map(String::as_str).unwrap_or_default();
    let tab_id = args.get(3).map(String::as_str).unwrap_or_default();
    let left_pane = args.get(4).map(String::as_str).unwrap_or_default();
    if workspace_id.is_empty() || tab_id.is_empty() || left_pane.is_empty() {
        die("finish-tab requires workspace, tab, and pane IDs");
    }

    if !wait_for_codex_and_accept_trust(left_pane, Duration::from_secs(20)) {
        let mut toast = Command::new(herdr_bin());
        toast.args([
            "notification",
            "show",
            "Codex needs attention",
            "--body",
            "Codex did not clear its startup prompt automatically.",
            "--position",
            "top-right",
            "--sound",
            "request",
        ]);
        let _ = toast.status();
        process::exit(1);
    }

    let herdr = herdr_bin();
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

/// Action (headless): forget the current jj workspace, delete it, close its tab.
fn cmd_remove() -> ! {
    if which("jj").is_none() {
        die("jj not found on PATH");
    }
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

    let mut forget = Command::new("jj");
    forget.current_dir(&canon).args(["workspace", "forget"]);
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
        yellow: Color::Rgb(249, 226, 175),
    }
}

/// Returns the chosen source + name, or None when cancelled.
fn run_workspace_wizard(
    choices: &[WorkspaceChoice],
    initial_selection: usize,
    root: &Path,
    initial_name: String,
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
    let mut replace_on_type = true;
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
                    field = match field {
                        WizardField::WorkspaceSearch => WizardField::Name,
                        WizardField::Name => WizardField::WorkspaceSearch,
                    };
                    error = None;
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
                    if is_jj_workspace(&source.path) {
                        let checkout = workspace_destination(root, &source.path, &name);
                        if checkout.exists() {
                            error =
                                Some(format!("checkout already exists: {}", checkout.display()));
                            continue;
                        }
                    }
                    break Some(WizardResult {
                        source,
                        name: name.clone(),
                    });
                }
                KeyCode::Backspace if field == WizardField::WorkspaceSearch => {
                    query.pop();
                    filtered = filtered_choice_indices(choices, &query);
                    selected = 0;
                    error = None;
                }
                KeyCode::Backspace if field == WizardField::Name => {
                    if replace_on_type {
                        name.clear();
                        replace_on_type = false;
                    } else {
                        name.pop();
                    }
                    error = None;
                }
                KeyCode::Char(c)
                    if field == WizardField::Name
                        && !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    if replace_on_type {
                        name.clear();
                        replace_on_type = false;
                    }
                    name.push(c);
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

fn draw_workspace_wizard(frame: &mut Frame, view: &WizardView<'_>) {
    let WizardView {
        choices,
        filtered,
        selected,
        field,
        query,
        name,
        root,
        error,
    } = *view;
    let p = catppuccin();
    let area = frame.area();
    dim_background(frame, area);
    let Some(inner) = render_modal_shell(frame, area, 86, 22, &p) else {
        return;
    };
    if inner.height < 12 || choices.is_empty() {
        return;
    }

    let list_height = usize::from(inner.height.saturating_sub(11).clamp(3, 8));
    let max_start = filtered.len().saturating_sub(list_height);
    let start = selected.saturating_sub(list_height / 2).min(max_start);
    let end = (start + list_height).min(filtered.len());
    let mut y = inner.y;

    render_modal_header(
        frame,
        Rect::new(inner.x, y, inner.width, 1),
        "new workspace",
        &p,
    );
    y += 1;
    let source_style = if field == WizardField::WorkspaceSearch {
        Style::default().fg(p.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(p.overlay0)
    };
    frame.render_widget(
        Paragraph::new(" source workspace  type to filter · ↑/↓ navigate · tab edit name")
            .style(source_style),
        Rect::new(inner.x, y, inner.width, 1),
    );
    y += 1;
    let query_cursor = if field == WizardField::WorkspaceSearch {
        "█"
    } else {
        ""
    };
    frame.render_widget(
        Paragraph::new(format!(" {query}{query_cursor}"))
            .style(Style::default().fg(p.text).bg(p.surface0)),
        Rect::new(inner.x, y, inner.width, 1),
    );
    y += 1;

    for (visible_index, choice_index) in filtered[start..end].iter().enumerate() {
        let absolute_index = start + visible_index;
        let choice = &choices[*choice_index];
        let active = absolute_index == selected;
        let marker = if active { " ▸ " } else { "   " };
        let kind = if is_jj_workspace(&choice.path) {
            "jj"
        } else {
            "dir"
        };
        let line = Line::from(vec![
            Span::styled(
                format!("{marker}{} ", choice.label),
                Style::default().add_modifier(if active {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            ),
            Span::styled(
                format!("[{kind}] {}", choice.path),
                Style::default().fg(p.subtext0),
            ),
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
            Paragraph::new("   no matching workspaces").style(Style::default().fg(p.overlay0)),
            Rect::new(inner.x, y, inner.width, 1),
        );
        y += 1;
    }
    while y < inner.y + 3 + list_height as u16 {
        y += 1;
    }

    let name_style = if field == WizardField::Name {
        Style::default().fg(p.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(p.overlay0)
    };
    frame.render_widget(
        Paragraph::new(" name  tab to edit").style(name_style),
        Rect::new(inner.x, y, inner.width, 1),
    );
    y += 1;
    let cursor = if field == WizardField::Name {
        "█"
    } else {
        ""
    };
    frame.render_widget(
        Paragraph::new(format!(" {name}{cursor}"))
            .style(Style::default().fg(p.text).bg(p.surface0)),
        Rect::new(inner.x, y, inner.width, 1),
    );
    y += 1;

    let choice = filtered.get(selected).map(|index| &choices[*index]);
    let (preview_label, preview, warning) = match choice {
        Some(choice) if is_jj_workspace(&choice.path) => (
            " checkout",
            workspace_destination(root, &choice.path, name)
                .display()
                .to_string(),
            None,
        ),
        Some(choice) => (
            " folder",
            choice.path.clone(),
            Some("not a jj workspace — the same folder will be opened"),
        ),
        None => (" workspace", "no matching workspace".into(), None),
    };
    frame.render_widget(
        Paragraph::new(preview_label).style(Style::default().fg(p.overlay0)),
        Rect::new(inner.x, y, inner.width, 1),
    );
    y += 1;
    frame.render_widget(
        Paragraph::new(format!(" {preview}")).style(Style::default().fg(p.subtext0)),
        Rect::new(inner.x, y, inner.width, 1),
    );
    y += 1;
    if let Some(message) = error.or(warning) {
        let color = if error.is_some() { p.red } else { p.yellow };
        frame.render_widget(
            Paragraph::new(format!(" {message}")).style(Style::default().fg(color)),
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

/// Checkout root: $JJ_WORKSPACE_ROOT override, else ~/.herdr/workspaces.
fn workspaces_root() -> PathBuf {
    if let Some(root) = config_value("JJ_WORKSPACE_ROOT") {
        return PathBuf::from(expand_tilde(root.trim_end_matches('/')));
    }
    PathBuf::from(expand_tilde("~/.herdr/workspaces"))
}

fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = env::var("HOME") {
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
        .unwrap_or_else(|| "nathanflurry.jj-workspace".into())
}

fn is_jj_workspace(repo: &str) -> bool {
    !repo.is_empty() && Path::new(repo).join(".jj").exists()
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

fn config_value(key: &str) -> Option<String> {
    if let Ok(value) = env::var(key) {
        if !value.is_empty() {
            return Some(value);
        }
    }
    let dir = env::var("HERDR_PLUGIN_CONFIG_DIR").ok()?;
    let content = fs::read_to_string(Path::new(&dir).join(".env")).ok()?;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            if k.trim() == key {
                let v = v.trim().trim_matches('"').trim_matches('\'');
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

fn which(cmd: &str) -> Option<()> {
    let paths = env::var_os("PATH")?;
    env::split_paths(&paths)
        .find(|dir| dir.join(cmd).is_file())
        .map(|_| ())
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
    eprintln!("error: {message}");
    print!("\npress enter to close...");
    let _ = io::stdout().flush();
    let mut line = String::new();
    let _ = io::stdin().read_line(&mut line);
    process::exit(1);
}

fn die(message: &str) -> ! {
    eprintln!("error: {message}");
    process::exit(1);
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
    fn update_runs_fetch_before_rebase() {
        assert_eq!(
            JJ_UPDATE_COMMAND,
            "jj git fetch && jj rebase -s @ -d 'trunk()'"
        );
    }

    #[test]
    fn shell_arguments_are_single_quoted() {
        assert_eq!(shell_quote("plain"), "'plain'");
        assert_eq!(shell_quote("it's"), "'it'\"'\"'s'");
    }
}
