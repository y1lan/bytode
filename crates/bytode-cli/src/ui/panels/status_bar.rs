use crate::ui::events::{DispatchResult, KeyAction, MouseAction, PanelId, WindowSlot, WindowSpec};
use crate::ui::panels::{PanelContext, RenderContext, UiContext};
use crate::ui::statusbar;
use ratatui::prelude::*;

pub struct StatusBarPanel;

impl StatusBarPanel {
    pub fn new() -> Self {
        Self
    }

    pub fn id(&self) -> PanelId {
        PanelId::StatusBar
    }

    pub fn visible(&self, _ctx: &UiContext<'_>) -> bool {
        true
    }

    pub fn focusable(&self, _ctx: &UiContext<'_>) -> bool {
        false
    }

    pub fn window_spec(&self, _ctx: &UiContext<'_>) -> WindowSpec {
        WindowSpec {
            id: self.id(),
            z_index: 0,
            slot: WindowSlot::Bottom,
            size: 1,
        }
    }

    pub fn handle_key(&mut self, _key: KeyAction, _ctx: &PanelContext<'_>) -> DispatchResult {
        DispatchResult::Ignored
    }

    pub fn handle_mouse(
        &mut self,
        _mouse: MouseAction,
        _area: Rect,
        _ctx: &PanelContext<'_>,
    ) -> DispatchResult {
        DispatchResult::Ignored
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, ctx: &RenderContext<'_>) {
        statusbar::render(frame, area, &ctx.runtime.status);
    }
}
