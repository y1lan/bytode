use crate::ui::events::{DispatchResult, Effect, KeyAction, OverlayId, OverlayState, WindowSlot};
use crate::ui::panels::RenderContext;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

const BG: Color = Color::Rgb(239, 241, 245);
const TXT: Color = Color::Rgb(76, 79, 105);
const BLUE: Color = Color::Rgb(30, 102, 245);

pub struct OverlayContext;

enum OverlayNode {
    Dialog(DialogOverlay),
    Help(HelpOverlay),
}

impl OverlayNode {
    pub fn from_state(state: OverlayState) -> Self {
        match state.id {
            OverlayId::Dialog => OverlayNode::Dialog(DialogOverlay::from_state(state)),
            OverlayId::Help => OverlayNode::Help(HelpOverlay::from_state(state)),
        }
    }

    pub fn id(&self) -> OverlayId {
        match self {
            OverlayNode::Dialog(overlay) => overlay.id(),
            OverlayNode::Help(overlay) => overlay.id(),
        }
    }

    pub fn z_index(&self) -> i16 {
        match self {
            OverlayNode::Dialog(overlay) => overlay.z_index(),
            OverlayNode::Help(overlay) => overlay.z_index(),
        }
    }

    pub fn modal(&self) -> bool {
        match self {
            OverlayNode::Dialog(overlay) => overlay.modal(),
            OverlayNode::Help(overlay) => overlay.modal(),
        }
    }

    pub fn capture_key(&self, key: &KeyAction) -> bool {
        match self {
            OverlayNode::Dialog(overlay) => overlay.capture_key(key),
            OverlayNode::Help(overlay) => overlay.capture_key(key),
        }
    }

    pub fn handle_key(&mut self, key: KeyAction, ctx: &mut OverlayContext) -> DispatchResult {
        match self {
            OverlayNode::Dialog(overlay) => overlay.handle_key(key, ctx),
            OverlayNode::Help(overlay) => overlay.handle_key(key, ctx),
        }
    }

    pub fn render(&self, frame: &mut Frame, ctx: &RenderContext<'_>) {
        match self {
            OverlayNode::Dialog(overlay) => overlay.render(frame, ctx),
            OverlayNode::Help(overlay) => overlay.render(frame, ctx),
        }
    }
}

pub struct OverlayStack {
    overlays: Vec<OverlayNode>,
}

impl OverlayStack {
    pub fn new() -> Self {
        Self {
            overlays: Vec::new(),
        }
    }

    pub fn open(&mut self, state: OverlayState) {
        self.close(state.id);
        self.overlays.push(OverlayNode::from_state(state));
        self.overlays.sort_by_key(OverlayNode::z_index);
    }

    pub fn close(&mut self, id: OverlayId) {
        self.overlays.retain(|overlay| overlay.id() != id);
    }

    pub fn is_open(&self, id: OverlayId) -> bool {
        self.overlays.iter().any(|overlay| overlay.id() == id)
    }

    pub fn close_top(&mut self) -> Option<OverlayId> {
        self.overlays.pop().map(|overlay| overlay.id())
    }

    pub fn has_modal(&self) -> bool {
        self.overlays.iter().any(OverlayNode::modal)
    }

    pub fn handle_modal_key(&mut self, key: KeyAction) -> Option<Vec<Effect>> {
        let overlay = self
            .overlays
            .iter_mut()
            .rev()
            .find(|overlay| overlay.modal())?;
        Some(handle_overlay_key(overlay, key))
    }

    pub fn handle_capture_key(&mut self, key: KeyAction) -> Option<Vec<Effect>> {
        let overlay = self
            .overlays
            .iter_mut()
            .rev()
            .find(|overlay| !overlay.modal() && overlay.capture_key(&key))?;
        Some(handle_overlay_key(overlay, key))
    }

    pub fn render(&self, frame: &mut Frame, ctx: &RenderContext<'_>) {
        for overlay in &self.overlays {
            overlay.render(frame, ctx);
        }
    }
}

fn handle_overlay_key(overlay: &mut OverlayNode, key: KeyAction) -> Vec<Effect> {
    let mut ctx = OverlayContext;
    match overlay.handle_key(key, &mut ctx) {
        DispatchResult::Ignored => Vec::new(),
        DispatchResult::Consumed(effects) => effects,
    }
}

struct DialogOverlay {
    state: OverlayState,
}

struct HelpOverlay {
    state: OverlayState,
    scroll: usize,
}

impl HelpOverlay {
    fn from_state(state: OverlayState) -> Self {
        Self { state, scroll: 0 }
    }

