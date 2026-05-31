use crate::tools::ToolResult;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph, Wrap};

pub fn tool_result(frame: &mut Frame, area: Rect, result: &ToolResult) {
    let (title, content) = match result {
        ToolResult::FileContent {
            path, content, ..
        } => (path.clone(), content.clone()),
        ToolResult::Diagnostics {
            total,
            errors,
            warnings,
            list,
            ..
        } => {
            let mut s = format!(
                "{} diagnostics ({} errors, {} warnings)\n",
                total, errors, warnings
            );
            for d in list.iter().filter(|d| d.severity == "error") {
                s.push_str(&format!(
                    "  {}: {} [{}:{}]\n",
                    d.code.as_deref().unwrap_or("?"),
                    d.message,
                    d.file,
                    d.line,
                ));
            }
            if *errors < *total {
                s.push_str(&format!(
                    "\n  ... {} warnings/hints",
                    *total - *errors
                ));
            }
            ("".into(), s)
        }
        ToolResult::Json {
            tool, count, data, ..
        } => {
            let s = serde_json::to_string_pretty(data).unwrap_or_default();
            (format!("{} ({} results)", tool, count), s)
        }
        ToolResult::Matches {
            pattern,
            count,
            items,
            truncated,
        } => {
            let mut s = format!(
                "{}: {} matches{}\n",
                pattern,
                count,
                if *truncated { " (truncated)" } else { "" }
            );
            for m in items.iter().take(15) {
                s.push_str(&format!(
                    "  {}:{}:{}  {}\n",
                    m.file, m.line, m.column, m.text
                ));
            }
            ("".into(), s)
        }
        ToolResult::WriteConfirmation {
            path, diff, ..
        } => (path.clone(), diff.clone()),
        ToolResult::Text { content, .. } => ("".into(), content.clone()),
    };

    let block = if title.is_empty() {
        Block::default()
            .borders(Borders::NONE)
            .style(Style::default().bg(Color::Rgb(239, 241, 245)))
    } else {
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(188, 192, 204)))
            .style(Style::default().bg(Color::Rgb(239, 241, 245)))
    };

    let paragraph = Paragraph::new(content)
        .block(block)
        .style(Style::default().fg(Color::Rgb(76, 79, 105)))
        .wrap(Wrap { trim: false });

    frame.render_widget(paragraph, area);
}

const TXT: Color = Color::Rgb(76, 79, 105);
const TXT_SUBTLE: Color = Color::Rgb(108, 111, 133);
const BLUE: Color = Color::Rgb(30, 102, 245);
const RED: Color = Color::Rgb(210, 15, 57);
const ORANGE: Color = Color::Rgb(223, 142, 29);
const CYAN: Color = Color::Rgb(32, 159, 181);
const GREEN: Color = Color::Rgb(64, 160, 43);
const PURPLE: Color = Color::Rgb(136, 57, 239);
const CODE_FG: Color = Color::Rgb(242, 119, 122);

