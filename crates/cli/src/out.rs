//! Terminal output helpers: colors (only on a TTY, respects NO_COLOR),
//! word wrapping and aligned tables. No extra dependencies.

use std::io::IsTerminal;
use std::sync::OnceLock;

fn color_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none())
}

pub fn paint(code: &str, s: &str) -> String {
    if color_enabled() {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

pub fn bold(s: &str) -> String {
    paint("1", s)
}
pub fn dim(s: &str) -> String {
    paint("2", s)
}
pub fn red(s: &str) -> String {
    paint("31", s)
}
pub fn green(s: &str) -> String {
    paint("32", s)
}
pub fn yellow(s: &str) -> String {
    paint("33", s)
}
pub fn blue(s: &str) -> String {
    paint("34", s)
}
pub fn cyan(s: &str) -> String {
    paint("36", s)
}
pub fn magenta(s: &str) -> String {
    paint("35", s)
}

pub fn term_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|c| c.parse().ok())
        .filter(|w: &usize| *w >= 40)
        .unwrap_or(110)
}

/// Visible width (ignores ANSI escapes; counts chars).
pub fn vis_len(s: &str) -> usize {
    let mut n = 0;
    let mut in_esc = false;
    for c in s.chars() {
        match (in_esc, c) {
            (false, '\x1b') => in_esc = true,
            (true, 'm') => in_esc = false,
            (true, _) => {}
            _ => n += 1,
        }
    }
    n
}

pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(8);
    let mut lines = vec![];
    for para in text.lines() {
        let mut cur = String::new();
        for word in para.split_whitespace() {
            if !cur.is_empty() && vis_len(&cur) + 1 + vis_len(word) > width {
                lines.push(std::mem::take(&mut cur));
            }
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(word);
        }
        lines.push(cur);
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

/// Render rows as an aligned table. The last column wraps to fit the terminal.
pub fn table(headers: &[&str], rows: &[Vec<String>], indent: usize) -> String {
    let cols = headers.len();
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for r in rows {
        for (i, c) in r.iter().enumerate().take(cols - 1) {
            widths[i] = widths[i].max(vis_len(c));
        }
    }
    let fixed: usize = widths[..cols - 1].iter().sum::<usize>() + 2 * (cols - 1) + indent;
    let last_w = term_width().saturating_sub(fixed).max(20);
    let pad = " ".repeat(indent);
    let mut out = String::new();
    let mut line = pad.clone();
    for (i, h) in headers.iter().enumerate() {
        let cell = if i + 1 < cols {
            format!("{h:<w$}  ", w = widths[i])
        } else {
            h.to_string()
        };
        line.push_str(&dim(&cell));
    }
    out.push_str(line.trim_end());
    out.push('\n');
    for r in rows {
        let wrapped = wrap(r.get(cols - 1).map(String::as_str).unwrap_or(""), last_w);
        let wrapped = if wrapped.is_empty() {
            vec![String::new()]
        } else {
            wrapped
        };
        for (li, w) in wrapped.iter().enumerate() {
            let mut line = pad.clone();
            for (i, width) in widths.iter().enumerate().take(cols - 1) {
                let cell = if li == 0 {
                    r.get(i).cloned().unwrap_or_default()
                } else {
                    String::new()
                };
                line.push_str(&cell);
                line.push_str(&" ".repeat(width - vis_len(&cell) + 2));
            }
            line.push_str(w);
            out.push_str(line.trim_end());
            out.push('\n');
        }
    }
    out
}

/// Mask values of secret-looking headers in printed output.
pub fn mask_header(name: &str, value: &str) -> String {
    let n = name.to_ascii_lowercase();
    let secret = n == "authorization"
        || n == "cookie"
        || n == "set-cookie"
        || n.ends_with("-key")
        || n.contains("token");
    if !secret || value.len() <= 4 {
        return value.to_string();
    }
    let keep: String = value.chars().take(6).collect();
    format!("{keep}••••••")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_and_table() {
        assert_eq!(wrap("one two three four", 9), ["one two", "three", "four"]);
        let t = table(&["A", "B"], &[vec!["x".into(), "hello".into()]], 2);
        assert_eq!(t, "  A  B\n  x  hello\n");
    }

    #[test]
    fn masks_secrets() {
        assert_eq!(
            mask_header("Authorization", "Bearer abcdef123"),
            "Bearer••••••"
        );
        assert_eq!(mask_header("X-Api-Key", "1234567890"), "123456••••••");
        assert_eq!(
            mask_header("Accept", "application/json"),
            "application/json"
        );
    }
}
