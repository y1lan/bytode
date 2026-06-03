use ratatui::prelude::*;

const TXT: Color = Color::Rgb(76, 79, 105);
const TXT_SUBTLE: Color = Color::Rgb(108, 111, 133);
const BLUE: Color = Color::Rgb(30, 102, 245);
const RED: Color = Color::Rgb(210, 15, 57);
const ORANGE: Color = Color::Rgb(223, 142, 29);
const CYAN: Color = Color::Rgb(32, 159, 181);
const GREEN: Color = Color::Rgb(64, 160, 43);
const PURPLE: Color = Color::Rgb(136, 57, 239);
const CODE_FG: Color = Color::Rgb(242, 119, 122);

pub fn render_md(content: &str) -> Vec<Line<'_>> {
    let mut out: Vec<Line<'_>> = Vec::new();
    let mut in_code_block = false;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            out.push(Line::from(Span::styled(
                format!("   {trimmed}"),
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
        if let Some(diff_line) = detect_diff(line) {
            out.push(diff_line);
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("\u{27f3} ") {
            if let Some((name, _)) = rest.split_once('(') {
                out.push(Line::from(vec![
                    Span::styled(
                        "   \u{27f3} ",
                        Style::default()
                            .fg(ORANGE)
                            .add_modifier(Modifier::SLOW_BLINK),
                    ),
                    Span::styled(name, Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
                    Span::styled(
                        &rest[name.len()..],
                        Style::default().fg(CYAN).add_modifier(Modifier::DIM),
                    ),
                ]));
                continue;
            }
        }
        if let Some((name, _)) = trimmed.split_once('(') {
            if trimmed.ends_with(')') && !name.contains(' ') && name.len() <= 20 {
                out.push(Line::from(Span::styled(
                    format!("   {trimmed}"),
                    Style::default().fg(CYAN).add_modifier(Modifier::DIM),
                )));
                continue;
            }
        }
        if let Some(text) = trimmed.strip_prefix("### ") {
            out.push(header_line(text));
            continue;
        }
        if let Some(text) = trimmed.strip_prefix("## ") {
            out.push(header_line(text));
            continue;
        }
        if let Some(text) = trimmed.strip_prefix("# ") {
            out.push(header_line(text));
            continue;
        }
        if let Some(text) = trimmed.strip_prefix("> ") {
            out.push(Line::from(Span::styled(
                format!("   | {text}"),
                Style::default()
                    .fg(TXT_SUBTLE)
                    .add_modifier(Modifier::ITALIC),
            )));
            continue;
        }
        if let Some(text) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            let spans = parse_inline_md(text, false);
            let mut line_spans = vec![Span::styled("   \u{2022} ", Style::default().fg(BLUE))];
            line_spans.extend(spans);
            out.push(Line::from(line_spans));
            continue;
        }
        if let Some(idx) = trimmed.find(". ") {
            if idx > 0 && trimmed[..idx].chars().all(|c| c.is_ascii_digit()) {
                let num = &trimmed[..idx];
                let text = &trimmed[idx + 2..];
                let spans = parse_inline_md(text, false);
                let mut line_spans = vec![Span::styled(
                    format!("   {num}. "),
                    Style::default().fg(BLUE),
                )];
                line_spans.extend(spans);
                out.push(Line::from(line_spans));
                continue;
            }
        }
        if trimmed.starts_with("Error:") || trimmed.starts_with("error:") {
            out.push(Line::from(Span::styled(
                format!("   {trimmed}"),
                Style::default().fg(RED),
            )));
            continue;
        }
        if trimmed.starts_with("warning:") {
            out.push(Line::from(Span::styled(
                format!("   {trimmed}"),
                Style::default().fg(ORANGE),
            )));
            continue;
        }

        out.push(Line::from(parse_inline_md(line, true)));
    }

    out
}

fn header_line(text: &str) -> Line<'_> {
    Line::from(Span::styled(
        format!("   {text}"),
        Style::default().fg(BLUE).add_modifier(Modifier::BOLD),
    ))
}

fn parse_inline_md(line: &str, add_prefix: bool) -> Vec<Span<'_>> {
    let mut spans: Vec<Span<'_>> = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut buf = String::new();
    let mut need_prefix = add_prefix;

    fn flush_text(buf: &mut String, spans: &mut Vec<Span<'_>>, need_prefix: &mut bool) {
        if !buf.is_empty() {
            let text = if *need_prefix {
                *need_prefix = false;
                format!("   {}", std::mem::take(buf))
            } else {
                std::mem::take(buf)
            };
            spans.push(Span::styled(text, Style::default().fg(TXT)));
        }
    }

    fn push_styled(spans: &mut Vec<Span<'_>>, text: String, style: Style, need_prefix: &mut bool) {
        let text = if *need_prefix {
            *need_prefix = false;
            format!("   {text}")
        } else {
            text
        };
        spans.push(Span::styled(text, style));
    }

    while i < len {
        if chars[i] == '`' {
            flush_text(&mut buf, &mut spans, &mut need_prefix);
            let mut code = String::new();
            i += 1;
            while i < len && chars[i] != '`' {
                code.push(chars[i]);
                i += 1;
            }
            if i < len {
                i += 1;
            }
            push_styled(
                &mut spans,
                code,
                Style::default().fg(CODE_FG).add_modifier(Modifier::BOLD),
                &mut need_prefix,
            );
            continue;
        }
        if i + 1 < len && chars[i] == '*' && chars[i + 1] == '*' {
            flush_text(&mut buf, &mut spans, &mut need_prefix);
            i += 2;
            let mut bold = String::new();
            while i + 1 < len && !(chars[i] == '*' && chars[i + 1] == '*') {
                bold.push(chars[i]);
                i += 1;
            }
            if i + 1 < len {
                i += 2;
            }
            push_styled(
                &mut spans,
                bold,
                Style::default().fg(TXT).add_modifier(Modifier::BOLD),
                &mut need_prefix,
            );
            continue;
        }
        if chars[i] == '*' {
            flush_text(&mut buf, &mut spans, &mut need_prefix);
            i += 1;
            let mut italic = String::new();
            while i < len && chars[i] != '*' {
                italic.push(chars[i]);
                i += 1;
            }
            if i < len {
                i += 1;
            }
            if !italic.is_empty() {
                push_styled(
                    &mut spans,
                    italic,
                    Style::default().fg(TXT).add_modifier(Modifier::ITALIC),
                    &mut need_prefix,
                );
            } else {
                buf.push('*');
            }
            continue;
        }
        buf.push(chars[i]);
        i += 1;
    }

    flush_text(&mut buf, &mut spans, &mut need_prefix);
    if spans.is_empty() && !line.is_empty() {
        push_styled(
            &mut spans,
            line.to_string(),
            Style::default().fg(TXT),
            &mut need_prefix,
        );
    }
    spans
}

