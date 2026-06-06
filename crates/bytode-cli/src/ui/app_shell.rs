use crate::agent::{Agent, AgentMode};
use crate::project::ProjectProfile;
use crate::ui::events::{
    Effect, ExecState, KeyAction, MouseAction, OverlayId, OverlayState, PanelId, TurnId, WindowSlot,
};
use crate::ui::overlays::OverlayStack;
use crate::ui::panels::{
    ContentPanel, HistoryEntry, InputPanel, PanelNode, RenderContext, RuntimeSnapshot,
    SidebarPanel, StatusBarPanel, UiContext,
};
use crate::ui::statusbar::StatusBarState;
use crate::ui::window_manager::WindowManager;
use ratatui::prelude::*;

pub struct AppShell {
    window_manager: WindowManager,
    overlays: OverlayStack,
    exec_state: ExecState,
    runtime: RuntimeSnapshot,
    sidebar_visible: bool,
    next_turn_id: TurnId,
    saved_main_entries: Option<Vec<HistoryEntry>>,
}

impl AppShell {
    pub fn new(
        profile: &ProjectProfile,
        git_branch: Option<String>,
        git_dirty: bool,
        detection_source: &str,
        chat_history_init: Vec<HistoryEntry>,
    ) -> Self {
        let runtime = RuntimeSnapshot {
            tool_names: Vec::new(),
            status: StatusBarState {
                dir: std::env::current_dir()
                    .map(|dir| dir.to_string_lossy().to_string())
                    .unwrap_or_default(),
                git_branch,
                git_dirty,
                task: String::new(),
                model: String::new(),
                ctx_used: 0,
                ctx_total: 1_000_000,
                tool_count: 0,
                session_cost: 0.0,
                session_calls: 0,
                mode: String::new(),
                elapsed: "0ms".into(),
                spinner: ' ',
            },
            primary_language: format!("{} [{}]", profile.primary, profile.build_system),
            detection_source: detection_source.to_string(),
        };

        let panels = vec![
            PanelNode::Content(ContentPanel::new(chat_history_init)),
            PanelNode::Input(InputPanel::new()),
            PanelNode::Sidebar(SidebarPanel::new()),
            PanelNode::StatusBar(StatusBarPanel::new()),
        ];

        let mut shell = Self {
            window_manager: WindowManager::new(panels),
            overlays: OverlayStack::new(),
            exec_state: ExecState::Idle,
            runtime,
            sidebar_visible: false,
            next_turn_id: 1,
            saved_main_entries: None,
        };
        shell.normalize_focus();
        shell
    }

    pub fn sync_runtime_from_agent(&mut self, agent: &Agent, elapsed: String, spinner: char) {
        self.runtime.tool_names = agent.tool_names();
        self.runtime.status.model = agent.model_name().to_string();
        self.runtime.status.ctx_used = agent.context_used();
        self.runtime.status.ctx_total = agent.context_total();
        self.runtime.status.tool_count = self.runtime.tool_names.len();
        self.runtime.status.session_cost = agent.session_cost();
        self.runtime.status.session_calls = agent.session_call_count();
        self.runtime.status.mode = mode_label(agent.mode()).to_string();
        self.runtime.status.elapsed = elapsed;
        self.runtime.status.spinner = spinner;
        self.runtime.status.task = task_label(&self.exec_state).to_string();
        self.normalize_focus();
    }

    pub fn render(&self, frame: &mut Frame) {
        let ctx = RenderContext {
            exec_state: &self.exec_state,
            runtime: &self.runtime,
            sidebar_visible: self.sidebar_visible,
            focused: Some(self.window_manager.focus()),
        };
        self.window_manager.render(frame, &ctx);
        self.overlays.render(frame, &ctx);
    }