pub fn message_block(frame: &mut Frame, area: Rect, content: &str, is_user: bool) {
    let border_color = if is_user {
        Color::Rgb(30, 102, 245)
    } else {
        Color::Rgb(140, 143, 161)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .style(Style::default())
        .padding(Padding::symmetric(1, 0));

    let paragraph = if is_user {
        let lines: Vec<Line> = content
            .lines()
            .map(|line| Line::from(Span::raw(format!(" {}", line))))
            .collect();
        Paragraph::new(lines)
    } else {
        let lines = render_md(content);
        Paragraph::new(lines)
    }
    .block(block)
    .style(Style::default().fg(TXT))
    .wrap(Wrap { trim: false });

    frame.render_widget(paragraph, area);
}

fn render_md(content: &str) -> Vec<Line<'_>> {
    let mut out: Vec<Line<'_>> = Vec::new();
    let mut in_code_block = false;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            out.push(Line::from(Span::styled(
                format!("   {}", trimmed),
                Style::default().fg(TXT_SUBTLE).add_modifier(Modifier::DIM),
            )));
            continue;
        }

        if in_code_block {
            let spans = highlight_rust(line);
            let mut line_spans = vec![Span::raw("   ")];
            line_spans.extend(spans);
            out.push(Line::from(line_spans));
            continue;
        }

        // Diff lines: display with +/- colors + Rust highlighting
        if let Some(diff_line) = detect_diff(line) {
            out.push(diff_line);
            continue;
        }

        // Block-level patterns
        if let Some(text) = trimmed.strip_prefix("### ") {
            out.push(Line::from(Span::styled(
                format!("   {}", text),
                Style::default().fg(BLUE).add_modifier(Modifier::BOLD),
            )));
            continue;
        }
        if let Some(text) = trimmed.strip_prefix("## ") {
            out.push(Line::from(Span::styled(
                format!("   {}", text),
                Style::default().fg(BLUE).add_modifier(Modifier::BOLD),
            )));
            continue;
        }
        if let Some(text) = trimmed.strip_prefix("# ") {
            out.push(Line::from(Span::styled(
                format!("   {}", text),
                Style::default().fg(BLUE).add_modifier(Modifier::BOLD),
            )));
            continue;
        }
        if let Some(text) = trimmed.strip_prefix("> ") {
            out.push(Line::from(Span::styled(
                format!("   | {}", text),
                Style::default().fg(TXT_SUBTLE).add_modifier(Modifier::ITALIC),
            )));
            continue;
        }
        if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
            let text = &trimmed[2..];
            let spans = parse_inline_md(text, false);
            let mut line_spans = vec![Span::styled(
                "   \u{2022} ",
                Style::default().fg(BLUE),
            )];
            line_spans.extend(spans);
            out.push(Line::from(line_spans));
            continue;
        }
        if let Some(idx) = trimmed.find(". ")
            && idx > 0 && trimmed[..idx].chars().all(|c| c.is_ascii_digit()) {
                let num = &trimmed[..idx];
                let text = &trimmed[idx + 2..];
                let spans = parse_inline_md(text, false);
                let mut line_spans = vec![Span::styled(
                    format!("   {}. ", num),
                    Style::default().fg(BLUE),
                )];
                line_spans.extend(spans);
                out.push(Line::from(line_spans));
                continue;
            }

        // Tool call pending spinner: "  ⟳ tool_name(args)"
        if let Some(rest) = trimmed.strip_prefix("\u{27f3} ") {
            if let Some((name, _args)) = rest.split_once('(') {
                out.push(Line::from(vec![
                    Span::styled(
                        "   \u{27f3} ",
                        Style::default().fg(ORANGE).add_modifier(Modifier::SLOW_BLINK),
                    ),
                    Span::styled(
                        name,
                        Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        &rest[name.len()..],
                        Style::default().fg(CYAN).add_modifier(Modifier::DIM),
                    ),
                ]));
                continue;
            }
        }

        // Tool call completed: "  tool_name(args)" (no spinner)
        if let Some((name, _args)) = trimmed.split_once('(') {
            if trimmed.ends_with(')') && !name.contains(' ') && name.len() <= 20 {
                out.push(Line::from(Span::styled(
                    format!("   {}", trimmed),
                    Style::default().fg(CYAN).add_modifier(Modifier::DIM),
                )));
                continue;
            }
        }
        if trimmed.starts_with("Error:") || trimmed.starts_with("error:") {
            out.push(Line::from(Span::styled(
                format!("   {}", trimmed),
                Style::default().fg(RED),
            )));
            continue;
        }
        if trimmed.starts_with("warning:") {
            out.push(Line::from(Span::styled(
                format!("   {}", trimmed),
                Style::default().fg(ORANGE),
            )));
            continue;
        }

        // Inline markdown
        let spans = parse_inline_md(line, true);
        out.push(Line::from(spans));
    }

    out
}

fn parse_inline_md(line: &str, add_prefix: bool) -> Vec<Span<'_>> {
    let prefix = if add_prefix { "   " } else { "" };
    let text = line;

    let mut spans: Vec<Span<'_>> = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut buf = String::new();

    let flush = |buf: &mut String, spans: &mut Vec<Span<'_>>| {
        if !buf.is_empty() {
            spans.push(Span::styled(
                std::mem::take(buf),
                Style::default().fg(TXT),
            ));
        }
    };

    while i < len {
        // Inline code: `code`
        if chars[i] == '`' {
            flush(&mut buf, &mut spans);
            let mut code = String::new();
            i += 1; // skip opening `
            while i < len && chars[i] != '`' {
                code.push(chars[i]);
                i += 1;
            }
            if i < len {
                i += 1; // skip closing `
            }
            // Add prefix to first span inside code
            let display = if add_prefix && spans.is_empty() {
                format!("{}{}", prefix, code)
            } else {
                code
            };
            spans.push(Span::styled(
                display,
                Style::default().fg(CODE_FG).add_modifier(Modifier::BOLD),
            ));
            continue;
        }

        // Bold: **text**
        if i + 1 < len && chars[i] == '*' && chars[i + 1] == '*' {
            flush(&mut buf, &mut spans);
            i += 2; // skip **
            let mut bold = String::new();
            while i + 1 < len && !(chars[i] == '*' && chars[i + 1] == '*') {
                bold.push(chars[i]);
                i += 1;
            }
            if i + 1 < len {
                i += 2; // skip closing **
            }
            let display = if add_prefix && spans.is_empty() {
                format!("{}{}", prefix, bold)
            } else {
                bold
            };
            spans.push(Span::styled(
                display,
                Style::default().fg(TXT).add_modifier(Modifier::BOLD),
            ));
            continue;
        }

        // Italic: *text* (but not ** or empty)
        if chars[i] == '*' {
            flush(&mut buf, &mut spans);
            i += 1;
            let mut italic = String::new();
            while i < len && chars[i] != '*' {
                italic.push(chars[i]);
                i += 1;
            }
            if i < len {
                i += 1; // skip closing *
            }
            if !italic.is_empty() {
                let display = if add_prefix && spans.is_empty() {
                    format!("{}{}", prefix, italic)
                } else {
                    italic
                };
                spans.push(Span::styled(
                    display,
                    Style::default().fg(TXT).add_modifier(Modifier::ITALIC),
                ));
            } else {
                // Stray * — treat as literal
                buf.push('*');
            }
            continue;
        }

        buf.push(chars[i]);
        i += 1;
    }

    flush(&mut buf, &mut spans);

    if spans.is_empty() && !text.is_empty() {
        spans.push(Span::styled(
            format!("{}{}", prefix, text),
            Style::default().fg(TXT),
        ));
    }

    spans
}

