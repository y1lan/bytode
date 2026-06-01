# bytode UI Design Log

## 2026-06-01

### Panel/window-manager rewrite

- Replaced the old layout-centric TUI structure with `AppShell`,
  `WindowManager`, panels, and overlays.
- Moved UI-local state out of `repl.rs` and into panel-owned state.
- Added `ExecState`, `Effect`, `PanelId`, `OverlayId`, and `KeyAction`
  primitives in `src/ui/events.rs`.

### UI module structure

Current UI structure:

```text
src/ui/
├── mod.rs
├── app_shell.rs
├── events.rs
├── window_manager.rs
├── overlays/
│   └── mod.rs
├── panels/
│   ├── mod.rs
│   ├── input.rs
│   ├── sidebar.rs
│   ├── status_bar.rs
│   └── content/
│       ├── mod.rs
│       ├── model.rs
│       ├── parser.rs
│       ├── components.rs
│       └── panel.rs
├── render/
│   ├── mod.rs
│   ├── markdown.rs
│   └── prompt.rs
└── statusbar.rs
```

### Layered key routing

Current routing order:

```text
Raw KeyEvent
  -> KeyAction
  -> modal overlay
  -> global keys
  -> Ctrl+C CancelIntent by ExecState
  -> non-modal overlay capture
  -> focused panel
```

### Overlays

- Added `OverlayStack` with modal-first and capture routing semantics.
- Added `Help` and `Dialog` overlays as the first concrete overlay types.
- `Ctrl+/` toggles help.
- `Esc` closes the top overlay.

### Input editor

- `InputPanel` now owns local cursor state and multiline editing.
- `Enter` submits.
- `Shift+Enter` inserts newline.
- `Ctrl+A` / `Ctrl+E` move to start/end.
- `Ctrl+U` clears input.
- `Backspace` / `Delete` edit at cursor.
- Footer hints and transient notices render below input content and do not enter
  transcript history.

### Cancel/exit semantics

- `Ctrl+C` is treated as `CancelIntent`, not unconditional exit.
- `Ctrl+D` is the explicit exit key.
- Idle input state can surface `Ctrl+D exits` as a notice.

### Transcript structure

- `ContentPanel` was split into dedicated modules:
  - `model.rs`
  - `parser.rs`
  - `components.rs`
  - `panel.rs`
- This prepares transcript rendering for semantic blocks rather than raw
  stdout-style output.

### Status bar and footer

- Footer hints were moved into `InputPanel`.
- The bottom global status bar no longer repeats the current model string.