    pub fn dispatch_key(&mut self, key: KeyAction) -> Vec<Effect> {
        if let Some(effects) = self.overlays.handle_modal_key(key.clone()) {
            return effects;
        }

        if let Some(effects) = self.route_global_key(&key) {
            return effects;
        }

        if let Some(effects) = self.route_cancel_intent(&key) {
            return effects;
        }

        if let Some(effects) = self.overlays.handle_capture_key(key.clone()) {
            return effects;
        }

        let exec_state = self.exec_state.clone();
        let runtime = self.runtime.clone();
        let mut effects = self.window_manager.dispatch_key(
            key,
            &UiContext {
                exec_state: &exec_state,
                runtime: &runtime,
                sidebar_visible: self.sidebar_visible,
            },
        );
        self.rewrite_panel_effects(&mut effects);
        effects
    }

    pub fn dispatch_mouse(&mut self, mouse: MouseAction, area: Rect) -> Vec<Effect> {
        if self.overlays.has_modal() {
            return Vec::new();
        }

        let exec_state = self.exec_state.clone();
        let runtime = self.runtime.clone();
        let mut effects = self.window_manager.dispatch_mouse(
            mouse,
            area,
            &UiContext {
                exec_state: &exec_state,
                runtime: &runtime,
                sidebar_visible: self.sidebar_visible,
            },
        );
        self.rewrite_panel_effects(&mut effects);
        effects
    }

    pub fn start_turn(&mut self, turn_id: TurnId, input: &str) {
        self.exec_state = ExecState::Streaming { turn_id };
        self.runtime.status.task = task_label(&self.exec_state).to_string();
        self.input_panel_mut().clear_notice();
        self.content_panel_mut().push_user(input.to_string());
        self.content_panel_mut().scroll_to_bottom();
        self.content_panel_mut().begin_stream();
    }

    pub fn append_stream_chunk(&mut self, turn_id: TurnId, chunk: &str) {
        if self.exec_state.active_turn_id() != Some(turn_id) {
            return;
        }
        self.content_panel_mut().append_stream_chunk(chunk);
    }

    pub fn finish_turn(&mut self, turn_id: TurnId) {
        if self.exec_state.active_turn_id() != Some(turn_id) {
            return;
        }
        self.exec_state = ExecState::Idle;
        self.runtime.status.task.clear();
        self.content_panel_mut().finalize_stream();
        self.input_panel_mut().clear_notice();
    }

    pub fn fail_turn(&mut self, turn_id: Option<TurnId>, message: String) {
        if turn_id.is_some() {
            self.content_panel_mut().finalize_stream();
        }
        self.exec_state = ExecState::Blocked {
            turn_id,
            reason: message.clone(),
        };
        self.runtime.status.task = "blocked".into();
        self.content_panel_mut().push_error(message);
    }

    pub fn record_runtime_error(&mut self, message: String, agent: Option<&mut Agent>) {
        self.exec_state = ExecState::Idle;
        self.runtime.status.task.clear();
        self.content_panel_mut().finalize_stream();
        self.content_panel_mut().push_error(message.clone());
        // Persist for history restore when the agent is recoverable.
        if let Some(agent) = agent {
            let _ = agent.record_system_note(crate::agent::SystemNoteKind::Error, &message);
        }
        self.show_notice(message);
    }

    pub fn mark_cancel_requested(&mut self, turn_id: TurnId) {
        self.exec_state = ExecState::Cancelling { turn_id };
        self.runtime.status.task = task_label(&self.exec_state).to_string();
        self.input_panel_mut().set_notice("interrupt requested");
    }

    pub fn begin_tool_approval(&mut self, turn_id: TurnId, request_id: String, summary: String) {
        self.exec_state = ExecState::AwaitingApproval {
            turn_id,
            request: crate::ui::events::ApprovalPrompt {
                request_id,
                summary: summary.clone(),
            },
        };
        self.runtime.status.task = task_label(&self.exec_state).to_string();
        self.overlays.open(OverlayState {
            id: OverlayId::Dialog,
            title: "Approval Required".into(),
            body: format!("Approve tool call:\n\n{summary}\n\nCtrl+Y approve\nCtrl+N reject"),
            z_index: 10,
            slot: WindowSlot::Center,
            modal: true,
            capture: true,
        });
    }

    pub fn resolve_tool_approval(&mut self) {
        if let ExecState::AwaitingApproval { turn_id, .. } = self.exec_state {
            self.exec_state = ExecState::Streaming { turn_id };
            self.runtime.status.task = task_label(&self.exec_state).to_string();
            self.overlays.close(OverlayId::Dialog);
            self.input_panel_mut().clear_notice();
        }
    }