pub fn user_input(frame: &mut Frame, area: Rect, input: &str) {
    let line = Line::from(vec![
        Span::styled(
            "bytode",
            Style::default()
                .fg(Color::Rgb(30, 102, 245))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("> ", Style::default().fg(Color::Rgb(64, 160, 43))),
        Span::styled(input, Style::default().fg(Color::Rgb(76, 79, 105))),
        Span::styled(
            "|",
            Style::default()
                .fg(Color::Rgb(76, 79, 105))
                .add_modifier(Modifier::SLOW_BLINK),
        ),
    ]);
    let paragraph = Paragraph::new(line).style(Style::default().bg(Color::Rgb(239, 241, 245)));
    frame.render_widget(paragraph, area);
}

pub fn approve(frame: &mut Frame, path: &str, diff: &str, preview_lines: usize) {
    let area = frame.area();
    let popup_width = std::cmp::min(80, area.width) as u16;
    let popup_height = std::cmp::min(20, area.height) as u16;
    let popup_x = (area.width.saturating_sub(popup_width)) / 2;
    let popup_y = (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);

    let diff_preview: String = diff.lines().take(preview_lines).collect::<Vec<_>>().join("\n");

    let content = format!(
        "File: {}\n\n{}\n\n[Y] approve  [N] reject  [V] view full diff",
        path, diff_preview
    );

    let block = Block::default()
        .title("Confirm Write")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(223, 142, 29)))
        .style(Style::default().bg(Color::Rgb(239, 241, 245)));

    let paragraph = Paragraph::new(content)
        .block(block)
        .style(Style::default().fg(Color::Rgb(76, 79, 105)))
        .wrap(Wrap { trim: false });

    frame.render_widget(paragraph, popup_area);
}

fn detect_diff(line: &str) -> Option<Line<'_>> {
    if let Some(code) = line.strip_prefix("   - ") {
        let mut spans = vec![Span::styled(
            "   - ",
            Style::default().fg(RED),
        )];
        spans.extend(highlight_rust(code));
        Some(Line::from(spans))
    } else if let Some(code) = line.strip_prefix("   + ") {
        let mut spans = vec![Span::styled(
            "   + ",
            Style::default().fg(GREEN),
        )];
        spans.extend(highlight_rust(code));
        Some(Line::from(spans))
    } else if let Some(code) = line.strip_prefix("   -") {
        let mut spans = vec![Span::styled(
            "   -",
            Style::default().fg(RED),
        )];
        spans.extend(highlight_rust(code));
        Some(Line::from(spans))
    } else if let Some(code) = line.strip_prefix("   +") {
        let mut spans = vec![Span::styled(
            "   +",
            Style::default().fg(GREEN),
        )];
        spans.extend(highlight_rust(code));
        Some(Line::from(spans))
    } else {
        None
    }
}