    fn id(&self) -> OverlayId {
        self.state.id
    }

    fn z_index(&self) -> i16 {
        self.state.z_index
    }

    fn modal(&self) -> bool {
        self.state.modal
    }

    fn capture_key(&self, key: &KeyAction) -> bool {
        matches!(
            key,
            KeyAction::Tab
                | KeyAction::BackTab
                | KeyAction::Up
                | KeyAction::Down
                | KeyAction::PageUp
                | KeyAction::PageDown
                | KeyAction::Esc
                | KeyAction::CtrlSlash
                | KeyAction::Char('q')
                | KeyAction::Enter
        )
    }

    fn handle_key(&mut self, key: KeyAction, _ctx: &mut OverlayContext) -> DispatchResult {
        match key {
            KeyAction::CtrlSlash | KeyAction::Esc | KeyAction::Char('q') => {
                DispatchResult::Consumed(vec![Effect::CloseOverlay(self.state.id)])
            }
            KeyAction::Up => {
                self.scroll = self.scroll.saturating_sub(1);
                DispatchResult::Consumed(Vec::new())
            }
            KeyAction::Down => {
                self.scroll = self.scroll.saturating_add(1);
                DispatchResult::Consumed(Vec::new())
            }
            KeyAction::PageUp => {
                self.scroll = self.scroll.saturating_sub(8);
                DispatchResult::Consumed(Vec::new())
            }
            KeyAction::PageDown => {
                self.scroll = self.scroll.saturating_add(8);
                DispatchResult::Consumed(Vec::new())
            }
            KeyAction::Tab | KeyAction::BackTab | KeyAction::Enter => {
                DispatchResult::Consumed(Vec::new())
            }
            _ => DispatchResult::Ignored,
        }
    }

    fn render(&self, frame: &mut Frame, _ctx: &RenderContext<'_>) {
        let area = centered_rect(frame.area(), 76, 18);
        frame.render_widget(Clear, area);
        let block = Block::default()
            .title(format!(" {} ", self.state.title))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(BLUE))
            .style(Style::default().bg(BG));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let lines = self
            .state
            .body
            .lines()
            .skip(self.scroll)
            .map(Line::from)
            .collect::<Vec<_>>();
        let paragraph = Paragraph::new(lines)
            .style(Style::default().fg(TXT))
            .wrap(Wrap { trim: false });
        frame.render_widget(paragraph, inner);
    }
}

impl DialogOverlay {
    fn from_state(state: OverlayState) -> Self {
        Self { state }
    }

    fn id(&self) -> OverlayId {
        self.state.id
    }

    fn z_index(&self) -> i16 {
        self.state.z_index
    }

    fn modal(&self) -> bool {
        self.state.modal
    }

    fn capture_key(&self, _key: &KeyAction) -> bool {
        self.state.capture
    }

    fn handle_key(&mut self, key: KeyAction, _ctx: &mut OverlayContext) -> DispatchResult {
        match key {
            KeyAction::Enter => DispatchResult::Consumed(vec![Effect::CloseOverlay(self.state.id)]),
            _ => DispatchResult::Ignored,
        }
    }

    fn render(&self, frame: &mut Frame, _ctx: &RenderContext<'_>) {
        let area = overlay_rect(frame.area(), self.state.slot);
        frame.render_widget(Clear, area);

        let block = Block::default()
            .title(format!(" {} ", self.state.title))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(BLUE))
            .style(Style::default().bg(BG));
        let paragraph = Paragraph::new(self.state.body.clone())
            .block(block)
            .style(Style::default().fg(TXT))
            .wrap(Wrap { trim: false });
        frame.render_widget(paragraph, area);
    }
}

fn overlay_rect(area: Rect, slot: WindowSlot) -> Rect {
    match slot {
        WindowSlot::AroundInput => centered_rect(area, 72, 8),
        _ => centered_rect(area, 72, 12),
    }
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::events::{KeyAction, OverlayId, OverlayState, WindowSlot};

    #[test]
    fn modal_overlay_routes_first() {
        let mut stack = OverlayStack::new();
        stack.open(OverlayState {
            id: OverlayId::Dialog,
            title: "Test".into(),
            body: "Body".into(),
            z_index: 10,
            slot: WindowSlot::Center,
            modal: true,
            capture: true,
        });

        let effects = stack.handle_modal_key(KeyAction::Enter).expect("modal effects");
        assert_eq!(effects, vec![Effect::CloseOverlay(OverlayId::Dialog)]);
    }
}