    pub fn apply_slash_command(&mut self, command: &str, agent: &mut Agent) -> Vec<Effect> {
        self.content_panel_mut().push_user(command.to_string());
        // Persist the slash command so it is restored in history after a restart.
        let _ = agent.record_system_note(crate::agent::SystemNoteKind::SlashCommand, command);
        let mut effects: Vec<Effect> = Vec::new();
        match parse_slash_command(command) {
            SlashCommand::Exit => {}
            SlashCommand::Model(None) => {
                self.content_panel_mut()
                    .push_assistant(format!("Current model: {}", agent.model_name()));
                self.content_panel_mut()
                    .push_assistant("Usage: /model pro | /model flash".into());
            }
            SlashCommand::Model(Some(model)) => {
                let resolved = match model.as_str() {
                    "pro" | "deepseek-v4-pro" => Some("deepseek-v4-pro"),
                    "flash" | "deepseek-v4-flash" => Some("deepseek-v4-flash"),
                    _ => None,
                };
                if let Some(model_name) = resolved {
                    agent.set_model(model_name.to_string());
                    self.content_panel_mut()
                        .push_assistant(format!("Switched to model: {model_name}"));
                } else {
                    self.content_panel_mut()
                        .push_error(format!("Unknown model: {model}"));
                }
            }
            SlashCommand::Plan(toggle) => {
                let mode = match toggle.as_deref() {
                    Some("on") | Some("enable") => AgentMode::Plan,
                    Some("off") | Some("disable") => AgentMode::Normal,
                    None | Some("toggle") => {
                        if agent.mode() == AgentMode::Plan {
                            AgentMode::Normal
                        } else {
                            AgentMode::Plan
                        }
                    }
                    Some(other) => {
                        self.content_panel_mut()
                            .push_error(format!("Unknown /plan argument: {other}"));
                        self.input_panel_mut().clear_notice();
                        return Vec::new();
                    }
                };
                agent.set_mode(mode);
                let label = mode_label(mode);
                self.content_panel_mut().push_assistant(format!(
                    "Mode: {label} | tools: {}",
                    agent.tool_names().join(", ")
                ));
            }
            SlashCommand::Compact => match agent.start_interactive_compact() {
                Ok(()) => {
                    self.saved_main_entries = Some(self.content_panel_mut().save_and_clear());
                    self.content_panel_mut().push_assistant(
                        "Compact session — generating initial summary, then edit and /commit or /abort."
                            .into(),
                    );
                    self.show_notice("compact — /commit or /abort".into());
                    // Auto-generate the initial summary as a compact turn so the
                    // user has content to review before requesting changes.
                    let turn_id = self.allocate_turn_id();
                    effects.push(Effect::StartTurn {
                        turn_id,
                        input: "Generate the initial compressed summary.".into(),
                    });
                }
                Err(e) => {
                    self.content_panel_mut()
                        .push_error(format!("Cannot start interactive compact: {e}"));
                }
            },
            SlashCommand::CommitCompact => {
                if !agent.is_in_interactive_compact() {
                    self.content_panel_mut().push_error(
                        "Not in an interactive compact session. Use /compact first.".into(),
                    );
                } else {
                    match agent.commit_interactive_compact() {
                        Ok(entry) => {
                            if let Some(saved) = self.saved_main_entries.take() {
                                self.content_panel_mut().restore_entries(saved);
                            }
                            self.content_panel_mut().push_assistant(format!(
                                "Compact committed. {}",
                                entry.operation_digest
                            ));
                            self.show_notice(String::new());
                        }
                        Err(e) => {
                            self.content_panel_mut()
                                .push_error(format!("Commit failed: {e}"));
                        }
                    }
                }
            }
            SlashCommand::AbortCompact => {
                if !agent.is_in_interactive_compact() {
                    self.content_panel_mut().push_error(
                        "Not in an interactive compact session. Use /compact first.".into(),
                    );
                } else {
                    match agent.abort_interactive_compact() {
                        Ok(()) => {
                            if let Some(saved) = self.saved_main_entries.take() {
                                self.content_panel_mut().restore_entries(saved);
                            }
                            self.content_panel_mut()
                                .push_assistant("Compact aborted.".into());
                            self.show_notice(String::new());
                        }
                        Err(e) => {
                            self.content_panel_mut()
                                .push_error(format!("Abort failed: {e}"));
                        }
                    }
                }
            }
            SlashCommand::Unknown => {
                self.content_panel_mut()
                    .push_error(format!("Unknown command: {command}"));
            }
        }
        self.input_panel_mut().clear_notice();
        effects
    }