fn highlight_rust(code: &str) -> Vec<Span<'_>> {
    if code.is_empty() {
        return vec![];
    }

    let mut spans: Vec<Span<'_>> = Vec::new();
    let chars: Vec<char> = code.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut buf = String::new();

    let flush = |buf: &mut String, spans: &mut Vec<Span<'static>>, style: Style| {
        if !buf.is_empty() {
            spans.push(Span::styled(std::mem::take(buf), style));
        }
    };

    let normal = Style::default().fg(TXT);

    while i < len {
        // Comments
        if i + 1 < len && chars[i] == '/' && chars[i + 1] == '/' {
            flush(&mut buf, &mut spans, normal);
            let remaining: String = chars[i..].iter().collect();
            spans.push(Span::styled(
                remaining,
                Style::default().fg(TXT_SUBTLE).add_modifier(Modifier::DIM),
            ));
            return spans;
        }

        // String literals
        if chars[i] == '"' {
            flush(&mut buf, &mut spans, normal);
            let mut s = String::new();
            s.push('"');
            i += 1;
            while i < len && chars[i] != '"' {
                if chars[i] == '\\' && i + 1 < len {
                    s.push(chars[i]);
                    s.push(chars[i + 1]);
                    i += 2;
                } else {
                    s.push(chars[i]);
                    i += 1;
                }
            }
            if i < len {
                s.push('"');
                i += 1;
            }
            spans.push(Span::styled(s, Style::default().fg(GREEN)));
            continue;
        }

        // Raw string
        if i + 1 < len && chars[i] == 'r' && chars[i + 1] == '"' {
            flush(&mut buf, &mut spans, normal);
            let mut s = String::new();
            while i < len {
                s.push(chars[i]);
                if chars[i] == '"' && s.len() > 2 {
                    i += 1;
                    break;
                }
                i += 1;
            }
            spans.push(Span::styled(s, Style::default().fg(GREEN)));
            continue;
        }

        // Lifetime
        if chars[i] == '\'' && i + 1 < len && chars[i + 1].is_alphabetic() {
            flush(&mut buf, &mut spans, normal);
            let mut lt = String::new();
            lt.push('\'');
            i += 1;
            while i < len && (chars[i].is_alphanumeric() || chars[i] == '_') {
                lt.push(chars[i]);
                i += 1;
            }
            spans.push(Span::styled(lt, Style::default().fg(CYAN)));
            continue;
        }

        // Number
        if chars[i].is_ascii_digit() && (i == 0 || !chars[i - 1].is_alphanumeric() && chars[i - 1] != '_') {
            flush(&mut buf, &mut spans, normal);
            let mut num = String::new();
            while i < len && (chars[i].is_ascii_digit() || chars[i] == '.' || chars[i] == '_' || chars[i] == 'x' || chars[i] == 'o' || chars[i] == 'b' || chars[i].is_ascii_hexdigit()) {
                num.push(chars[i]);
                i += 1;
            }
            spans.push(Span::styled(num, Style::default().fg(ORANGE)));
            continue;
        }

        // Identifier — check if keyword or type
        if chars[i].is_alphabetic() || chars[i] == '_' {
            let start = i;
            let mut word = String::new();
            while i < len && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '!') {
                word.push(chars[i]);
                i += 1;
            }

            let style = if is_keyword(&word) {
                Style::default().fg(PURPLE).add_modifier(Modifier::BOLD)
            } else if is_type(&word) {
                Style::default().fg(BLUE)
            } else if start > 0 && chars[start - 1] == '#' {
                // attribute: #[derive(...)]
                Style::default().fg(TXT_SUBTLE)
            } else {
                normal
            };

            flush(&mut buf, &mut spans, normal);
            buf = word;
            flush(&mut buf, &mut spans, style);
            continue;
        }

        buf.push(chars[i]);
        i += 1;
    }

    flush(&mut buf, &mut spans, normal);
    spans
}

fn is_keyword(word: &str) -> bool {
    matches!(
        word,
        "fn" | "let" | "mut" | "if" | "else" | "match" | "return" | "loop"
            | "while" | "for" | "in" | "break" | "continue"
            | "struct" | "enum" | "impl" | "trait" | "pub" | "use" | "mod"
            | "async" | "await" | "where" | "move" | "ref" | "static" | "const"
            | "type" | "as" | "unsafe" | "extern" | "crate" | "super" | "self"
            | "true" | "false" | "dyn" | "macro_rules!" | "macro"
    )
}

fn is_type(word: &str) -> bool {
    matches!(
        word,
        "String" | "Vec" | "Option" | "Result" | "HashMap" | "HashSet"
            | "Box" | "Arc" | "Mutex" | "Rc" | "Cell" | "RefCell"
            | "u8" | "u16" | "u32" | "u64" | "u128" | "usize"
            | "i8" | "i16" | "i32" | "i64" | "i128" | "isize"
            | "f32" | "f64" | "bool" | "char" | "str"
            | "PathBuf" | "Path" | "Duration" | "Instant"
            | "BTreeMap" | "BTreeSet" | "BinaryHeap" | "VecDeque"
            | "Cow" | "OsString" | "CString"
    ) || word.starts_with(|c: char| c.is_uppercase())
}
