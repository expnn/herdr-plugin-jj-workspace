# workspace-name-edit Tasks

## 1. State machine implementation

- [x] 1.1 Introduce `NameEditState` enum (Fresh / Prefixed / Free) and a pure `apply_name_key(name, state, key) -> (String, NameEditState)` function implementing the transition table from design.md D1/D2/D3 (prefix derived via `rsplit_once('/')`, no hardcoded prefix; component ops only at anchor states)
- [x] 1.2 Wire the name-field `KeyCode::Char` branch (main.rs:1477-1488) to `apply_name_key`; replace `replace_on_type` (name-only, main.rs:1356) with the `NameEditState` variable
- [x] 1.3 Wire the name-field `KeyCode::Backspace` branch (main.rs:1468-1476) to `apply_name_key`
- [x] 1.4 Remove the now-dead `replace_on_type` name usage; keep `base_replace_on_type` (base field) untouched

## 2. Tests

- [x] 2.1 Unit-test the full transition table: Fresh+Char keeps prefix and replaces slug; Fresh+Backspace drops slug to Prefixed; Prefixed+Char appends; Prefixed+Backspace clears to Free; Free+Char appends; Free+Backspace pops one char
- [x] 2.2 Unit-test composite paths: double-Backspace clears prefix then free-typed unprefixed name; Fresh→Free then per-char backspace (typo-fix case `workspace/fix-ap1` → `workspace/fix-ap`); multi-segment user name `feature/foo` edits per-char

## 3. Verification

- [x] 3.1 `cargo test` all green (including existing wizard rendering/validation tests)
- [x] 3.2 Manual smoke: open wizard — first keystroke keeps prefix; single Backspace drops slug; second Backspace drops prefix; free-typed name edits per-char; empty-name submit still shows "name must match [A-Za-z0-9._/-]"