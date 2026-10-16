# Syntax highlighting: syntect vs tree-sitter

**Status: decision pending. No dependency has been added.** The measurements come from a
throwaway branch (`throwaway/highlight-bench`, crate `crates/hl-bench`). That branch is
never merged.

## What was measured

There is one tiny binary per option, built with this workspace's release profile
(`lto = "fat"`, `codegen-units = 1`, `opt-level = 3`, `strip`, `panic = "abort"`), so the
size deltas match what `hermes-desktop-rs` would gain. Each binary:

1. Initialises the highlighter (`init`).
2. Highlights the same **500-line Rust file** twice: `cold` is the first run, `warm` the second.
3. Reports RSS before and after.

Dev machine (Omarchy, 12 threads), 5 runs each. The table gives medians; the spread across runs was ≤ 3 % except where noted.

| Option | Crates | Binary size | Δ vs baseline | init | 500 lines, cold | 500 lines, warm | RSS Δ |
|---|---|---:|---:|---:|---:|---:|---:|
| baseline (no highlighter) | — | 0.33 MB | — | — | — | — | +0.07 MiB |
| **syntect 5.3** (`default-syntaxes`, `default-themes`, `regex-fancy`, ~75 languages) | 8 new: syntect, fancy-regex, bincode, bit-set, bit-vec, aho-corasick, walkdir, same-file | 2.54 MB | **+2.2 MB** | 0.8 ms | **52 ms** (51–66) | 23 ms | **+12.3 MiB** |
| **tree-sitter 0.25** + highlight + 4 grammars (Rust, Python, JavaScript, Bash) | 10 new: tree-sitter, tree-sitter-highlight, tree-sitter-language, 4 grammar crates, regex, aho-corasick, streaming-iterator; plus `cc` at build time (C grammars) | 5.35 MB | **+5.0 MB** | **45 ms** (query compilation, 4 grammars) | **2.8 ms** | 2.7 ms | **+9.4 MiB** |

The current app binary is 22.3 MB, so syntect would add about 10 % and tree-sitter with 4
grammars about 23 %.

Notes on the numbers:

* syntect's `cold` run includes lazily compiling the regexes for Rust. That is a
  first-code-block cost, paid once per language.
* tree-sitter's `init` compiles the highlight queries of all four grammars (about 11 ms
  each). It could be done lazily per language.
* The span counts aren't comparable (syntect styles every token run, tree-sitter counts
  highlight captures), so they are not compared.
* Each extra tree-sitter grammar adds roughly 0.5–1.5 MB of parse tables. Getting near
  Shiki's language coverage (the Electron app) would cost tens of MB.

## Startup cost (first frame)

**Zero for both, if loaded lazily.** Nothing needs to load before the first frame. Create
the highlighter on a tokio worker the first time a fenced code block appears, and cache the
highlighted runs per block. Neither option should run on the UI thread: syntect's 23–52 ms
per 500 lines would drop frames.

While streaming, only the new lines of the open code block need re-highlighting. syntect
carries parse state line by line, which fits this naturally. tree-sitter would re-parse the
block incrementally.

## Trade-offs

| | syntect | tree-sitter |
|---|---|---|
| Language coverage | ~75 out of the box (Sublime grammars) | only the grammars we add, ~1 MB each |
| Highlight speed | adequate on a worker thread | ~18× faster |
| Accuracy | regex-based, occasional mis-highlights | real parse trees |
| Build | pure Rust (`regex-fancy`, no Oniguruma) | needs a C compiler (`cc`). Fine for the PKGBUILD (base-devel), but adds C to the build. |
| Theming | TextMate themes, easy to map to our tokens | capture names, which we map to our tokens ourselves |
| Memory once used | +12 MiB | +9 MiB (4 grammars) |

## Recommendation

**syntect** (`default-features = false`, `default-syntaxes`, `default-themes`,
`regex-fancy`):

* It is closest to parity: chat code blocks can be in any language, and the Electron app
  (Shiki) covers them all. tree-sitter would need a grammar crate and about 1 MB per
  language to match.
* Smaller binary: +2.2 MB, against +5.0 MB for just four languages.
* Pure-Rust build, no C toolchain.
* Its slower speed doesn't matter off the UI thread with per-block caching.

The cost to flag is **+12 MiB RSS once the first code block is highlighted**, which is half
the app's idle PSS. A follow-up could trim the bundled syntax set to the common languages
(this has not been measured). The size and memory figures would be re-measured in the real
app after integration.

**Needs your approval** before anything is added: syntect (plus the 7 transitive crates
listed above), or tree-sitter instead, or neither.