fn detect_diff(line: &str) -> Option<Line<'_>> {
    let (prefix, code) = if let Some(code) = line.strip_prefix("   - ") {
        ("   - ", code)
    } else if let Some(code) = line.strip_prefix("   + ") {
        ("   + ", code)
    } else if let Some(code) = line.strip_prefix("   -") {
        ("   -", code)
    } else if let Some(code) = line.strip_prefix("   +") {
        ("   +", code)
    } else {
        return None;
    };

    let color = if prefix.contains('-') { RED } else { GREEN };
    let mut spans = vec![Span::styled(prefix, Style::default().fg(color))];
    spans.extend(highlight_rust(code));
    Some(Line::from(spans))
}

fn highlight_rust(code: &str) -> Vec<Span<'static>> {
    if code.is_empty() {
        return vec![];
    }

    let mut spans: Vec<Span<'static>> = Vec::new();
    let chars: Vec<char> = code.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut buf = String::new();
    let normal = Style::default().fg(TXT);

    let flush = |buf: &mut String, spans: &mut Vec<Span<'static>>, style: Style| {
        if !buf.is_empty() {
            spans.push(Span::styled(std::mem::take(buf), style));
        }
    };

    while i < len {
        if i + 1 < len && chars[i] == '/' && chars[i + 1] == '/' {
            flush(&mut buf, &mut spans, normal);
            let remaining: String = chars[i..].iter().collect();
            spans.push(Span::styled(
                remaining,
                Style::default().fg(TXT_SUBTLE).add_modifier(Modifier::DIM),
            ));
            return spans;
        }
        if chars[i] == '"' {
            flush(&mut buf, &mut spans, normal);
            let mut string = String::new();
            string.push('"');
            i += 1;
            while i < len && chars[i] != '"' {
                if chars[i] == '\\' && i + 1 < len {
                    string.push(chars[i]);
                    string.push(chars[i + 1]);
                    i += 2;
                } else {
                    string.push(chars[i]);
                    i += 1;
                }
            }
            if i < len {
                string.push('"');
                i += 1;
            }
            spans.push(Span::styled(string, Style::default().fg(GREEN)));
            continue;
        }
        if i + 1 < len && chars[i] == 'r' && chars[i + 1] == '"' {
            flush(&mut buf, &mut spans, normal);
            let mut raw = String::new();
            while i < len {
                raw.push(chars[i]);
                if chars[i] == '"' && raw.len() > 2 {
                    i += 1;
                    break;
                }
                i += 1;
            }
            spans.push(Span::styled(raw, Style::default().fg(GREEN)));
            continue;
        }
        if chars[i] == '\'' && i + 1 < len && chars[i + 1].is_alphabetic() {
            flush(&mut buf, &mut spans, normal);
            let mut lifetime = String::new();
            lifetime.push('\'');
            i += 1;
            while i < len && (chars[i].is_alphanumeric() || chars[i] == '_') {
                lifetime.push(chars[i]);
                i += 1;
            }
            spans.push(Span::styled(lifetime, Style::default().fg(CYAN)));
            continue;
        }
        if chars[i].is_ascii_digit()
            && (i == 0 || (!chars[i - 1].is_alphanumeric() && chars[i - 1] != '_'))
        {
            flush(&mut buf, &mut spans, normal);
            let mut num = String::new();
            while i < len
                && (chars[i].is_ascii_digit()
                    || chars[i] == '.'
                    || chars[i] == '_'
                    || chars[i] == 'x'
                    || chars[i] == 'o'
                    || chars[i] == 'b'
                    || chars[i].is_ascii_hexdigit())
            {
                num.push(chars[i]);
                i += 1;
            }
            spans.push(Span::styled(num, Style::default().fg(ORANGE)));
            continue;
        }
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
        "fn" | "let"
            | "mut"
            | "if"
            | "else"
            | "match"
            | "return"
            | "loop"
            | "while"
            | "for"
            | "in"
            | "break"
            | "continue"
            | "struct"
            | "enum"
            | "impl"
            | "trait"
            | "pub"
            | "use"
            | "mod"
            | "async"
            | "await"
            | "where"
            | "move"
            | "ref"
            | "static"
            | "const"
            | "type"
            | "as"
            | "unsafe"
            | "extern"
            | "crate"
            | "super"
            | "self"
            | "true"
            | "false"
            | "dyn"
    )
}

