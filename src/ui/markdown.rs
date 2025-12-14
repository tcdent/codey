//! Markdown rendering with syntax highlighting

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use syntect::{
    easy::HighlightLines,
    highlighting::{ThemeSet, Style as SyntectStyle},
    parsing::SyntaxSet,
    util::LinesWithEndings,
};

/// Markdown renderer with syntax highlighting
pub struct MarkdownRenderer {
    syntax_set: SyntaxSet,
    theme_set: ThemeSet,
    theme_name: String,
}

impl MarkdownRenderer {
    /// Create a new markdown renderer
    pub fn new() -> Self {
        Self {
            syntax_set: SyntaxSet::load_defaults_newlines(),
            theme_set: ThemeSet::load_defaults(),
            theme_name: "base16-ocean.dark".to_string(),
        }
    }

    /// Set the theme
    pub fn with_theme(mut self, theme_name: impl Into<String>) -> Self {
        self.theme_name = theme_name.into();
        self
    }

    /// Render markdown to styled lines
    pub fn render(&self, text: &str) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let mut in_code_block = false;
        let mut code_language = String::new();
        let mut code_buffer = String::new();

        for line in text.lines() {
            if line.starts_with("```") {
                if in_code_block {
                    // End of code block - render the accumulated code
                    lines.extend(self.highlight_code(&code_buffer, &code_language));
                    lines.push(Line::from(Span::styled(
                        "```",
                        Style::default().fg(Color::DarkGray),
                    )));
                    code_buffer.clear();
                    code_language.clear();
                    in_code_block = false;
                } else {
                    // Start of code block
                    code_language = line.trim_start_matches('`').to_string();
                    lines.push(Line::from(Span::styled(
                        line.to_string(),
                        Style::default().fg(Color::DarkGray),
                    )));
                    in_code_block = true;
                }
            } else if in_code_block {
                code_buffer.push_str(line);
                code_buffer.push('\n');
            } else {
                lines.push(self.render_inline(line));
            }
        }

        // Handle unclosed code block
        if in_code_block && !code_buffer.is_empty() {
            lines.extend(self.highlight_code(&code_buffer, &code_language));
        }

        lines
    }

    /// Render inline markdown (bold, italic, code)
    fn render_inline(&self, text: &str) -> Line<'static> {
        let mut spans = Vec::new();
        let mut current = String::new();
        let mut chars = text.chars().peekable();

        while let Some(c) = chars.next() {
            match c {
                '`' => {
                    // Inline code
                    if !current.is_empty() {
                        spans.push(Span::raw(std::mem::take(&mut current)));
                    }

                    let mut code = String::new();
                    while let Some(&next) = chars.peek() {
                        if next == '`' {
                            chars.next();
                            break;
                        }
                        code.push(chars.next().unwrap());
                    }

                    spans.push(Span::styled(
                        code,
                        Style::default().fg(Color::Yellow).bg(Color::DarkGray),
                    ));
                }
                '*' if chars.peek() == Some(&'*') => {
                    // Bold
                    chars.next(); // consume second *
                    if !current.is_empty() {
                        spans.push(Span::raw(std::mem::take(&mut current)));
                    }

                    let mut bold_text = String::new();
                    while let Some(&next) = chars.peek() {
                        if next == '*' {
                            chars.next();
                            if chars.peek() == Some(&'*') {
                                chars.next();
                                break;
                            }
                            bold_text.push('*');
                        } else {
                            bold_text.push(chars.next().unwrap());
                        }
                    }

                    spans.push(Span::styled(
                        bold_text,
                        Style::default().add_modifier(Modifier::BOLD),
                    ));
                }
                '_' if chars.peek() == Some(&'_') => {
                    // Also bold (alternative syntax)
                    chars.next();
                    if !current.is_empty() {
                        spans.push(Span::raw(std::mem::take(&mut current)));
                    }

                    let mut bold_text = String::new();
                    while let Some(&next) = chars.peek() {
                        if next == '_' {
                            chars.next();
                            if chars.peek() == Some(&'_') {
                                chars.next();
                                break;
                            }
                            bold_text.push('_');
                        } else {
                            bold_text.push(chars.next().unwrap());
                        }
                    }

                    spans.push(Span::styled(
                        bold_text,
                        Style::default().add_modifier(Modifier::BOLD),
                    ));
                }
                '*' | '_' => {
                    // Italic
                    let delimiter = c;
                    if !current.is_empty() {
                        spans.push(Span::raw(std::mem::take(&mut current)));
                    }

                    let mut italic_text = String::new();
                    while let Some(&next) = chars.peek() {
                        if next == delimiter {
                            chars.next();
                            break;
                        }
                        italic_text.push(chars.next().unwrap());
                    }

                    spans.push(Span::styled(
                        italic_text,
                        Style::default().add_modifier(Modifier::ITALIC),
                    ));
                }
                '#' if current.is_empty() => {
                    // Heading
                    let mut level = 1;
                    while chars.peek() == Some(&'#') {
                        chars.next();
                        level += 1;
                    }
                    // Skip space after #
                    if chars.peek() == Some(&' ') {
                        chars.next();
                    }

                    let heading_text: String = chars.collect();
                    let style = match level {
                        1 => Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                        2 => Style::default()
                            .fg(Color::Blue)
                            .add_modifier(Modifier::BOLD),
                        _ => Style::default()
                            .fg(Color::Magenta)
                            .add_modifier(Modifier::BOLD),
                    };

                    spans.push(Span::styled(heading_text, style));
                    break;
                }
                _ => {
                    current.push(c);
                }
            }
        }

        if !current.is_empty() {
            spans.push(Span::raw(current));
        }

        Line::from(spans)
    }

    /// Highlight code with syntect
    fn highlight_code(&self, code: &str, language: &str) -> Vec<Line<'static>> {
        let syntax = self
            .syntax_set
            .find_syntax_by_token(language)
            .or_else(|| self.syntax_set.find_syntax_by_extension(language))
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());

        let theme = self
            .theme_set
            .themes
            .get(&self.theme_name)
            .unwrap_or_else(|| &self.theme_set.themes["base16-ocean.dark"]);

        let mut highlighter = HighlightLines::new(syntax, theme);
        let mut lines = Vec::new();

        for line in LinesWithEndings::from(code) {
            let highlighted = highlighter
                .highlight_line(line, &self.syntax_set)
                .unwrap_or_default();

            let spans: Vec<Span> = highlighted
                .into_iter()
                .map(|(style, text)| {
                    Span::styled(
                        text.trim_end_matches('\n').to_string(),
                        syntect_style_to_ratatui(style),
                    )
                })
                .collect();

            lines.push(Line::from(spans));
        }

        lines
    }
}

impl Default for MarkdownRenderer {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert syntect style to ratatui style
fn syntect_style_to_ratatui(style: SyntectStyle) -> Style {
    let fg = Color::Rgb(
        style.foreground.r,
        style.foreground.g,
        style.foreground.b,
    );

    Style::default().fg(fg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_plain_text() {
        let renderer = MarkdownRenderer::new();
        let lines = renderer.render("Hello, world!");
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn test_render_code_block() {
        let renderer = MarkdownRenderer::new();
        let text = "```rust\nfn main() {}\n```";
        let lines = renderer.render(text);
        assert!(lines.len() >= 3); // At least opening, code, closing
    }

    #[test]
    fn test_render_inline_code() {
        let renderer = MarkdownRenderer::new();
        let lines = renderer.render("Use `code` here");
        assert_eq!(lines.len(), 1);
        assert!(lines[0].spans.len() >= 3); // text, code, text
    }
}
