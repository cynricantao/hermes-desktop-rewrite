//! ANSI SGR parsing for terminal / tool output (pure).
//!
//! Turns text with `ESC [ … m` colour sequences into styled runs per line. Other CSI /
//! OSC sequences (cursor movement, titles, hyperlinks) are dropped; `\r` rewinds to the
//! start of the line (progress bars overwrite themselves). Colours are resolved against a
//! caller-supplied 16-colour palette so light and dark themes keep their own tones.

/// Text attributes of one run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    pub fg: Option<Color>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
}

/// A colour reference: one of the 16 palette slots, or a direct RGB value (256-colour
/// cube / truecolor sequences).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Index(u8),
    Rgb(u8, u8, u8),
}

/// One styled run of text within a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub style: Style,
}

/// Parse `input` into lines of runs. Style carries across lines (as in a terminal).
pub fn parse(input: &str) -> Vec<Vec<Run>> {
    let mut lines = Vec::new();
    let mut cells: Vec<(char, Style)> = Vec::new();
    let mut cursor = 0;
    let mut style = Style::default();
    let mut chars = input.chars().peekable();

    fn finish_line(cells: &mut Vec<(char, Style)>) -> Vec<Run> {
        let mut runs: Vec<Run> = Vec::new();
        for (c, style) in cells.drain(..) {
            match runs.last_mut() {
                Some(last) if last.style == style => last.text.push(c),
                _ => runs.push(Run {
                    text: c.to_string(),
                    style,
                }),
            }
        }
        runs
    }

    fn put(cells: &mut Vec<(char, Style)>, cursor: &mut usize, c: char, style: Style) {
        if *cursor < cells.len() {
            cells[*cursor] = (c, style);
        } else {
            cells.push((c, style));
        }
        *cursor += 1;
    }

    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.peek() {
                Some('[') => {
                    chars.next();
                    let mut params = String::new();
                    // A control character aborts a malformed sequence. Leave it in the
                    // iterator so a newline or a new escape is handled normally.
                    while let Some(&p) = chars.peek() {
                        if p.is_control() {
                            break;
                        }
                        chars.next();
                        if ('\u{40}'..='\u{7e}').contains(&p) {
                            if p == 'm' {
                                apply_sgr(&mut style, &params);
                            }
                            break;
                        }
                        params.push(p);
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(&p) = chars.peek() {
                        if p == '\n' || p == '\r' {
                            break;
                        }
                        chars.next();
                        if p == '\u{7}' {
                            break;
                        }
                        if p == '\u{1b}' {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            },
            '\n' => {
                lines.push(finish_line(&mut cells));
                cursor = 0;
            }
            '\r' => cursor = 0,
            '\t' => {
                for _ in 0..4 {
                    put(&mut cells, &mut cursor, ' ', style);
                }
            }
            c if c.is_control() => {}
            c => put(&mut cells, &mut cursor, c, style),
        }
    }
    if !cells.is_empty() || lines.is_empty() {
        lines.push(finish_line(&mut cells));
    }
    lines
}

fn apply_sgr(style: &mut Style, params: &str) {
    let nums: Vec<u16> = if params.is_empty() {
        vec![0]
    } else {
        params.split([';', ':']).map(|p| p.parse().unwrap_or(0)).collect()
    };
    let mut i = 0;
    while i < nums.len() {
        match nums[i] {
            0 => *style = Style::default(),
            1 => style.bold = true,
            2 => style.dim = true,
            3 => style.italic = true,
            4 => style.underline = true,
            22 => {
                style.bold = false;
                style.dim = false;
            }
            23 => style.italic = false,
            24 => style.underline = false,
            n @ 30..=37 => style.fg = Some(Color::Index((n - 30) as u8)),
            n @ 90..=97 => style.fg = Some(Color::Index((n - 90 + 8) as u8)),
            39 => style.fg = None,
            38 => match nums.get(i + 1) {
                Some(5) => {
                    if let Some(&n) = nums.get(i + 2) {
                        style.fg = Some(xterm_256(n.min(255) as u8));
                    }
                    i += 2;
                }
                Some(2) => {
                    let c = |k: usize| nums.get(i + k).copied().unwrap_or(0).min(255) as u8;
                    style.fg = Some(Color::Rgb(c(2), c(3), c(4)));
                    i += 4;
                }
                _ => {}
            },
            // Backgrounds are ignored (the view has one background); skip their arguments.
            48 => match nums.get(i + 1) {
                Some(5) => i += 2,
                Some(2) => i += 4,
                _ => {}
            },
            _ => {}
        }
        i += 1;
    }
}

/// xterm 256-colour index → palette slot (0–15) or RGB.
fn xterm_256(n: u8) -> Color {
    match n {
        0..=15 => Color::Index(n),
        16..=231 => {
            let n = n - 16;
            let v = |k: u8| if k == 0 { 0 } else { 55 + k * 40 };
            Color::Rgb(v(n / 36), v((n / 6) % 6), v(n % 6))
        }
        _ => {
            let g = 8 + (n - 232) * 10;
            Color::Rgb(g, g, g)
        }
    }
}

/// Plain text of a parsed line (for copy / search).
pub fn line_text(runs: &[Run]) -> String {
    runs.iter().map(|r| r.text.as_str()).collect()
}

/// Strip all escape sequences.
pub fn strip(input: &str) -> String {
    parse(input).iter().map(|l| line_text(l)).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_one_run_per_line() {
        let l = parse("hello\nworld");
        assert_eq!(l.len(), 2);
        assert_eq!(line_text(&l[0]), "hello");
        assert_eq!(l[1][0].style, Style::default());
    }

    #[test]
    fn sgr_colours_and_reset() {
        let l = parse("\u{1b}[31merr\u{1b}[0m ok \u{1b}[1;92mgreen");
        assert_eq!(l[0][0].text, "err");
        assert_eq!(l[0][0].style.fg, Some(Color::Index(1)));
        assert_eq!(l[0][1].text, " ok ");
        assert_eq!(l[0][1].style, Style::default());
        assert_eq!(l[0][2].style.fg, Some(Color::Index(10)));
        assert!(l[0][2].style.bold);
    }

    #[test]
    fn extended_colours() {
        let l = parse("\u{1b}[38;5;196mA\u{1b}[38;2;1;2;3mB\u{1b}[38;5;244mC");
        assert_eq!(l[0][0].style.fg, Some(Color::Rgb(255, 0, 0)));
        assert_eq!(l[0][1].style.fg, Some(Color::Rgb(1, 2, 3)));
        assert_eq!(l[0][2].style.fg, Some(Color::Rgb(128, 128, 128)));
    }

    #[test]
    fn carriage_return_overwrites_progress() {
        let l = parse("10%\r50%\r100%\ndone");
        assert_eq!(line_text(&l[0]), "100%");
        assert_eq!(line_text(&l[1]), "done");
        assert_eq!(parse("a\r\nb").len(), 2);
    }

    #[test]
    fn carriage_return_preserves_the_unwritten_tail_and_style() {
        assert_eq!(strip("done\r"), "done");
        assert_eq!(strip("abc\rX"), "Xbc");
        let lines = parse("\u{1b}[31mabc\r\u{1b}[0mX");
        assert_eq!(lines[0][0].text, "X");
        assert_eq!(lines[0][0].style, Style::default());
        assert_eq!(lines[0][1].text, "bc");
        assert_eq!(lines[0][1].style.fg, Some(Color::Index(1)));
    }

    #[test]
    fn malformed_escapes_do_not_consume_later_lines() {
        assert_eq!(strip("before\u{1b}[31\nafter"), "before\nafter");
        assert_eq!(strip("before\u{1b}]unfinished title\nafter"), "before\nafter");
        assert_eq!(strip("before\u{1b}[31\u{1b}[32mafter"), "beforeafter");
    }

    #[test]
    fn other_sequences_are_dropped() {
        assert_eq!(strip("\u{1b}]0;title\u{7}\u{1b}[2Kline\u{1b}[1A"), "line");
        assert_eq!(strip("\u{1b}]8;;http://x\u{1b}\\link\u{1b}]8;;\u{1b}\\"), "link");
    }

    #[test]
    fn style_carries_across_lines_and_background_is_ignored() {
        let l = parse("\u{1b}[33;48;5;20ma\nb");
        assert_eq!(l[1][0].style.fg, Some(Color::Index(3)));
    }

    #[test]
    fn trailing_newline_does_not_add_a_line() {
        assert_eq!(parse("a\n").len(), 1);
        assert_eq!(parse("").len(), 1);
    }
}
