use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton as CrosstermMouseButton, MouseEvent,
    MouseEventKind,
};

pub type TurnId = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PanelId {
    Content,
    Input,
    Sidebar,
    StatusBar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OverlayId {
    Dialog,
    Help,
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
pub enum ContentView {
    Chat,
    Diff,
    FilePreview,
    Diagnostics,
    ToolLog,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OverlayState {
    pub id: OverlayId,
    pub title: String,
    pub body: String,
    pub z_index: i16,
    pub slot: WindowSlot,
    pub modal: bool,
    pub capture: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyAction {
    Char(char),
    Enter,
    AltEnter,
    ShiftEnter,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    Esc,
    Tab,
    BackTab,
    CtrlA,
    CtrlC,
    CtrlD,
    CtrlE,
    CtrlL,
    CtrlSlash,
    CtrlT,
    CtrlU,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseAction {
    Down {
        button: MouseButton,
        column: u16,
        row: u16,
    },
    ScrollUp {
        column: u16,
        row: u16,
    },
    ScrollDown {
        column: u16,
        row: u16,
    },
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
            KeyCode::Char('d') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(KeyAction::CtrlD)
            }
            KeyCode::Char('a') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(KeyAction::CtrlA)
            }
            KeyCode::Char('e') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(KeyAction::CtrlE)
            }
            KeyCode::Char('l') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(KeyAction::CtrlL)
            }
            KeyCode::Char('u') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(KeyAction::CtrlU)
            }
            KeyCode::Char('/') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(KeyAction::CtrlSlash)
            }
            KeyCode::Char('_') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(KeyAction::CtrlSlash)
            }
            KeyCode::Char('7') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(KeyAction::CtrlSlash)
            }
            KeyCode::Char('t') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(KeyAction::CtrlT)
            }
            KeyCode::Char(ch)
                if event.modifiers.is_empty() || event.modifiers == KeyModifiers::SHIFT =>
            {
                Some(KeyAction::Char(ch))
            }
            KeyCode::Enter if event.modifiers.contains(KeyModifiers::ALT) => {
                Some(KeyAction::AltEnter)
            }
            KeyCode::Enter if event.modifiers.contains(KeyModifiers::SHIFT) => {
                Some(KeyAction::ShiftEnter)
            }
            KeyCode::Enter => Some(KeyAction::Enter),
            KeyCode::Backspace => Some(KeyAction::Backspace),
            KeyCode::Delete => Some(KeyAction::Delete),
            KeyCode::Up => Some(KeyAction::Up),
            KeyCode::Down => Some(KeyAction::Down),
            KeyCode::Left => Some(KeyAction::Left),
            KeyCode::Right => Some(KeyAction::Right),
            KeyCode::PageUp => Some(KeyAction::PageUp),
            KeyCode::PageDown => Some(KeyAction::PageDown),
            KeyCode::Home => Some(KeyAction::Home),
            KeyCode::End => Some(KeyAction::End),
            KeyCode::Esc => Some(KeyAction::Esc),
            KeyCode::Tab => Some(KeyAction::Tab),
            KeyCode::BackTab => Some(KeyAction::BackTab),
            _ => None,
        }
    }
}

impl MouseAction {
    pub fn from_mouse_event(event: MouseEvent) -> Option<Self> {
        match event.kind {
            MouseEventKind::Down(button) => Some(MouseAction::Down {
                button: map_mouse_button(button)?,
                column: event.column,
                row: event.row,
            }),
            MouseEventKind::ScrollUp => Some(MouseAction::ScrollUp {
                column: event.column,
                row: event.row,
            }),
            MouseEventKind::ScrollDown => Some(MouseAction::ScrollDown {
                column: event.column,
                row: event.row,
            }),
            _ => None,
        }
    }
}

fn map_mouse_button(button: CrosstermMouseButton) -> Option<MouseButton> {
    match button {
        CrosstermMouseButton::Left => Some(MouseButton::Left),
        CrosstermMouseButton::Right => Some(MouseButton::Right),
        CrosstermMouseButton::Middle => Some(MouseButton::Middle),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecState {
    Idle,
    Streaming {
        turn_id: TurnId,
    },
    AwaitingApproval {
        turn_id: TurnId,
        request: String,
    },
    ToolRunning {
        turn_id: TurnId,
        tool_name: String,
    },
    Cancelling {
        turn_id: TurnId,
    },
    Blocked {
        turn_id: Option<TurnId>,
        reason: String,
    },
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
    ApproveTool(String),
    RejectTool(String),
    SwitchContentView(ContentView),
    OpenOverlay(OverlayState),
    CloseOverlay(OverlayId),
    SaveSession,
    RestoreTerminal,
    HandleSlashCommand(String),
    ShowNotice(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DispatchResult {
    Ignored,
    Consumed(Vec<Effect>),
}
