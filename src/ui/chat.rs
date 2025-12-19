//! Chat view component with native terminal scrollback
//!
//! Uses a "hot zone" approach: recent content is rendered in the viewport,
//! and as it overflows, lines are committed to the terminal's native
//! scrollback buffer via `insert_before()`. This provides O(active turns)
//! rendering instead of O(entire conversation).

// Scrollback
// this is content which has passed above the hot zone

// Hot Zone
// this is content currently rendered in the Viewport

// Frozen Turns
// one that has already passed into scrollback

use std::collections::{HashSet, HashMap, VecDeque};
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
    /// Mapping of turn ID to line count (for frozen turns)
    turn_line_counts: HashMap<usize, usize>,

    /// Width used for last render (to detect resize)
    width: u16,
}

impl ChatView {
    pub fn new(max_lines: usize) -> Self {
        Self {
            lines: VecDeque::with_capacity(10000),  // TODO do we need to pre-size this?
            max_lines,
            committed_count: 0,
            frozen_turn_ids: HashSet::new(),
            turn_line_counts: HashMap::new(),
            width: 0,
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
        self.width = width;

        // Render non-frozen turns to lines
        let mut active_lines: Vec<Line<'static>> = Vec::new();
        for turn in transcript.turns() {
            if self.frozen_turn_ids.contains(&turn.id) {
                continue;
            }
            let render = Self::render_turn_to_lines(turn, self.width);
            self.turn_line_counts.insert(turn.id, render.len());
            active_lines.extend(render);
        }

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
        let mut cumulative_lines = 0usize;

        for turn in transcript.turns() {
            if self.frozen_turn_ids.contains(&turn.id) {
                continue;
            }
            let turn_line_count = self.turn_line_counts.get(&turn.id).unwrap_or(&0);
            cumulative_lines += *turn_line_count;

            if cumulative_lines <= self.committed_count {
                // This turn is fully committed to scrollback
                self.frozen_turn_ids.insert(turn.id);
                // Subtract this turn's lines from committed_count since
                // frozen turns are filtered out of active_lines
                self.committed_count -= *turn_line_count;
            } else {
                // Once we hit a turn that's not fully committed, stop
                break;
            }
        }

        Ok(())
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

        // Bottom-aligned: only clone visible lines
        let skip = self.view.lines.len().saturating_sub(area.height as usize);
        let visible: Vec<Line> = self.view.lines.iter().skip(skip).cloned().collect();

        Paragraph::new(visible).render(area, buf);
    }
}


