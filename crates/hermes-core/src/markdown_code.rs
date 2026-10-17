//! Fence classification for assistant replies (pure). A port of the Electron app's
//! `src/lib/markdown-code.ts` (`sanitizeLanguageTag`, `isLikelyProseCodeBlock`,
//! `isLikelyStructuredText`): a fenced block that is really wrapped prose renders as plain
//! wrapped text instead of a code card, exactly like the original. No regex crate: each
//! pattern is matched by hand and the original regex is quoted next to its matcher.

const NON_CODE_FENCE_LANGUAGES: &[&str] = &["", "text", "plain", "plaintext", "md", "markdown"];

const COMMON_CODE_LANGUAGES: &[&str] = &[
    "bash",
    "c",
    "cpp",
    "css",
    "diff",
    "go",
    "html",
    "java",
    "javascript",
    "js",
    "json",
    "jsx",
    "markdown",
    "md",
    "php",
    "python",
    "py",
    "ruby",
    "rust",
    "rs",
    "sh",
    "sql",
    "swift",
    "tsx",
    "ts",
    "typescript",
    "xml",
    "yaml",
    "yml",
];

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// `/^[a-z0-9][a-z0-9+#-]*$/i` on the first whitespace token, at most 16 chars, lowercased;
/// anything else is `""`.
pub fn sanitize_language_tag(tag: &str) -> String {
    let first = tag.split_whitespace().next().unwrap_or("");
    let mut chars = first.chars();
    let ok = match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() => {
            chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '#' | '-'))
        }
        _ => false,
    };
    if ok && first.len() <= 16 {
        first.to_ascii_lowercase()
    } else {
        String::new()
    }
}

/// Lines whose trimmed text starts with `/^[A-Za-z0-9"'`*-]/`.
fn prose_line_count(body: &str) -> usize {
    body.lines()
        .filter(|l| {
            l.trim()
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '"' | '\'' | '`' | '*' | '-'))
        })
        .count()
}

const KEYWORDS: &[&str] = &[
    "const", "let", "var", "function", "class", "import", "export", "return", "if", "for", "while", "switch",
];
const LINE_KEYWORDS: &[&str] = &["#include", "SELECT", "INSERT", "UPDATE", "DELETE", "CREATE", "DROP"];

/// Case-insensitive `kw` at byte `i` of `s`, followed by a word boundary.
fn word_at(s: &str, i: usize, kw: &str) -> bool {
    let end = i + kw.len();
    s.get(i..end).is_some_and(|w| w.eq_ignore_ascii_case(kw)) && s[end..].chars().next().is_none_or(|c| !is_word(c))
}

/// Sum of the three `CODE_SIGNAL_RE` match counts.
fn code_signal_count(body: &str) -> usize {
    let b = body.as_bytes();
    let mut n = 0;
    // 1. /(^|\s)(const|let|…|switch)\b/gim. The leading \s is consumed by a match, so two
    //    keywords separated by one space both count (each has its own preceding space).
    for (i, _) in body.char_indices() {
        let starts = i == 0 || body[..i].chars().next_back().is_some_and(char::is_whitespace);
        if starts && KEYWORDS.iter().any(|kw| word_at(body, i, kw)) {
            n += 1;
        }
    }
    // 2. /=>|==|===|!=|!==|\{|\}|;|<\/?[a-z][^>]*>/gi — alternatives tried left to right.
    let mut i = 0;
    while i < b.len() {
        let rest = &b[i..];
        if rest.starts_with(b"=>") || rest.starts_with(b"==") || rest.starts_with(b"!=") {
            n += 1;
            i += 2;
        } else if matches!(rest[0], b'{' | b'}' | b';') {
            n += 1;
            i += 1;
        } else if rest[0] == b'<' {
            let mut j = 1;
            if rest.get(j) == Some(&b'/') {
                j += 1;
            }
            match (rest.get(j), rest[j.min(rest.len())..].iter().position(|&c| c == b'>')) {
                (Some(c), Some(close)) if c.is_ascii_alphabetic() => {
                    n += 1;
                    i += j + close + 1;
                }
                _ => i += 1,
            }
        } else {
            i += 1;
        }
    }
    // 3. /^\s*(#include|SELECT|INSERT|UPDATE|DELETE|CREATE|DROP)\b/gim — once per line.
    for line in body.lines() {
        let t = line.trim_start();
        if LINE_KEYWORDS.iter().any(|kw| word_at(t, 0, kw)) {
            n += 1;
        }
    }
    n
}

/// `/\*\*[^*]+\*\*/` or `` /`[^`\n]+`/ `` anywhere.
fn has_markdown(s: &str) -> bool {
    // `**` + one or more non-`*` + `**`, tried at every `**`.
    let bold = s.match_indices("**").any(|(i, _)| {
        let rest = &s[i + 2..];
        let inner = rest.find('*').unwrap_or(rest.len());
        inner > 0 && rest[inner..].starts_with("**")
    });
    // `` ` `` + one or more non-`` ` ``/newline + `` ` ``.
    let code = s.match_indices('`').any(|(i, _)| {
        let rest = &s[i + 1..];
        let inner = rest.find(['`', '\n']).unwrap_or(rest.len());
        inner > 0 && rest[inner..].starts_with('`')
    });
    bold || code
}

