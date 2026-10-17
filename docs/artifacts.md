# Artifacts (`crates/hermes-core/src/artifacts.rs`)

Detection and tracking of content the agent produced: artifact cards promoted from fenced
code blocks, the versioned artifact registry behind them, the Artifacts **library** scan over
stored transcripts, and the preview-kind classification that decides how a produced file
opens. All UI-free and unit-testable without a window.

Ported from the Electron desktop (read-only sources under `~/.hermes/hermes-agent/apps/desktop`):

| Behaviour | Electron source |
|---|---|
| Fence → artifact detection (`code` / `html` / `svg`), titles, slugs, hashes, download names | `src/lib/artifact-detect.ts` |
| Prose-vs-code heuristics used by the detector | `src/lib/markdown-code.ts` (`isLikelyProseCodeBlock`, `sanitizeLanguageTag`, `isLikelyStructuredText`) |
| Versioned artifact registry, pruning, version selection | `src/store/artifacts.ts` |
| Library scan of assistant messages and tool payloads | `src/app/artifacts/artifact-utils.ts` |
| `MEDIA:` tags and media markdown hrefs | `src/lib/chat-messages/parts.ts`, `src/lib/media.ts` |
| Preview kinds, media kinds/mime, preview languages | `electron/main.ts` (preview-target normalization), `src/lib/media.ts` |
| Library ordering (newest first, 30 sessions) | `src/app/artifacts/index.tsx` |

## Public API

### Fence artifacts (cards)

* `detect_artifact(language, code) -> Option<Detection>` — exact port of `detectArtifact`.
  * HTML (`html|htm|xhtml`): a document marker (`<!doctype html`, `<html`, `<head`, `<body`)
    needs **≥ 160** chars; a fragment needs **≥ 1200** chars **and** a plausible `<tag>`.
    Title = `<title>` text, else `<h1>` text (tags stripped, 80 units), else `HTML`.
  * SVG: **≥ 2000** chars **and** an `<svg…>` open tag; title = `<title>`, else `SVG`.
  * Every language in `NON_ARTIFACT_LANGUAGES` (`diff`, `listing`, `log(s)`, `markdown`/`md`,
    `mermaid`, `output`, `patch`, `plain(text)`, `shell-session`, `stdout`, `text`/`txt`,
    empty) is refused — those fences belong to richer renderers or are prose.
  * Other languages: **≥ 3000** chars **or ≥ 48** lines, then the prose veto; title =
    filename comment (`// app.py`, `# server.ts`, `<!-- … -->`, `/* … */`) → declaration name
    (`function|class|struct|interface|enum|trait|impl|def|fn`, with optional
    `export|default|async`) → the language tag.
* `artifact_slug(detection) -> String` — `kind:language:title`, title lowercased with
  non-alphanumeric runs folded to `-`, capped at 48 UTF-16 units, `untitled` when empty.
* `artifact_content_hash(content) -> String` — FNV-1a over UTF-16 code units, base36;
  byte-for-byte the JS values (`"abc"` → `7aigaz`).
* `artifact_download_name(kind, language, title)` — filesystem-safe, keeps an existing
  extension, else appends by kind (`html`/`svg`) or language table, else `.txt`.

### Registry (versions the cards)

* `ArtifactRegistry` — `upsert`/`upsert_at`, `get`, `for_session`, `select_version`,
  `version_selection`, `selected_index`, `clear`, `preview_target`.
* Identity is `(session, slug)`: regenerating "the dashboard" three times is one artifact
  with three versions. Same slug + same content hash is a **no-op** (streaming remounts);
  new content appends a version (oldest drops past `MAX_VERSIONS_PER_ARTIFACT = 20`).
* Pruning after every accepted upsert (`pruneRegistry`):
  1. per session: keep the 24 most recently updated (`MAX_ARTIFACTS_PER_SESSION`), order by
     `created_at`;
  2. keep the `MAX_SESSIONS = 40` sessions with the most recent update;
  3. content budget `MAX_REGISTRY_CONTENT_UNITS = 4 Mi` UTF-16 units: evict the
     globally-oldest **historical** versions first (every record keeps its newest version);
     if still over, evict the oldest whole records — the registry's single newest record is
     always retained, content is never truncated;
  4. an explicitly selected version follows its **hash** through index shifts and snaps back
     to "newest" (absent = newest) when evicted. Selecting the newest clears the pin.
* `preview_target(record)` → the rail reference `{ kind: "artifact", label: title,
  source: url: record.id }` — tabs hold the id, never the content.

### Library (Artifacts page)