fn is_type(word: &str) -> bool {
    matches!(
        word,
        "String"
            | "Vec"
            | "Option"
            | "Result"
            | "HashMap"
            | "HashSet"
            | "Box"
            | "Arc"
            | "Mutex"
            | "Rc"
            | "Cell"
            | "RefCell"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "f32"
            | "f64"
            | "bool"
            | "char"
            | "str"
            | "PathBuf"
            | "Path"
            | "Duration"
            | "Instant"
            | "BTreeMap"
            | "BTreeSet"
            | "Cow"
            | "OsString"
            | "CString"
    ) || word.starts_with(|c: char| c.is_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text() {
        let lines = render_md("hello world");
        assert_eq!(lines.len(), 1);
        let text: String = lines[0]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(text, "   hello world");
    }

    #[test]
    fn spinner_line() {
        let lines = render_md("  \u{27f3} read_file(/tmp/x)");
        let text: String = lines[0]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert!(text.contains("read_file"), "{text:?}");
    }

    #[test]
    fn inline_code() {
        let lines = render_md("use `Arc` here");
        let text: String = lines[0]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert!(text.contains("Arc"), "{text:?}");
    }

    #[test]
    fn code_block() {
        let lines = render_md("```rust\nfn main() {}\n```");
        assert!(lines.len() >= 3);
    }

    #[test]
    fn diff_line() {
        let lines = render_md("   + let x = 42;");
        let text: String = lines[0]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert!(text.contains('+'), "{text:?}");
        assert!(text.contains("let"), "{text:?}");
    }
}
