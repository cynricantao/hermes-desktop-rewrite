//! Throwaway benchmark (never merged): syntect vs tree-sitter for code-block highlighting.
//! Prints init time, time to highlight a 500-line file (cold and warm), spans produced, RSS.
use std::time::Instant;

fn rss_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status").ok().and_then(|s| {
        s.lines().find(|l| l.starts_with("VmRSS:"))?.split_whitespace().nth(1)?.parse().ok()
    }).unwrap_or(0)
}

fn sample(lines: usize) -> String {
    let unit = include_str!("../sample.rs");
    let mut out = String::new();
    while out.lines().count() < lines { out.push_str(unit); }
    out.lines().take(lines).collect::<Vec<_>>().join("\n") + "\n"
}

fn main() {
    let code = sample(500);
    let rss0 = rss_kib();
    let t0 = Instant::now();
    #[cfg(feature = "syntect")]
    let (spans_cold, cold, warm, init) = {
        use syntect::easy::HighlightLines;
        use syntect::highlighting::ThemeSet;
        use syntect::parsing::SyntaxSet;
        let ss = SyntaxSet::load_defaults_newlines();
        let ts = ThemeSet::load_defaults();
        let theme = &ts.themes["base16-ocean.dark"];
        let syntax = ss.find_syntax_by_extension("rs").unwrap();
        let init = t0.elapsed();
        let run = || {
            let mut h = HighlightLines::new(syntax, theme);
            let mut n = 0usize;
            for line in syntect::util::LinesWithEndings::from(&code) {
                n += h.highlight_line(line, &ss).unwrap().len();
            }
            n
        };
        let t = Instant::now(); let spans = run(); let cold = t.elapsed();
        let t = Instant::now(); run(); let warm = t.elapsed();
        (spans, cold, warm, init)
    };
    #[cfg(feature = "ts")]
    let (spans_cold, cold, warm, init) = {
        use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};
        let names = ["attribute","comment","constant","function","keyword","number","operator","property","string","type","variable"];
        let mk = |lang: tree_sitter::Language, hl: &str, inj: &str, loc: &str| {
            let mut c = HighlightConfiguration::new(lang, "x", hl, inj, loc).unwrap();
            c.configure(&names);
            c
        };
        let rust = mk(tree_sitter_rust::LANGUAGE.into(), tree_sitter_rust::HIGHLIGHTS_QUERY, tree_sitter_rust::INJECTIONS_QUERY, "");
        let _py = mk(tree_sitter_python::LANGUAGE.into(), tree_sitter_python::HIGHLIGHTS_QUERY, "", "");
        let _js = mk(tree_sitter_javascript::LANGUAGE.into(), tree_sitter_javascript::HIGHLIGHT_QUERY, tree_sitter_javascript::INJECTIONS_QUERY, tree_sitter_javascript::LOCALS_QUERY);
        let _sh = mk(tree_sitter_bash::LANGUAGE.into(), tree_sitter_bash::HIGHLIGHT_QUERY, "", "");
        let init = t0.elapsed();
        let mut h = Highlighter::new();
        let mut run = || {
            let mut n = 0usize;
            for ev in h.highlight(&rust, code.as_bytes(), None, |_| None).unwrap() {
                if let HighlightEvent::HighlightStart(_) = ev.unwrap() { n += 1; }
            }
            n
        };
        let t = Instant::now(); let spans = run(); let cold = t.elapsed();
        let t = Instant::now(); run(); let warm = t.elapsed();
        (spans, cold, warm, init)
    };
    #[cfg(not(any(feature = "syntect", feature = "ts")))]
    let (spans_cold, cold, warm, init) = (code.len(), t0.elapsed(), t0.elapsed(), t0.elapsed());
    let rss1 = rss_kib();
    println!("init_ms={:.2} highlight500_cold_ms={:.2} highlight500_warm_ms={:.2} spans={} rss_before_kib={} rss_after_kib={} rss_delta_kib={}",
        init.as_secs_f64()*1e3, cold.as_secs_f64()*1e3, warm.as_secs_f64()*1e3, spans_cold, rss0, rss1, rss1 as i64 - rss0 as i64);
}
