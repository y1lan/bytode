use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

pub type TurnId = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PanelId {
    Content,
    Input,
    Sidebar,
    StatusBar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowSlot {
    Left,
    Right,
    Top,
    Bottom,
    Content,
    Center,
    AroundInput,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowSpec {
    pub id: PanelId,
    pub z_index: i16,
    pub slot: WindowSlot,
    pub size: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyAction {
    Char(char),
    Enter,
    ShiftEnter,
    Backspace,
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Tab,
    BackTab,
    CtrlC,
    CtrlT,
}

impl KeyAction {
    pub fn from_key_event(event: KeyEvent) -> Option<Self> {
        if event.kind != KeyEventKind::Press {
            return None;
        }

        match event.code {
            KeyCode::Char('c') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(KeyAction::CtrlC)
            }
            KeyCode::Char('t') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(KeyAction::CtrlT)
            }
            KeyCode::Char(ch) if event.modifiers.is_empty() || event.modifiers == KeyModifiers::SHIFT => {
                Some(KeyAction::Char(ch))
            }
            KeyCode::Enter if event.modifiers.contains(KeyModifiers::SHIFT) => {
                Some(KeyAction::ShiftEnter)
            }
            KeyCode::Enter => Some(KeyAction::Enter),
            KeyCode::Backspace => Some(KeyAction::Backspace),
            KeyCode::Up => Some(KeyAction::Up),
            KeyCode::Down => Some(KeyAction::Down),
            KeyCode::Left => Some(KeyAction::Left),
            KeyCode::Right => Some(KeyAction::Right),
            KeyCode::PageUp => Some(KeyAction::PageUp),
            KeyCode::PageDown => Some(KeyAction::PageDown),
            KeyCode::Tab => Some(KeyAction::Tab),
            KeyCode::BackTab => Some(KeyAction::BackTab),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecState {
    Idle,
    Streaming { turn_id: TurnId },
    AwaitingApproval { turn_id: TurnId, request: String },
    ToolRunning { turn_id: TurnId, tool_name: String },
    Cancelling { turn_id: TurnId },
    Blocked { turn_id: Option<TurnId>, reason: String },
}

impl ExecState {
    pub fn active_turn_id(&self) -> Option<TurnId> {
        match self {
            ExecState::Idle => None,
            ExecState::Streaming { turn_id }
            | ExecState::AwaitingApproval { turn_id, .. }
            | ExecState::ToolRunning { turn_id, .. }
            | ExecState::Cancelling { turn_id } => Some(*turn_id),
            ExecState::Blocked { turn_id, .. } => *turn_id,
        }
    }

    pub fn is_busy(&self) -> bool {
        !matches!(self, ExecState::Idle)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    Exit,
    StartTurn { turn_id: TurnId, input: String },
    CancelTurn(TurnId),
    HandleSlashCommand(String),
    ShowNotice(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DispatchResult {
    Ignored,
    Consumed(Vec<Effect>),
}
