mod content;
mod input;
mod sidebar;
mod status_bar;

pub use content::{ContentPanel, HistoryEntry};
pub use input::InputPanel;
pub use sidebar::SidebarPanel;
pub use status_bar::StatusBarPanel;

use crate::ui::events::{DispatchResult, ExecState, KeyAction, MouseAction, PanelId, WindowSpec};
use crate::ui::statusbar::StatusBarState;
use ratatui::prelude::*;

pub const BG: Color = Color::Rgb(239, 241, 245);
pub const BORDER: Color = Color::Rgb(188, 192, 204);
pub const TXT: Color = Color::Rgb(76, 79, 105);
pub const TXT_SUBTLE: Color = Color::Rgb(108, 111, 133);
pub const BLUE: Color = Color::Rgb(30, 102, 245);
pub const ORANGE: Color = Color::Rgb(223, 142, 29);
pub const CYAN: Color = Color::Rgb(32, 159, 181);
pub const GREEN: Color = Color::Rgb(64, 160, 43);
pub const RED: Color = Color::Rgb(210, 15, 57);

#[derive(Clone)]
pub struct RuntimeSnapshot {
    pub tool_names: Vec<String>,
    pub status: StatusBarState,
    pub primary_language: String,
    pub detection_source: String,
}

pub struct UiContext<'a> {
    pub exec_state: &'a ExecState,
    pub runtime: &'a RuntimeSnapshot,
    pub sidebar_visible: bool,
}

pub struct RenderContext<'a> {
    pub exec_state: &'a ExecState,
    pub runtime: &'a RuntimeSnapshot,
    pub sidebar_visible: bool,
    pub focused: Option<PanelId>,
}

pub struct PanelContext<'a> {
    pub exec_state: &'a ExecState,
}

pub enum PanelNode {
    Content(ContentPanel),
    Input(InputPanel),
    Sidebar(SidebarPanel),
    StatusBar(StatusBarPanel),
}

impl PanelNode {
    pub fn id(&self) -> PanelId {
        match self {
            PanelNode::Content(panel) => panel.id(),
            PanelNode::Input(panel) => panel.id(),
            PanelNode::Sidebar(panel) => panel.id(),
            PanelNode::StatusBar(panel) => panel.id(),
        }
    }

    pub fn visible(&self, ctx: &UiContext<'_>) -> bool {
        match self {
            PanelNode::Content(panel) => panel.visible(ctx),
            PanelNode::Input(panel) => panel.visible(ctx),
            PanelNode::Sidebar(panel) => panel.visible(ctx),
            PanelNode::StatusBar(panel) => panel.visible(ctx),
        }
    }

    pub fn focusable(&self, ctx: &UiContext<'_>) -> bool {
        match self {
            PanelNode::Content(panel) => panel.focusable(ctx),
            PanelNode::Input(panel) => panel.focusable(ctx),
            PanelNode::Sidebar(panel) => panel.focusable(ctx),
            PanelNode::StatusBar(panel) => panel.focusable(ctx),
        }
    }

    pub fn window_spec(&self, ctx: &UiContext<'_>) -> WindowSpec {
        match self {
            PanelNode::Content(panel) => panel.window_spec(ctx),
            PanelNode::Input(panel) => panel.window_spec(ctx),
            PanelNode::Sidebar(panel) => panel.window_spec(ctx),
            PanelNode::StatusBar(panel) => panel.window_spec(ctx),
        }
    }

    pub fn handle_key(&mut self, key: KeyAction, ctx: &PanelContext<'_>) -> DispatchResult {
        match self {
            PanelNode::Content(panel) => panel.handle_key(key, ctx),
            PanelNode::Input(panel) => panel.handle_key(key, ctx),
            PanelNode::Sidebar(panel) => panel.handle_key(key, ctx),
            PanelNode::StatusBar(panel) => panel.handle_key(key, ctx),
        }
    }

    pub fn handle_mouse(
        &mut self,
        mouse: MouseAction,
        area: Rect,
        ctx: &PanelContext<'_>,
    ) -> DispatchResult {
        match self {
            PanelNode::Content(panel) => panel.handle_mouse(mouse, area, ctx),
            PanelNode::Input(panel) => panel.handle_mouse(mouse, area, ctx),
            PanelNode::Sidebar(panel) => panel.handle_mouse(mouse, area, ctx),
            PanelNode::StatusBar(panel) => panel.handle_mouse(mouse, area, ctx),
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, ctx: &RenderContext<'_>) {
        match self {
            PanelNode::Content(panel) => panel.render(frame, area, ctx),
            PanelNode::Input(panel) => panel.render(frame, area, ctx),
            PanelNode::Sidebar(panel) => panel.render(frame, area, ctx),
            PanelNode::StatusBar(panel) => panel.render(frame, area, ctx),
        }
    }

    pub fn normalize(&mut self, ctx: &UiContext<'_>) {
        if let PanelNode::Sidebar(panel) = self {
            panel.normalize(ctx);
        }
    }

    pub fn as_content_mut(&mut self) -> Option<&mut ContentPanel> {
        match self {
            PanelNode::Content(panel) => Some(panel),
            _ => None,
        }
    }

    pub fn as_input_mut(&mut self) -> Option<&mut InputPanel> {
        match self {
            PanelNode::Input(panel) => Some(panel),
            _ => None,
        }
    }
}

pub fn panel_block(title: &str, focused: bool) -> ratatui::widgets::Block<'static> {
    let border_style = if focused {
        Style::default().fg(BLUE)
    } else {
        Style::default().fg(BORDER)
    };
    ratatui::widgets::Block::default()
        .title(format!(" {title} "))
        .borders(ratatui::widgets::Borders::ALL)
        .border_style(border_style)
        .style(Style::default().bg(BG))
}
