//! Chat view component with native terminal scrollback
//!
//! Uses a "hot zone" approach: recent content is rendered in the viewport,
//! and as it overflows, lines are committed to the terminal's native
//! scrollback buffer via `insert_before()`. This provides O(active turns)
//! rendering instead of O(entire conversation).

use std::collections::{HashSet, VecDeque};
use std::io::Stdout;

use ratatui::{
    backend::CrosstermBackend,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Padding, Paragraph, Widget},
    Terminal,
};

use crate::transcript::{Role, Transcript, Turn};

/// Chat view with native scrollback support.
/// 
/// The "hot zone" is a sliding window of lines that are actively rendered
/// in the viewport. When new content causes overflow, the oldest lines are
/// committed to the terminal's native scrollback buffer.
#[derive(Debug)]
pub struct ChatView {
    /// Lines currently in the hot zone (re-renderable)
    lines: VecDeque<Line<'static>>,
    /// Maximum lines before overflow commits to scrollback
    max_lines: usize,
    /// Lines committed from active turns (not frozen ones)
    committed_count: usize,
    /// Turn IDs fully committed to scrollback - never re-render these
    frozen_turn_ids: HashSet<usize>,
    /// Width used for last render (to detect resize)
    last_width: u16,
}

impl ChatView {
    pub fn new(max_lines: usize) -> Self {
        Self {
            lines: VecDeque::with_capacity(10000),  // TODO do we need to cap this?
            max_lines,
            committed_count: 0,
            frozen_turn_ids: HashSet::new(),
            last_width: 0,
        }
    }

    /// Render active (non-frozen) turns into the hot zone.
    /// Overflow lines are committed to native scrollback via `insert_before()`.
    pub fn render_to_scrollback(
        &mut self,
        transcript: &Transcript,
        width: u16,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    ) -> anyhow::Result<()> {
        // Check for width change - if so, we need to re-render everything
        // TODO disabling width change for now since it's not essential
        // // but we can't fix already-committed content
        // if width != self.last_width && self.last_width != 0 {
        //     // Width changed - reset committed count since line counts may differ
        //     // Note: already-committed content in scrollback may have wrong width
        //     self.committed_count = 0;
        //     self.frozen_turn_ids.clear();
        // }
        self.last_width = width;

        // Render only non-frozen turns
        let active_lines: Vec<Line<'static>> = transcript
            .turns()
            .iter()
            .filter(|t| !self.frozen_turn_ids.contains(&t.id))
            .flat_map(|turn| Self::render_turn_to_lines(turn, width))
            .collect();

        // Skip lines already committed to scrollback
        let hot_lines: Vec<_> = active_lines
            .into_iter()
            .skip(self.committed_count)
            .collect();

        self.lines.clear();

        for line in hot_lines {
            self.lines.push_back(line);

            // Overflow promotes to scrollback
            while self.lines.len() > self.max_lines {
                let committed = self.lines.pop_front().unwrap();
                terminal.insert_before(1, |buf| {
                    Paragraph::new(committed).render(buf.area, buf);
                })?;
                self.committed_count += 1;
            }
        }

        // Check if any turns should be frozen
        self.check_freeze_turns(transcript, width);

        Ok(())
    }

    /// Check if any active turns have fully scrolled into scrollback
    fn check_freeze_turns(&mut self, transcript: &Transcript, width: u16) {
        let mut cumulative_lines = 0usize;

        for turn in transcript.turns() {
            if self.frozen_turn_ids.contains(&turn.id) {
                continue;
            }

            let turn_line_count = Self::render_turn_to_lines(turn, width).len();
            cumulative_lines += turn_line_count;

            if cumulative_lines <= self.committed_count {
                // This turn is fully committed to scrollback
                self.frozen_turn_ids.insert(turn.id);
                // Subtract this turn's lines from committed_count since
                // frozen turns are filtered out of active_lines
                self.committed_count -= turn_line_count;
            } else {
                // Once we hit a turn that's not fully committed, stop
                break;
            }
        }
    }

    /// Render a turn to lines (header + content + separator)
    fn render_turn_to_lines(turn: &Turn, width: u16) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        // Role header
        let (role_text, role_style) = match turn.role {
            Role::User => (
                "You",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Role::Assistant => (
                "Codey",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Role::System => (
                "System",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
        };

        let header = Line::from(vec![
            Span::styled(role_text, role_style),
            Span::styled(
                format!(" ({})", turn.timestamp.format("%H:%M:%S")),
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        lines.push(header);

        // Content lines - convert to owned by mapping spans
        for line in turn.render(width) {
            let owned_spans: Vec<Span<'static>> = line
                .spans
                .iter()
                .map(|span| Span::styled(span.content.to_string(), span.style))
                .collect();
            lines.push(Line::from(owned_spans));
        }

        // Separator (empty line)
        lines.push(Line::default());

        lines
    }

    /// Create a widget for rendering the hot zone content
    pub fn widget(&self) -> ChatViewWidget<'_> {
        ChatViewWidget { view: self }
    }
}

/// Widget for rendering the hot zone content in the viewport
pub struct ChatViewWidget<'a> {
    view: &'a ChatView,
}

impl Widget for ChatViewWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        // Collect lines and render as paragraph
        let lines: Vec<Line> = self.view.lines.iter().cloned().collect();
        
        // Calculate how many lines we can show
        let visible_lines = area.height as usize;
        let total_lines = lines.len();
        
        // Show the most recent lines (bottom-aligned)
        let skip = total_lines.saturating_sub(visible_lines);
        let visible: Vec<Line> = lines.into_iter().skip(skip).collect();

        let content_block = Block::default();

        let paragraph = Paragraph::new(visible).block(content_block);
        paragraph.render(area, buf);
    }
}