    pub fn apply_effect(&mut self, effect: &Effect) {
        match effect {
            Effect::ShowNotice(notice) => self.show_notice(notice.clone()),
            Effect::OpenOverlay(state) => self.overlays.open(state.clone()),
            Effect::CloseOverlay(id) => self.overlays.close(*id),
            Effect::SwitchContentView(view) => {
                self.show_notice(format!("content view {:?} not implemented", view));
            }
            Effect::ApproveTool(request) => {
                self.show_notice(format!("approved: {}", request.summary));
            }
            Effect::RejectTool(request) => {
                self.show_notice(format!("rejected: {}", request.summary));
            }
            Effect::SaveSession
            | Effect::RestoreTerminal
            | Effect::Exit
            | Effect::StartTurn { .. }
            | Effect::CancelTurn(_)
            | Effect::HandleSlashCommand(_) => {}
        }
    }

    pub fn take_pending_input(&mut self) -> Option<String> {
        self.input_panel_mut().take_pending()
    }

    pub fn active_turn_id(&self) -> Option<TurnId> {
        self.exec_state.active_turn_id()
    }

    fn route_global_key(&mut self, key: &KeyAction) -> Option<Vec<Effect>> {
        if let ExecState::AwaitingApproval { request, .. } = &self.exec_state {
            if *key == KeyAction::CtrlY {
                return Some(vec![Effect::ApproveTool(request.clone())]);
            }
            if *key == KeyAction::CtrlN {
                return Some(vec![Effect::RejectTool(request.clone())]);
            }
        }

        match key {
            KeyAction::CtrlD => {
                if self.exec_state.is_busy() {
                    self.show_notice("Press Ctrl+C to cancel first".into());
                    return Some(Vec::new());
                }
                Some(vec![Effect::Exit])
            }
            KeyAction::CtrlT => {
                self.sidebar_visible = !self.sidebar_visible;
                self.normalize_focus();
                Some(Vec::new())
            }
            KeyAction::CtrlSlash => {
                self.toggle_help_overlay();
                Some(Vec::new())
            }
            KeyAction::Esc => {
                if let Some(id) = self.overlays.close_top() {
                    return Some(vec![Effect::CloseOverlay(id)]);
                }
                Some(Vec::new())
            }
            _ => None,
        }
    }

    fn route_cancel_intent(&mut self, key: &KeyAction) -> Option<Vec<Effect>> {
        if *key != KeyAction::CtrlC {
            return None;
        }

        match self.exec_state.clone() {
            ExecState::Idle if self.window_manager.focus() == PanelId::Input => {
                if self.input_panel_mut().is_empty() {
                    self.show_notice("Ctrl+D exits".into());
                } else {
                    self.input_panel_mut().clear_input();
                    self.show_notice("input cleared".into());
                }
                Some(Vec::new())
            }
            ExecState::Streaming { turn_id } => {
                self.mark_cancel_requested(turn_id);
                Some(vec![Effect::CancelTurn(turn_id)])
            }
            ExecState::AwaitingApproval { turn_id, request } => Some(vec![
                Effect::RejectTool(request),
                Effect::CancelTurn(turn_id),
            ]),
            ExecState::ToolRunning { turn_id, .. } => {
                self.show_notice("cancelling running tool".into());
                Some(vec![Effect::CancelTurn(turn_id)])
            }
            ExecState::Blocked { turn_id, .. } => {
                let mut effects = Vec::new();
                if let Some(id) = self.overlays.close_top() {
                    effects.push(Effect::CloseOverlay(id));
                }
                if let Some(turn_id) = turn_id {
                    effects.push(Effect::CancelTurn(turn_id));
                }
                Some(effects)
            }
            ExecState::Cancelling { .. } | ExecState::Idle => Some(Vec::new()),
        }
    }