/// `/^\s*[-*]\s+\S+/gm`.
fn bullet_line_count(s: &str) -> usize {
    s.lines()
        .filter(|l| {
            let t = l.trim_start();
            let mut c = t.chars();
            matches!(c.next(), Some('-' | '*')) && {
                let rest = c.as_str();
                rest.starts_with(char::is_whitespace) && !rest.trim().is_empty()
            }
        })
        .count()
}

/// `/^[A-Za-z0-9_][\w.-]*/` — length of the identifier prefix, if any.
fn config_key_len(s: &str) -> Option<usize> {
    let mut it = s.char_indices();
    match it.next() {
        Some((_, c)) if is_word(c) => {}
        _ => return None,
    }
    Some(
        it.find(|&(_, c)| !(is_word(c) || c == '.' || c == '-'))
            .map_or(s.len(), |(i, _)| i),
    )
}

/// `isConfigDirectiveLine`: `Key: value` / `Key = value`, or a 2–3 token `Key value`.
fn is_config_directive_line(line: &str) -> bool {
    let t = line.trim();
    if let Some(k) = config_key_len(t) {
        let rest = t[k..].trim_start();
        if let Some(v) = rest.strip_prefix([':', '='])
            && !v.trim_start().is_empty()
        {
            return true;
        }
    }
    let tokens: Vec<&str> = t.split_whitespace().collect();
    (2..=3).contains(&tokens.len()) && config_key_len(tokens[0]) == Some(tokens[0].len())
}

/// `isLikelyStructuredText`: config / key-value / indented listings stay fenced.
pub fn is_likely_structured_text(body: &str) -> bool {
    let lines: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() < 2 {
        return false;
    }
    // /^\s+\S/ — any indented line.
    if lines.iter().any(|l| l.starts_with(char::is_whitespace)) {
        return true;
    }
    // /[.!?](?:\s|$)/ — any sentence punctuation.
    let sentence = |l: &str| {
        let t = l.trim();
        t.char_indices()
            .any(|(i, c)| matches!(c, '.' | '!' | '?') && t[i + 1..].chars().next().is_none_or(char::is_whitespace))
    };
    if lines.iter().any(|l| sentence(l)) {
        return false;
    }
    let config = lines.iter().filter(|l| is_config_directive_line(l)).count();
    // Math.max(2, Math.ceil(n * 0.6)) without floats.
    config >= 2.max((lines.len() * 6).div_ceil(10))
}

/// `isLikelyProseCodeBlock(language, code)`: render this fence as wrapped prose.
pub fn is_likely_prose_code_block(language: &str, code: &str) -> bool {
    let lang = sanitize_language_tag(language);
    let trimmed = code.trim();
    let signals = code_signal_count(trimmed);
    if trimmed.is_empty() || signals >= 3 {
        return false;
    }
    let prose = prose_line_count(trimmed);
    if bullet_line_count(trimmed) >= 1 && (has_markdown(trimmed) || prose >= 2) {
        return true;
    }
    if is_likely_structured_text(code) {
        return false;
    }
    if NON_CODE_FENCE_LANGUAGES.contains(&lang.as_str()) {
        return prose >= 3 && signals == 0;
    }
    !COMMON_CODE_LANGUAGES.contains(&lang.as_str()) && prose >= 2 && signals <= 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_tags() {
        assert_eq!(sanitize_language_tag("Rust"), "rust");
        assert_eq!(sanitize_language_tag("c++ title=x"), "c++");
        assert_eq!(sanitize_language_tag("{python}"), "");
        assert_eq!(sanitize_language_tag("averyveryverylongtag"), "");
    }

    #[test]
    fn code_signals_match_the_original_regexes() {
        assert_eq!(code_signal_count("const x = 1;"), 2); // const + ;
        assert_eq!(code_signal_count("a => b == c"), 2);
        assert_eq!(code_signal_count("<div>hi</div>"), 2);
        assert_eq!(code_signal_count("SELECT * FROM t"), 1);
        assert_eq!(code_signal_count("constant letters"), 0); // \b after the keyword
        assert_eq!(code_signal_count("if for"), 2);
    }

    #[test]
    fn prose_fences_unwrap() {
        let prose = "This is a sentence about things.\nAnother line of plain words here.\nAnd a third.";
        assert!(is_likely_prose_code_block("", prose));
        assert!(is_likely_prose_code_block("text", prose));
        // Common code languages never unwrap unless bullets carry markdown.
        assert!(!is_likely_prose_code_block("rust", prose));
        assert!(is_likely_prose_code_block("", "- **one** item\n- two"));
    }

    #[test]
    fn code_and_config_stay_fenced() {
        assert!(!is_likely_prose_code_block("", "fn main() {\n    println!(\"hi\");\n}"));
        assert!(!is_likely_prose_code_block(
            "",
            "Host example\n    HostName 10.0.0.1\n    Port 22"
        ));
        assert!(!is_likely_prose_code_block("", "API_KEY=abc\nPORT=8080\nDEBUG=true"));
        assert!(!is_likely_prose_code_block("python", "x = 1"));
        assert!(!is_likely_prose_code_block("", "   "));
    }

    #[test]
    fn structured_text() {
        assert!(is_likely_structured_text("Host a\n  HostName b"));
        assert!(is_likely_structured_text("Port 22\nUser me\nHost x"));
        assert!(!is_likely_structured_text("Port 22. Done\nUser me"));
        assert!(!is_likely_structured_text("one line"));
    }
}