* `collect_artifacts_for_session(session, messages)` / `..._at(..., now_ms)` — port of
  `collectArtifactsForSession`. Found order = first occurrence in message order; de-dupe key
  is `${session.id}:${value}`.
  * Assistant text: `MEDIA:` tags (explicit deliveries), markdown images `![alt](href)`,
    markdown links (decoding `#media:` hrefs), bare `https?://` URLs, unix (`/`, `~/`, `./`,
    `../`, `\\`) and windows (`C:\`) paths.
  * Tool rows: only producer tools (`create*`, `download`, `export`, `generate*`, `render`,
    `save`, `speech`, `tts`, `write*`, plus legacy `bfl_flux3_*`) and `terminal` have their
    text scanned; `browser_vision` contributes `Screenshot path: …` captures; JSON payloads
    (and `<untrusted_tool_result>` wrappers) contribute string values whose key paths hit
    the strong key set (`artifact_*`, `file(s)_(created|modified|written)`,
    `generated_*`, `media_tag`, `output_*`, `result_*`, `saved_to`, `screenshot_path`) or,
    for producer tools, the generic set (`artifact(s)`, `attachment(s)`, `download(s)`,
    `media(_file|_path|_url)`, `audio|image|video(_file|_path|_url)`, `path`, `file_path`,
    `local_path`). `terminal` values under `output`/`stdout`/`path` are re-scanned as prose.
* `looks_like_artifact(value, explicit)` — pip traffic is filtered first (`~/.cache/pip`,
  `*.whl`, package-index `*.tar.gz|tar.bz2|zip|egg`); `http(s)://`/`data:image/` always
  qualify; otherwise a local path must end in a known image/file extension **or** be an
  explicit `MEDIA:` delivery.
* `normalize_value`, `decode_media_href_value`, `media_tag_values`, `artifact_href`,
  `artifact_label`, `library_kind`, `normalize_timestamp` (seconds < 1e10 → ms),
  `sort_newest_first` — the remaining `artifact-utils.ts` rules.
* `ScanSession` / `ScanMessage` — lenient `#[serde(default)]` row types;
  `ScanMessage::from_api(&api::sessions::Message)` adapts the REST transcript rows fetched
  with `api::sessions::session_messages_request` (the Electron library pages messages at
  100/request with `order: 'oldest'` over the 30 most recent sessions; paging stays with the
  caller, the scan is pure).

### Preview kinds

* `preview_kind(path, mime_type, binary) -> PreviewKind` — the pure part of Electron's
  normalization: `html (.html/.htm) → image (mime image/*) → pdf (.pdf or
  application/pdf) → binary → text`. `PreviewKind::Directory` / `Missing` are typed
  non-previewable results **the caller** decides with a filesystem probe (Electron #101683).
* `media_kind` / `media_mime` / `media_info` — `lib/media.ts` extension table (image, audio,
  video; default `File` / `application/octet-stream`).
* `is_markdown_document_path` (`.md|markdown|mdown|mkd` render as markdown),
  `is_html_preview_path`, `is_pdf_preview_path`, `preview_language_for_path`
  (`PREVIEW_LANGUAGE_BY_EXT`, default `text`), `is_artifact_file_path`,
  `is_file_media_path`, `path_to_local_file_url`.

### Transcript extraction (for the UI)

* `fence_artifacts(blocks)` — walks the rewrite's `Block::Text` (assistant) bodies through
  `markdown::split` (streaming-safe) and returns `FenceArtifact { detection, content,
  block_index, closed }` for every promoted fence; feed each to `ArtifactRegistry::upsert_at`
  (the `closed` flag lets the card render while the fence is still streaming).
* `library_artifacts_from_blocks(session, blocks, now_ms)` — the library rules applied to
  `Block::Text` + `Block::Tools` calls (`ToolCall::output` is scanned under the tool's own
  rules).

## Deviations from Electron (documented in the module docs)

* No filesystem probes (directory/missing) and no remote-gateway URL rewriting in
  `artifact_href` — the UI supplies both from its own connection state.
* `Date.now()` is injected (`upsert_at(..., now_ms)`, `collect_..._at(..., now_ms)`), so the
  logic is deterministic.
* `ScanMessage::from_api` has no `context` column (the REST row type does not carry one), so
  the Electron text fallback chain is `content → text`.
* Python `re`/JS regexes are hand-rolled scanners (the crate has no `regex` dependency);
  every threshold, alternation order and edge case is covered by tests.

## Tests

42 tests in `artifacts.rs` (`cargo test -p hermes-core --lib artifacts::`) using fixtures
copied from Electron's own suites (`src/lib/artifact-detect.test.ts`,
`src/store/artifacts.test.ts`, `src/app/artifacts/index.test.ts`): HTML document / h1
fallback / small snippet / small and large SVG / declaration and filename-comment titles /
prose and rich-renderer refusals; slug, hash (pinned against the JS implementation) and
download names; registry dedupe, versions, session scope, selection clamping, and all four
content-budget cases (historical eviction, whole-record eviction, oversized single record,
pinned-selection re-pointing); library link/media/screenshot/producer/shell fixtures,
passive-output exclusion, pip filtering, timestamp normalization and ordering; preview-kind,
media-kind and language classification; and `Block` extraction including the user-text
exclusion.