    fn rewrite_panel_effects(&mut self, effects: &mut Vec<Effect>) {
        for effect in effects.iter_mut() {
            if let Effect::StartTurn { turn_id, input } = effect {
                if input == "/exit" || input == "/quit" {
                    *effect = Effect::Exit;
                    continue;
                }
                if input.starts_with("/model")
                    || input.starts_with("/plan")
                    || input == "/compact"
                    || input == "/commit"
                    || input == "/abort"
                {
                    *effect = Effect::HandleSlashCommand(input.clone());
                    continue;
                }
                *turn_id = self.allocate_turn_id();
            }
        }
    }

    pub fn allocate_turn_id(&mut self) -> TurnId {
        let turn_id = self.next_turn_id;
        self.next_turn_id = self.next_turn_id.saturating_add(1);
        turn_id
    }

    fn ui_context(&self) -> UiContext<'_> {
        UiContext {
            exec_state: &self.exec_state,
            runtime: &self.runtime,
            sidebar_visible: self.sidebar_visible,
        }
    }

    fn normalize_focus(&mut self) {
        let exec_state = self.exec_state.clone();
        let runtime = self.runtime.clone();
        self.window_manager.normalize_focus(&UiContext {
            exec_state: &exec_state,
            runtime: &runtime,
            sidebar_visible: self.sidebar_visible,
        });
    }

    fn content_panel_mut(&mut self) -> &mut ContentPanel {
        self.window_manager
            .panel_mut(PanelId::Content)
            .and_then(PanelNode::as_content_mut)
            .expect("content panel missing")
    }

    fn input_panel_mut(&mut self) -> &mut InputPanel {
        self.window_manager
            .panel_mut(PanelId::Input)
            .and_then(PanelNode::as_input_mut)
            .expect("input panel missing")
    }

    fn show_notice(&mut self, notice: String) {
        self.input_panel_mut().set_notice(notice);
    }

    fn toggle_help_overlay(&mut self) {
        if self.overlays.is_open(OverlayId::Help) {
            self.overlays.close(OverlayId::Help);
            return;
        }

        self.overlays.open(OverlayState {
            id: OverlayId::Help,
            title: "Keymap".into(),
            body: help_overlay_text().into(),
            z_index: 50,
            slot: WindowSlot::Center,
            modal: true,
            capture: true,
        });
    }

    #[cfg(test)]
    pub(crate) fn rendered_content_text_for_test(
        &mut self,
        visible: usize,
        width: usize,
    ) -> Vec<String> {
        let exec_state = self.exec_state.clone();
        self.content_panel_mut()
            .rendered_text_for_test(visible, width, &exec_state)
    }
}

enum SlashCommand {
    Exit,
    Model(Option<String>),
    Plan(Option<String>),
    Compact,
    CommitCompact,
    AbortCompact,
    Unknown,
}

fn parse_slash_command(command: &str) -> SlashCommand {
    if command == "/exit" || command == "/quit" {
        return SlashCommand::Exit;
    }
    if command == "/compact" {
        return SlashCommand::Compact;
    }
    if command == "/commit" {
        return SlashCommand::CommitCompact;
    }
    if command == "/abort" {
        return SlashCommand::AbortCompact;
    }
    if command.starts_with("/model") {
        let model = command.split_whitespace().nth(1).map(str::to_string);
        return SlashCommand::Model(model);
    }
    if command.starts_with("/plan") {
        let toggle = command
            .split_whitespace()
            .nth(1)
            .map(|item| item.to_lowercase());
        return SlashCommand::Plan(toggle);
    }
    SlashCommand::Unknown
}

fn mode_label(mode: AgentMode) -> &'static str {
    match mode {
        AgentMode::Normal => "build",
        AgentMode::Plan => "plan",
        AgentMode::InteractiveCompact => "compact",
    }
}

