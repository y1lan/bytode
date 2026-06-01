use crate::ui::events::{DispatchResult, Effect, KeyAction, PanelId, WindowSlot};
use crate::ui::panels::{PanelContext, PanelNode, RenderContext, UiContext};
use ratatui::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FocusState {
    pub current: PanelId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WindowPlacement {
    id: PanelId,
    z_index: i16,
    area: Rect,
}

pub struct WindowManager {
    panels: Vec<PanelNode>,
    focus: FocusState,
}

impl WindowManager {
    pub fn new(panels: Vec<PanelNode>) -> Self {
        Self {
            panels,
            focus: FocusState {
                current: PanelId::Input,
            },
        }
    }

    pub fn focus(&self) -> PanelId {
        self.focus.current
    }

    pub fn focusable_windows(&self, ctx: &UiContext<'_>) -> Vec<PanelId> {
        let mut windows = self
            .panels
            .iter()
            .filter(|panel| panel.visible(ctx) && panel.focusable(ctx))
            .map(|panel| {
                let spec = panel.window_spec(ctx);
                (panel.id(), spec.z_index, focus_rank(panel.id()))
            })
            .collect::<Vec<_>>();
        windows.sort_by_key(|(_, z_index, focus)| (std::cmp::Reverse(*z_index), *focus));
        windows.into_iter().map(|(id, _, _)| id).collect()
    }

    pub fn normalize_focus(&mut self, ctx: &UiContext<'_>) {
        for panel in &mut self.panels {
            panel.normalize(ctx);
        }

        let focusable = self.focusable_windows(ctx);
        if focusable.contains(&self.focus.current) {
            return;
        }

        self.focus.current = if focusable.contains(&PanelId::Input) {
            PanelId::Input
        } else {
            focusable.first().copied().unwrap_or(PanelId::Content)
        };
    }

    pub fn cycle_focus(&mut self, reverse: bool, ctx: &UiContext<'_>) {
        let focusable = self.focusable_windows(ctx);
        if focusable.is_empty() {
            return;
        }

        let index = focusable
            .iter()
            .position(|id| *id == self.focus.current)
            .unwrap_or(0);

        let next = if reverse {
            if index == 0 {
                focusable.len() - 1
            } else {
                index - 1
            }
        } else {
            (index + 1) % focusable.len()
        };
        self.focus.current = focusable[next];
    }

    pub fn dispatch_key(&mut self, key: KeyAction, ctx: &UiContext<'_>) -> Vec<Effect> {
        self.normalize_focus(ctx);

        match key {
            KeyAction::Tab => {
                self.cycle_focus(false, ctx);
                return Vec::new();
            }
            KeyAction::BackTab => {
                self.cycle_focus(true, ctx);
                return Vec::new();
            }
            _ => {}
        }

        let panel_ctx = PanelContext {
            exec_state: ctx.exec_state,
        };
        let Some(panel) = self
            .panels
            .iter_mut()
            .find(|panel| panel.id() == self.focus.current)
        else {
            return Vec::new();
        };

        match panel.handle_key(key, &panel_ctx) {
            DispatchResult::Ignored => Vec::new(),
            DispatchResult::Consumed(effects) => effects,
        }
    }

    pub fn render(&self, frame: &mut Frame, ctx: &RenderContext<'_>) {
        let placements = self.layout(frame.area(), &UiContext {
            exec_state: ctx.exec_state,
            runtime: ctx.runtime,
            sidebar_visible: ctx.sidebar_visible,
        });

        for placement in placements {
            if let Some(panel) = self.panels.iter().find(|panel| panel.id() == placement.id) {
                panel.render(frame, placement.area, ctx);
            }
        }
    }

    pub fn panel_mut(&mut self, id: PanelId) -> Option<&mut PanelNode> {
        self.panels.iter_mut().find(|panel| panel.id() == id)
    }

    fn layout(&self, area: Rect, ctx: &UiContext<'_>) -> Vec<WindowPlacement> {
        let mut visible_specs = self
            .panels
            .iter()
            .filter(|panel| panel.visible(ctx))
            .map(|panel| panel.window_spec(ctx))
            .collect::<Vec<_>>();
        visible_specs.sort_by_key(|spec| {
            (
                spec.z_index,
                slot_rank(spec.slot),
                slot_member_rank(spec.slot, spec.id),
            )
        });

        let mut placements = Vec::new();
        let mut current_layer = i16::MIN;
        let mut remaining = area;

        for spec in visible_specs {
            if spec.z_index != current_layer {
                current_layer = spec.z_index;
                remaining = area;
            }

            let window_area = match spec.slot {
                WindowSlot::Top => {
                    let height = spec.size.min(remaining.height);
                    let top = Rect::new(remaining.x, remaining.y, remaining.width, height);
                    remaining.y = remaining.y.saturating_add(height);
                    remaining.height = remaining.height.saturating_sub(height);
                    top
                }
                WindowSlot::Bottom => {
                    let height = spec.size.min(remaining.height);
                    let y = remaining
                        .y
                        .saturating_add(remaining.height.saturating_sub(height));
                    let bottom = Rect::new(remaining.x, y, remaining.width, height);
                    remaining.height = remaining.height.saturating_sub(height);
                    bottom
                }
                WindowSlot::Left => {
                    let width = spec.size.min(remaining.width);
                    let left = Rect::new(remaining.x, remaining.y, width, remaining.height);
                    remaining.x = remaining.x.saturating_add(width);
                    remaining.width = remaining.width.saturating_sub(width);
                    left
                }
                WindowSlot::Right => {
                    let width = spec.size.min(remaining.width);
                    let x = remaining
                        .x
                        .saturating_add(remaining.width.saturating_sub(width));
                    let right = Rect::new(x, remaining.y, width, remaining.height);
                    remaining.width = remaining.width.saturating_sub(width);
                    right
                }
                WindowSlot::Content => remaining,
                WindowSlot::Center => centered_rect(area, spec.size.max(20), spec.size.max(8)),
                WindowSlot::AroundInput => centered_rect(area, spec.size.max(20), spec.size.max(6)),
            };

            placements.push(WindowPlacement {
                id: spec.id,
                z_index: spec.z_index,
                area: window_area,
            });
        }

        placements.sort_by_key(|placement| placement.z_index);
        placements
    }
}

fn slot_rank(slot: WindowSlot) -> u8 {
    match slot {
        WindowSlot::Top => 0,
        WindowSlot::Bottom => 1,
        WindowSlot::Left => 2,
        WindowSlot::Right => 3,
        WindowSlot::Content => 4,
        WindowSlot::Center => 5,
        WindowSlot::AroundInput => 6,
    }
}

fn slot_member_rank(slot: WindowSlot, id: PanelId) -> u8 {
    match (slot, id) {
        (WindowSlot::Bottom, PanelId::StatusBar) => 0,
        (WindowSlot::Bottom, PanelId::Input) => 1,
        _ => 0,
    }
}

fn focus_rank(id: PanelId) -> u8 {
    match id {
        PanelId::Input => 0,
        PanelId::Content => 1,
        PanelId::Sidebar => 2,
        PanelId::StatusBar => 3,
    }
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let popup_width = width.min(area.width);
    let popup_height = height.min(area.height);
    Rect::new(
        area.x + area.width.saturating_sub(popup_width) / 2,
        area.y + area.height.saturating_sub(popup_height) / 2,
        popup_width,
        popup_height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::events::{ExecState, PanelId};
    use crate::ui::panels::{ContentPanel, InputPanel, RuntimeSnapshot, SidebarPanel, StatusBarPanel};
    use crate::ui::statusbar::StatusBarState;

    fn runtime() -> RuntimeSnapshot {
        RuntimeSnapshot {
            tool_names: vec!["read_file".into()],
            status: StatusBarState {
                dir: String::new(),
                git_branch: None,
                git_dirty: false,
                task: String::new(),
                model: String::new(),
                ctx_used: 0,
                ctx_total: 1,
                tool_count: 0,
                session_cost: 0.0,
                session_calls: 0,
                mode: String::new(),
                elapsed: String::new(),
                spinner: ' ',
            },
            primary_language: "rust".into(),
            detection_source: "auto".into(),
        }
    }

    fn ui_context(sidebar_visible: bool) -> UiContext<'static> {
        let runtime = Box::leak(Box::new(runtime()));
        UiContext {
            exec_state: &ExecState::Idle,
            runtime,
            sidebar_visible,
        }
    }

    #[test]
    fn normalize_focus_skips_hidden_sidebar() {
        let mut manager = WindowManager::new(vec![
            PanelNode::Content(ContentPanel::new(Vec::new())),
            PanelNode::Input(InputPanel::new()),
            PanelNode::Sidebar(SidebarPanel::new()),
            PanelNode::StatusBar(StatusBarPanel::new()),
        ]);
        manager.focus = FocusState {
            current: PanelId::Sidebar,
        };

        manager.normalize_focus(&ui_context(false));
        assert_eq!(manager.focus(), PanelId::Input);
    }

    #[test]
    fn focus_cycle_includes_visible_sidebar_only() {
        let mut manager = WindowManager::new(vec![
            PanelNode::Content(ContentPanel::new(Vec::new())),
            PanelNode::Input(InputPanel::new()),
            PanelNode::Sidebar(SidebarPanel::new()),
            PanelNode::StatusBar(StatusBarPanel::new()),
        ]);

        manager.cycle_focus(false, &ui_context(true));
        assert_eq!(manager.focus(), PanelId::Content);
        manager.cycle_focus(false, &ui_context(false));
        assert_eq!(manager.focus(), PanelId::Input);
        manager.normalize_focus(&ui_context(false));
        assert_eq!(manager.focus(), PanelId::Input);
    }

    #[test]
    fn status_bar_stays_below_input() {
        let manager = WindowManager::new(vec![
            PanelNode::Content(ContentPanel::new(Vec::new())),
            PanelNode::Input(InputPanel::new()),
            PanelNode::Sidebar(SidebarPanel::new()),
            PanelNode::StatusBar(StatusBarPanel::new()),
        ]);

        let placements = manager.layout(Rect::new(0, 0, 100, 30), &ui_context(false));
        let input = placements
            .iter()
            .find(|placement| placement.id == PanelId::Input)
            .expect("input placement");
        let status = placements
            .iter()
            .find(|placement| placement.id == PanelId::StatusBar)
            .expect("status placement");

        assert_eq!(status.area.y + status.area.height, 30);
        assert_eq!(input.area.y + input.area.height, status.area.y);
    }
}
