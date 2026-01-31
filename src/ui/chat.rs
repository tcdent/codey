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

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Stdout;

use chrono::Local;
use ratatui::{
    backend::CrosstermBackend,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
    Terminal,
};

#[cfg(feature = "profiling")]
use crate::profile_span;
use crate::transcript::{Block, Role, Stage, Status, Transcript, Turn};

/// Chat view with native scrollback support.
///
/// Owns the conversation transcript and handles rendering to terminal.
pub struct ChatView {
    /// The conversation transcript (owned)
    pub transcript: Transcript,
    /// Terminal width for text wrapping
    width: u16,
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
    /// Agent name for display (configurable)
    agent_name: String,
}

impl ChatView {
    pub fn new(transcript: Transcript, width: u16, max_lines: usize, agent_name: String) -> Self {
        Self {
            transcript,
            width,
            lines: VecDeque::with_capacity(10000),
            max_lines,
            committed_count: 0,
            frozen_turn_ids: HashSet::new(),
            turn_line_counts: HashMap::new(),
            agent_name,
        }
    }

    // ==================== Transcript mutation helpers ====================

    /// Mark the first block of a turn as complete
    // TODO: Confusing - name says "last" but accesses first_mut(). Review whether
    // this should be first or last, and rename accordingly.
    #[allow(dead_code)]
    pub fn mark_last_block_complete(&mut self, turn_id: usize) {
        if let Some(turn) = self.transcript.get_mut(turn_id) {
            if let Some(block) = turn.content.first_mut() {
                block.set_status(Status::Complete);
            }
        }
    }

    // ==================== Transcript mutation + render ====================

    /// Begin a new turn and render
    pub fn begin_turn(&mut self, role: Role, terminal: &mut Terminal<CrosstermBackend<Stdout>>) {
        self.transcript.begin_turn(role);
        self.render(terminal)
    }

    /// Finish the current turn and render
    pub fn finish_turn(&mut self, terminal: &mut Terminal<CrosstermBackend<Stdout>>) {
        self.transcript.finish_turn();
        self.render(terminal)
    }

    /// Start a new block in the current turn and render
    pub fn start_block(
        &mut self,
        block: Box<dyn Block>,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    ) {
        self.transcript.start_block(block);
        self.render(terminal)
    }

    /// Add a complete turn (for initial setup, doesn't auto-render)
    pub fn add_turn(&mut self, role: Role, block: impl Block + 'static) -> usize {
        self.transcript.add_turn(role, block)
    }

    /// Update the terminal width (e.g., on resize)
    pub fn set_width(&mut self, width: u16) {
        self.width = width;
    }

    /// Replace the transcript and reset view state (used after compaction rotation)
    /// Renders the new transcript fully to scrollback
    pub fn reset_transcript(
        &mut self,
        transcript: Transcript,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    ) {
        self.transcript = transcript;
        self.lines.clear();
        self.committed_count = 0;
        self.frozen_turn_ids.clear();
        self.turn_line_counts.clear();
        self.render(terminal)
    }

    /// Render active (non-frozen) turns into the hot zone.
    /// Overflow lines are committed to native scrollback via `insert_before()`.
    pub fn render(&mut self, terminal: &mut Terminal<CrosstermBackend<Stdout>>) {
        #[cfg(feature = "profiling")]
        let _span = profile_span!("ChatView::render");

        // Render non-frozen turns to lines
        let mut active_lines: Vec<Line<'static>> = Vec::new();
        for turn in self.transcript.turns() {
            if self.frozen_turn_ids.contains(&turn.id) {
                continue;
            }
            let render = Self::render_turn_to_lines(turn, self.width, &self.agent_name);
            self.turn_line_counts.insert(turn.id, render.len());
            active_lines.extend(render);
        }

        // Render staged blocks (pending notifications/messages)
        if !self.transcript.stage.is_empty() {
            active_lines.extend(Self::render_stage(&self.transcript.stage, self.width));
        }

        // Skip lines already committed to scrollback
        let hot_lines: Vec<_> = active_lines
            .into_iter()
            .skip(self.committed_count)
            .collect();

        tracing::trace!(
            "render(): hot_lines={}, committed_count={}, max_lines={}",
            hot_lines.len(),
            self.committed_count,
            self.max_lines
        );

        self.lines.clear();

        for line in hot_lines {
            self.lines.push_back(line);

            // Overflow promotes to scrollback
            while self.lines.len() > self.max_lines {
                let committed = self.lines.pop_front().unwrap();
                let line_preview: String =
                    committed.spans.iter().map(|s| s.content.as_ref()).collect();
                tracing::trace!(
                    "Scrollback commit: line={:?}, committed_count={}, lines.len={}, max={}",
                    line_preview,
                    self.committed_count,
                    self.lines.len(),
                    self.max_lines
                );

                if let Err(e) = terminal.insert_before(1, |buf| {
                    Paragraph::new(committed).render(buf.area, buf);
                }) {
                    tracing::warn!("Failed to commit line to scrollback: {}", e);
                }
                self.committed_count += 1;
            }
        }

        // Check if any turns should be frozen
        let mut cumulative_lines = 0usize;

        for turn in self.transcript.turns() {
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
    }

    /// Render a turn to lines (header + content + separator)
    fn render_turn_to_lines(turn: &Turn, width: u16, agent_name: &str) -> Vec<Line<'static>> {
        #[cfg(feature = "profiling")]
        let _span = profile_span!("ChatView::render_turn_to_lines");

        let mut lines = Vec::new();

        // Role header
        let (role_text, role_style) = match turn.role {
            Role::User => (
                "You".to_string(),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Role::Assistant => (
                agent_name.to_string(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Role::System => (
                "System".to_string(),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
        };

        // Convert UTC timestamp to local time for display
        let local_time = turn.timestamp.with_timezone(&Local);
        let header = Line::from(vec![
            Span::styled(role_text, role_style),
            Span::styled(
                format!(" ({})", local_time.format("%H:%M:%S")),
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

    /// Render staged blocks (pending notifications awaiting consumption)
    fn render_stage(stage: &Stage, width: u16) -> Vec<Line<'static>> {
        if stage.is_empty() {
            return Vec::new();
        }

        let mut lines = Vec::new();

        // Render staged blocks
        for line in stage.render(width) {
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
}

/// Widget for rendering the hot zone content in the viewport
pub struct ChatViewWidget<'a> {
    view: &'a ChatView,
}

impl Widget for ChatViewWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        #[cfg(feature = "profiling")]
        let _span = profile_span!("ChatViewWidget::render");

        if area.width == 0 || area.height == 0 {
            return;
        }

        // Bottom-aligned: only clone visible lines
        let skip = self.view.lines.len().saturating_sub(area.height as usize);
        let visible: Vec<Line> = self.view.lines.iter().skip(skip).cloned().collect();

        Paragraph::new(visible).render(area, buf);
    }
}