fn task_label(exec_state: &ExecState) -> &'static str {
    match exec_state {
        ExecState::Idle => "",
        ExecState::Streaming { .. } => "thinking...",
        ExecState::AwaitingApproval { .. } => "awaiting approval",
        ExecState::ToolRunning { .. } => "tool running",
        ExecState::Cancelling { .. } => "cancelling...",
        ExecState::Blocked { .. } => "blocked",
    }
}

fn help_overlay_text() -> &'static str {
    "Ctrl+D  exit\nCtrl+C  cancel intent\nCtrl+T  toggle sidebar\nCtrl+/  toggle help\nCtrl+Y  approve tool\nCtrl+N  reject tool\nTab     next focus\nShift+Tab previous focus\n\nInput:\nEnter submit\nShift+Enter newline\nCtrl+A/Ctrl+E move\nCtrl+U clear\n\nContent:\nUp/Down scroll\nPageUp/PageDown page\nHome top\nEnd bottom"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{BuildSystem, Language};

    fn profile() -> ProjectProfile {
        ProjectProfile {
            primary: Language::Rust,
            build_system: BuildSystem::Cargo,
            all_languages: vec![Language::Rust],
            test_framework: None,
            root: std::path::PathBuf::from("."),
            source_dirs: vec![std::path::PathBuf::from("src")],
            is_workspace: false,
            workspace_members: Vec::new(),
        }
    }

    #[test]
    fn ctrl_c_shows_exit_notice_when_idle_input_empty() {
        let mut shell = AppShell::new(&profile(), None, false, "auto", Vec::new());
        let effects = shell.dispatch_key(KeyAction::CtrlC);
        assert!(effects.is_empty());
        assert_eq!(shell.input_panel_mut().notice_text(), Some("Ctrl+D exits"));
    }

    #[test]
    fn ctrl_c_cancels_active_turn() {
        let mut shell = AppShell::new(&profile(), None, false, "auto", Vec::new());
        shell.start_turn(7, "hello");

        let effects = shell.dispatch_key(KeyAction::CtrlC);
        assert_eq!(effects, vec![Effect::CancelTurn(7)]);
        assert!(matches!(
            shell.exec_state,
            ExecState::Cancelling { turn_id: 7 }
        ));
    }

    #[test]
    fn modal_overlay_blocks_global_key_routing() {
        let mut shell = AppShell::new(&profile(), None, false, "auto", Vec::new());
        shell.apply_effect(&Effect::OpenOverlay(OverlayState {
            id: OverlayId::Dialog,
            title: "Modal".into(),
            body: "Body".into(),
            z_index: 10,
            slot: WindowSlot::Center,
            modal: true,
            capture: true,
        }));

        let effects = shell.dispatch_key(KeyAction::CtrlT);
        assert!(effects.is_empty());
        assert!(!shell.sidebar_visible);
    }

    #[test]
    fn show_notice_updates_input_panel_state() {
        let mut shell = AppShell::new(&profile(), None, false, "auto", Vec::new());
        shell.apply_effect(&Effect::ShowNotice("still streaming".into()));

        let input = shell.input_panel_mut();
        assert_eq!(input.notice_text(), Some("still streaming"));
    }

    #[test]
    fn ctrl_d_exits_when_idle() {
        let mut shell = AppShell::new(&profile(), None, false, "auto", Vec::new());
        let effects = shell.dispatch_key(KeyAction::CtrlD);
        assert_eq!(effects, vec![Effect::Exit]);
    }

    #[test]
    fn ctrl_slash_toggles_help_overlay() {
        let mut shell = AppShell::new(&profile(), None, false, "auto", Vec::new());
        assert!(shell.dispatch_key(KeyAction::CtrlSlash).is_empty());
        assert!(shell.overlays.is_open(OverlayId::Help));
        let effects = shell.dispatch_key(KeyAction::CtrlSlash);
        assert_eq!(effects, vec![Effect::CloseOverlay(OverlayId::Help)]);
        shell.apply_effect(&effects[0]);
        assert!(!shell.overlays.is_open(OverlayId::Help));
    }
}
