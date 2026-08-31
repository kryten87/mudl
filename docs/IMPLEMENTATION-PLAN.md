# mudl Implementation Plan

`mudl` is a Linux port of `mud` — "A Perfect Markdown Viewer." `mud` is a
macOS-only SwiftUI/AppKit app (parsing via the C `cmark-gfm` library, hosted
in a `WKWebView`). `mudl` reimplements the same product for Ubuntu 22.04+, in
Rust, following the project's coding standards: minimal third-party
dependencies, clean layering, small pure functions with exhaustive tests,
strict TDD, and dependency injection at every impure boundary.

This document is the step-by-step build plan. It is organized as a series of
small, single-purpose steps grouped into phases. Steps within a phase are
mostly parallelizable (marked **[P]**) once their stated prerequisites are
met; steps marked **[S]** must happen in the given order relative to their
immediate neighbors. Each phase ends in a working, testable increment.

Source material for this plan came from a close reading of the `mud`
repository: `Core/Sources/**`, `Core/Tests/**`, `App/**`, `App/CLI/**`,
`Preferences/**`, and `Doc/**`. Where this plan says "port," it means
"reimplement the same externally observable behavior," not "translate the
Swift line-by-line" — the two languages and platforms don't have the same
seams.


## 1. Scope decision

`mud` has two tiers of features:

- **Core viewer**: parse a Markdown file, render it two ways (Up = styled
  HTML, Down = raw source with line numbers), auto-reload on save, folder
  tree/outline sidebars, themes, dark mode, zoom/word-wrap/readable-column
  toggles, find, CLI rendering tool, preferences.
- **Advanced/authoring-adjacent features**: git-integrated change tracking
  ("waypoints," word-level diff overlay), and a comments system that stores
  threaded discussion as GFM footnotes inside the document.

The advanced tier is large (it's roughly half of `Core/Sources` by file
count) and separable — nothing in the core viewer depends on it. This plan
builds the **core viewer first (Phases 0–12)**, so there's a real, useful,
fully-tested application as early as possible, then covers **change tracking
(Phase 13)** and **comments (Phase 14)** as later phases that plug into the
same architecture. If you want to change this ordering or drop the advanced
phases entirely, that's a config change to this document, not to the
architecture — nothing in Phases 0–12 is designed to make 13–14 harder.

Also out of scope, permanently (no Linux equivalent needed/wanted):

- macOS App Sandbox machinery (`AssetAccessStore`, `LocalAssetProbe`'s
  denied-vs-missing distinction, `BlockedAssetLog`, security-scoped
  bookmarks). Linux has no default sandbox denying file reads; a plain
  `ENOENT`/`EACCES` distinction from `std::fs` covers the same ground.
- Quick Look / Thumbnail Finder extensions (no Nautilus equivalent
  requested).
- Sparkle auto-update, Mac App Store build variant.
- `osascript`/AppleScript-based CLI installer privilege elevation — the
  Linux CLI installer is a plain symlink into `~/.local/bin`.


## 2. Architecture

A Cargo workspace of small, independently testable crates:

```
mudl/
  Cargo.toml                 # workspace root
  crates/
    mudl-core/               # pure: parsing orchestration, HTML rendering,
                              # frontmatter, slugs, themes, templates
    mudl-config/             # pure: preferences parse/validate; impure: load/save
    mudl-watch/               # file-change detection (trait + polling impl)
    mudl-server/              # local HTTP server (std::net) serving rendered
                              # content + assets to the webview
    mudl-cli/                 # `mudl` binary: flags, render-to-stdout, or
                              # launch the GUI
    mudl-gui/                 # GTK3 + WebKit2GTK application shell
  resources/
    css/                      # theme-*.css, mud.css, mud-up.css, mud-down.css, ...
    js/                       # highlight.min.js, mermaid.min.js, temml.min.js,
                              # mud-up.js, mud-down.js, mud-find.js, ...
  docs/
    IMPLEMENTATION-PLAN.md    # this file
```

Data flow for the GUI (Phase 10+):

```
file on disk --[mudl-watch]--> change event --[mudl-server]--> bump version
                                                    |
webkit2gtk WebView --http://127.0.0.1:PORT/doc--> [mudl-server] --calls--> mudl-core::render_up/render_down
                                                    |
                                     resources/css, resources/js served as static files
```

Data flow for the CLI (Phase 9):

```
mudl -u file.md --> mudl-cli parses flags --> mudl-core::render_up_to_html --> stdout
```

No HTTP server is involved in CLI rendering mode — that path is a pure
function call from argument-parsed input to a `String` written to stdout.

### Why a local HTTP server rather than a custom WebKit URI scheme

`WebKit2GTK` supports registering a custom URI scheme handler (the rough
equivalent of `mud`'s `mud-asset:` scheme), but that requires FFI-level
callback registration and produces a bespoke protocol only the app
understands. Instead, `mudl-server` runs a small HTTP server directly on
`std::net::TcpListener` bound to `127.0.0.1:0` (OS picks a free port) and the
WebView simply navigates to a normal `http://127.0.0.1:<port>/...` URL. This:

- keeps the request-routing and response-formatting logic as **plain,
  pure, testable functions** (a real TCP client — even `curl` — can exercise
  it in tests, no WebKit required),
- matches the project's standing guidance to prefer `std::net`/`std::io`
  over a framework,
- gives local image/relative-link serving, live-reload notification, and
  static asset serving a uniform interface (they're all just routes).

### Live reload without extra dependencies

`mudl-watch` maintains a `version: Arc<(Mutex<u64>, Condvar)>` bumped
whenever the watched file changes. `mudl-server` exposes a long-poll route
`GET /wait?since=<version>` that blocks (via the `Condvar`) until the version
advances past `since` or a timeout elapses, then returns the new version
number. A small bundled client script does:

```js
async function pollForever(since) {
  const next = await fetch(`/wait?since=${since}`).then(r => r.json());
  if (next.version !== since) location.reload();
  else pollForever(next.version); // timed out, no change — poll again
}
```

This gives near-instant reload with zero extra crates (no WebSocket
library, no async runtime).


## 3. Dependency policy and justified exceptions

Per the project's standards, the default is `std`. Every third-party crate
below is a deliberate exception with a specific justification — if a
reviewer can point at a std-only alternative that isn't a false economy
(i.e., wouldn't mean re-deriving a large, correctness-critical spec by
hand), prefer that instead.

| Dependency | Used by | Why std can't reasonably do this |
|---|---|---|
| `pulldown-cmark` | `mudl-core` | CommonMark + GFM (tables, strikethrough, task lists, footnotes, autolinks) is a large, precisely-specified grammar with hundreds of conformance edge cases. Hand-rolling a spec-compliant parser is exactly the kind of "Postgres wire protocol" scale effort the standards call out as a legitimate exception. `pulldown-cmark` is pure Rust (no C bindings), exposes byte offsets per event, and supports the needed GFM extensions via feature flags. |
| `gtk` (gtk-rs, GTK 3 bindings) | `mudl-gui` | Native window chrome, toolbar, split-pane sidebar, tree view, tabs. `std` has no windowing/widget capability on any platform. GTK 3 is Ubuntu 22.04's default desktop toolkit (`libgtk-3-dev` ships in the default repos), giving native look-and-feel for free. |
| `webkit2gtk` | `mudl-gui` | Embeddable web rendering surface to host the HTML `mudl-core` produces (styled Markdown, highlight.js, Mermaid, Temml). `std` has no HTML/CSS/JS engine. `libwebkit2gtk-4.1-dev` is available on Ubuntu 22.04. This is the direct Linux analog of `mud`'s AppKit + `WKWebView` pairing. |

Explicitly **not** taken as dependencies, and why the std/hand-rolled path is
fine:

- **JSON parsing** — the only JSON in scope is `mud`'s bundled
  `emoji.json` (a static, one-time data asset, not user input). Converted
  once by a small offline script into a checked-in Rust source file (a
  `&[(&str, &str)]` array of shortcode → emoji), so no JSON parser is ever
  invoked at runtime. See Phase 1, step 1.6.
- **Base64 encoding** (for `--standalone` image data URIs) — a ~20-line,
  fully pure, exhaustively-testable algorithm. Hand-rolled in
  `mudl-core::encoding::base64`.
- **Config file format (preferences)** — rather than pull in `toml` +
  `serde`, `mudl-config` uses a tiny hand-rolled `key = value` line format
  (see Phase 8). Parsing it is a small, pure state machine, easy to test
  exhaustively, and avoids a whole dependency family.
- **CLI argument parsing** — hand-rolled over `std::env::args()`; `mud`'s
  own flag set is small and simple enough not to need `clap`.
- **File watching** — polling via `std::fs::metadata(...).modified()` on a
  background thread (Phase 7), not `inotify`. This is a deliberate
  simplicity-over-latency tradeoff: polling needs no dependency (not even
  `libc`), and the trait boundary (`ChangeSource`) means an `inotify`-backed
  implementation can be swapped in later without touching any consumer.
  Flagged as an explicit future optimization, not a v1 requirement.
- **Syntax highlighting, math rendering, Mermaid diagrams** — all three
  are pushed to the client side as bundled JS (see §4), so no
  `syntect`/TeX-rendering crate is needed in Rust at all. `mudl-core`'s job
  for these is only to emit the right semantic HTML (`<pre><code
  class="language-X">`, escaped source) and let the browser do the rest,
  exactly as `mud` already does for Mermaid today.


## 4. Rendering split: what Rust does vs. what bundled JS does

This is the single biggest scope-reduction decision in this plan, so it's
worth stating plainly:

- **`mudl-core` (Rust, pure)** parses Markdown, walks the resulting AST, and
  emits final HTML: block/inline structure, tables, task lists, footnotes,
  GFM alerts / DocC-style asides, frontmatter (collapsible `<details>`),
  heading slugs, emoji shortcode substitution, HTML escaping, the
  surrounding document template (theme `<link>`s, CSP, conditional
  script/style inclusion), and — critically — code blocks and math blocks
  are emitted as **plain escaped text** tagged with a language/kind class,
  not highlighted or typeset.
- **Bundled JS (static assets, run inside the WebView, unmodified or
  lightly adapted from `mud`'s own resources)** does syntax highlighting
  (`highlight.min.js`), math typesetting (`temml.min.js`), and diagram
  rendering (`mermaid.min.js`) entirely client-side, on page load. This is
  already exactly how `mud` handles Mermaid; this plan just extends the same
  treatment to highlighting and math instead of reimplementing
  `CodeHighlighter`/`MathRenderer`'s JavaScriptCore-hosted logic in Rust.

Practical effect: `Rendering/BundledJSContext.swift`, `CodeHighlighter.swift`,
`MathRenderer.swift`, and `HTMLLineSplitter.swift` (which exists only to
re-split *already-highlighted* HTML spans across line boundaries) have **no
Rust port at all** — the equivalent behavior (per-line wrapping for the Down
mode gutter, syntax coloring) is handled by adapting `mud`'s existing
`mud-down.js`/`mud-up.js` client scripts, which already run in the browser.


## 5. Cross-cutting conventions

### 5.1 The TDD cadence (applies to every unit of pure logic below)

For each function or small cluster of related functions:

1. **Scaffold** — add the function/module signature (types, no logic body
   beyond `todo!()`/a trivial default) so the test file compiles and a test
   run fails on an assertion, never on a missing item or type error.
2. **RED** — write the test(s) for the next smallest slice of behavior;
   run and confirm they fail for the right reason (wrong output, not a
   panic from `todo!()` unless that's the point of the first test).
3. **GREEN** — write the minimum implementation to pass.
4. **Refactor** — clean up under green tests.

This plan does not spell out all four steps for every single function
below (that would 4x the length without adding information) — each step
description gives the target signature and the test cases to cover; apply
the cadence to get there.

### 5.2 Dependency injection boundaries

Every impure dependency is a trait, injected, never constructed inline:

```rust
trait Clock { fn now(&self) -> SystemTime; }
trait FileSystem {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;
    fn metadata_modified(&self, path: &Path) -> io::Result<SystemTime>;
    fn write_atomic(&self, path: &Path, contents: &[u8]) -> io::Result<()>;
}
trait ChangeSource { fn poll(&mut self) -> Option<ChangeEvent>; }
```

Production code gets `RealClock`, `RealFileSystem` (thin wrappers over
`SystemTime::now()` / `std::fs`), and a `PollingChangeSource`. Tests use
`FakeClock` (settable), `InMemoryFileSystem` (a `HashMap<PathBuf, Vec<u8>>`),
and a `ScriptedChangeSource` (a `VecDeque<ChangeEvent>`). No test in
`mudl-core`, `mudl-config`, or the pure parts of `mudl-server`/`mudl-watch`
touches the real filesystem or clock.

### 5.3 Directory/module conventions

- Every crate's pure logic lives in modules with `#[cfg(test)] mod tests`
  blocks alongside the code (idiomatic Rust convention), not in a separate
  top-level `tests/` tree — except black-box integration tests (e.g. real
  TCP requests against `mudl-server`, golden-file HTML snapshots), which do
  belong in `tests/`.
- Golden/snapshot fixtures (input Markdown + expected HTML) live under
  `crates/mudl-core/tests/golden/`, mirroring `mud`'s `Core/Tests/Golden/`
  approach — ported test cases, not ported test code.
- `cargo fmt` and `cargo clippy --all-targets -- -D warnings` must be clean
  before a step is considered done.


## 6. Phase 0 — Workspace scaffolding

0.1 **[S]** Create the Cargo workspace root (`Cargo.toml` with
    `[workspace] members = [...]`) and empty crate skeletons for
    `mudl-core`, `mudl-config`, `mudl-watch`, `mudl-server`, `mudl-cli`,
    `mudl-gui`, each with a `lib.rs` (or `main.rs` for `mudl-cli`) containing
    only a passing placeholder test (`#[test] fn crate_compiles() {
    assert!(true); }`). Green state: `cargo test --workspace` passes with 6
    trivial tests, `cargo clippy --workspace` is clean.

0.2 **[P]** Add `rustfmt.toml` and `clippy.toml` (or document the default
    lint set) at the workspace root; add a `Makefile` or `justfile` with
    `test`, `fmt`, `lint` targets — convenience only, no logic to test.

0.3 **[P]** Set up CI (GitHub Actions or equivalent, matching whatever
    `mud`'s `.github/` already uses for style) running `cargo test`,
    `cargo fmt --check`, `cargo clippy -- -D warnings` on Ubuntu 22.04,
    including `apt-get install libgtk-3-dev libwebkit2gtk-4.1-dev` (needed
    once `mudl-gui` has real code, but harmless to add now).

0.4 **[P]** Copy `mud`'s `Doc/LICENSE.md` (MIT with Commons Clause) to
    `mudl/LICENSE.md`, adjusted for the new repo if the license terms
    differ; note in `mudl/README.md` that bundled JS/CSS assets originate
    from `mud` and retain their own upstream licenses (highlight.js,
    Mermaid, Temml are all separately-licensed open-source projects `mud`
    already vendors — carry their license files over unchanged).


## 7. Phase 1 — Pure core: parser-independent building blocks

These have no dependency on `pulldown-cmark` at all and can start
immediately and in parallel with Phase 0's later steps.

1.1 **[P]** `mudl-core::encoding::html_escape(s: &str) -> String` — escapes
    `&`, `<`, `>`, `"` (not `'`, matching `mud`). Tests: empty string, no
    special chars, all four specials, mixed, string that already contains
    literal `&amp;` (must still escape the `&`, no smart double-escape
    detection — matches `mud`'s behavior).

1.2 **[P]** `mudl-core::encoding::base64_encode(bytes: &[u8]) -> String` —
    standard Base64 (RFC 4648, with padding). Tests: empty input, 1/2/3-byte
    inputs (each padding case), known test vectors (`"Zg=="`, `"Zm8="`,
    `"Zm9v"` for `"f"`/`"fo"`/`"foo"`), longer multi-block input.

1.3 **[P]** `mudl-core::slug::slugify(text: &str) -> String` and
    `mudl-core::slug::Tracker` (a `HashMap<String, usize>`-backed
    de-duplicator giving `-1`, `-2`, ... suffixes to repeats). Tests: plain
    text, punctuation stripped, leading/trailing space trimmed, internal
    whitespace runs collapsed to one hyphen, Unicode word characters
    preserved (e.g. `Ñoño` → `ñoño`), empty string → empty string,
    already-slugged input is idempotent, literal hyphens preserved
    (including the `"A - B"` → `"a---b"` edge case — space-hyphen-space
    becomes three hyphens, matching `mud` exactly), numbers preserved;
    `Tracker`: first occurrence unsuffixed, second occurrence of the same
    slug gets `-1`, a third gets `-2`, an unrelated slug is unaffected.

1.4 **[P]** `mudl-core::frontmatter::extract(markdown: &str) ->
    Option<FrontMatter>` where `FrontMatter { yaml: String, body: String,
    line_count: usize }`, plus `parse_top_level_keys(yaml: &str) ->
    Vec<KeyValue>` (`KeyValue { key: String, value:
    FrontMatterValue }`, `FrontMatterValue::{Scalar, InlineArray, Block}`).
    Tests (drawn directly from `mud`'s `FrontMatterExtractorTests`):
    standard frontmatter; empty frontmatter body; closing delimiter is
    `...` instead of `---`; no closing delimiter found → `None`; opening
    delimiter not on line 1 → `None`; text before the opening delimiter →
    `None`; trailing whitespace after either delimiter is tolerated; CRLF
    input; no frontmatter present → `None`; frontmatter-only document
    (with and without a trailing newline); a `---` thematic break appearing
    later in the document body must not be mistaken for a frontmatter
    delimiter; simple scalar values; inline arrays (`[a, b, c]`); block
    (multi-line) arrays; nested mappings; multiline literal (`|`) and
    folded (`>`) block scalars; quoted values preserved verbatim including
    their quote characters; comment lines (`#`) between keys ignored;
    all-comment YAML body → empty key list; empty input → empty key list;
    a document mixing simple and complex-valued keys.

1.5 **[P]** `mudl-core::alerts` — two independent pure detectors:
    `detect_gfm_alert(first_line: &str) -> Option<(AlertCategory, ())>` for
    the fixed `[!NOTE]`/`[!TIP]`/`[!IMPORTANT]`/`[!WARNING]`/`[!CAUTION]`/
    `[!STATUS]` prefixes, and `parse_aside_tag(first_line: &str) ->
    Option<(String, usize)>` (tag name + byte length consumed) plus
    `detect_docc_aside(tag: &str, mode: DocCAlertMode) -> Option<AlertCategory>`
    for `Note:`/`Tip:`/`Warning:`/etc. and the ~20 extended aliases
    (`Remark:`, `Precondition:`, `Bug:`, ...), gated by `off`/`common`/
    `extended`. Tests: unrecognized tag → `None`; tagless quote → `None`;
    `off` mode always `None`; `common` mode matches core kinds but not
    extended aliases; a pathological "tag shift" input (very short string,
    tag longer than remaining content) must return `None`, not panic — this
    is a direct regression test carried over from `mud`'s own fuzz-shaped
    edge case; correct byte-length accounting that skips the tag, the colon,
    and any trailing whitespace.

1.6 **[S, depends on nothing but is a one-time offline task, not a runtime
    step]** Write a small one-off script (Python or a `mudl-xtask` binary —
    either is fine, it is a developer tool, not part of the shipped
    product) that reads `mud`'s `Core/Sources/Resources/emoji.json` and
    emits `crates/mudl-core/src/emoji_data.rs` containing a checked-in
    `pub(crate) const EMOJI_SHORTCODES: &[(&str, &str)] = &[...]`. Run it
    once, commit the generated file, delete the script's runtime
    dependency footprint (it never runs again as part of `cargo build`).

1.7 **[P, depends on 1.6]** `mudl-core::emoji::replace_shortcodes(text: &str)
    -> String` using `EMOJI_SHORTCODES`. Tests: known shortcode replaced;
    shortcode containing `+`/`-` characters; unknown shortcode left
    unchanged; input with no `:` at all takes the fast path unchanged;
    mixed known/unknown/plain text; consecutive shortcodes; empty content
    between colons (`::`) is not a match; a colon-heavy string that isn't
    actually a shortcode (e.g. resembling a time format) is left alone.

1.8 **[P]** `mudl-core::footnotes::is_comment_label(label: &str) -> bool` —
    `comment-` prefix followed only by `[\w-]+`. Tests: valid label, empty
    suffix → `false`, suffix containing an invalid character → `false`,
    no prefix → `false`. (This predicate is needed even in the core-viewer
    phase to correctly *ignore* comment footnotes if a document happens to
    already contain them, even before Phase 14 adds comment authoring.)

1.9 **[P]** `mudl-core::images::is_external_source(source: &str) -> bool` —
    checks `http://`, `https://`, `data:`, `mailto:` prefixes,
    case-insensitively. Tests: each prefix in various cases, a relative
    path, an absolute local path, a bare filename — all non-external cases
    return `false`.

1.10 **[P]** `mudl-core::images::classify(source: &str, base_dir: &Path) ->
    Option<(PathBuf, &'static str)>` — pure path-resolution + MIME-type
    lookup (by extension: png/jpg/jpeg/gif/svg/webp) for a local image
    reference, given only a source string and a base directory (no
    filesystem access). Returns `None` for external sources. Tests: each
    supported extension, unknown extension → `None`, relative path
    resolved against `base_dir`, external source → `None`.

    Note the *impure* counterpart (`read` + base64-encode) is deliberately
    deferred to Phase 5, where it's injected via the `FileSystem` trait.


## 8. Phase 2 — Markdown parsing integration (`pulldown-cmark`)

2.1 **[S]** Add `pulldown-cmark` to `mudl-core` with the `tables` and
    `strikethrough` extension options enabled (footnotes handling is
    revisited in Phase 14; task-list parsing is a `pulldown-cmark` built-in).
    Scaffold `mudl-core::parse::ParsedMarkdown { events: Vec<(Event<'a>,
    Range<usize>)> }` wrapping `Parser::new_ext(...).into_offset_iter()`.
    First test: parsing produces at least one event for `"# Hello"` — a
    smoke test only, not yet asserting exact structure.

2.2 **[S]** `mudl-core::render::render_up(markdown: &str, options:
    &RenderOptions) -> String` — a visitor over the `pulldown-cmark` event
    stream producing Up-mode HTML, calling out to the Phase 1 pure helpers
    (`html_escape`, `slugify`+`Tracker` for headings, `replace_shortcodes`
    for text nodes, `detect_gfm_alert`/`detect_docc_aside` for blockquote
    openings). Build this incrementally, test-first, in the order
    CommonMark elements are introduced: paragraphs → emphasis/strong →
    inline code → links/images → headings → lists (loose vs. tight) →
    blockquotes → code fences (language class only, no highlighting) →
    thematic breaks → hard line breaks → HTML blocks (passthrough,
    escaped per CommonMark's raw-HTML rules). Each element gets its own
    small red/green cycle with a handful of golden-style input→output
    pairs (short inline fixtures, not yet full golden files).

2.3 **[S, depends on 2.2]** Extend `render_up` for GFM extensions: tables
    (cell alignment classes), strikethrough, task list items (`- [ ]`/`-
    [x]` → disabled checkbox input), autolinks. Test each extension's
    happy path plus at least one malformed/edge input (unterminated table
    row, empty table, nested strikethrough, mixed checked/unchecked list).

2.4 **[P, depends on 2.2]** `mudl-core::render::render_down(markdown: &str,
    options: &RenderOptions) -> String` — Down-mode raw-source view: split
    `markdown` into `<div class="line" data-line="N">` elements
    (`str::lines()`-based, trivial — this is the one place `mud`'s
    `HTMLLineSplitter` would have mattered, and it doesn't here because
    Rust never emits highlighted-spans-crossing-newlines; highlighting is
    entirely client-side per §4), each line HTML-escaped, with line numbers
    added via the wrapping structure (actual numbering is CSS-driven,
    matching `mud`'s approach). Tests: empty document, single line, no
    trailing newline doesn't produce a phantom trailing empty line,
    multiple blank lines preserved, a line containing `<`/`&` is escaped
    exactly once.

2.5 **[P, depends on 2.2]** `mudl-core::headings::extract_headings(markdown:
    &str) -> Vec<OutlineHeading>` (`OutlineHeading { level: u8, id: String,
    segments: Vec<OutlineTextSegment> }`, `OutlineTextSegment::{Plain(String),
    Code(String)}`) — reuses the *same* `slugify`+`Tracker` instance/state
    that `render_up` uses for `id=` attributes, so sidebar and document IDs
    are guaranteed to match (this parity requirement is carried over
    directly from `mud`, where a dedicated test enforces it — port that
    test as `heading_ids_match_render_up_ids`). Tests: single heading,
    multiple headings at different levels, heading containing inline code/
    emphasis/a link, empty document → empty vec, duplicate heading text →
    deduplicated slugs, a soft line break inside a heading becomes a space
    segment.

2.6 **[P, depends on 2.1]** `mudl-core::frontmatter_html::render_table(keys:
    &[KeyValue]) -> String` — renders parsed frontmatter (from Phase 1.4)
    as a collapsible `<details><table>`, falling back to a `<pre><code>`
    block if rendering as a table isn't sensible (e.g. deeply nested
    values). Tests: simple scalar keys, an inline array value, a block
    value, empty key list → minimal/empty output.


## 9. Phase 3 — Document template & theme assembly

3.1 **[P]** `mudl-core::template::HtmlDocument` — a pure builder struct
    (title, base dir, style hrefs, script srcs, CSP source lists, body
    classes) with `render(&self) -> String`. Break into small pure
    sub-functions exactly as `mud` does: `build_csp(sources: &[&str]) ->
    String`, `build_html_attributes(classes: &[&str]) -> String`,
    `build_scripts(srcs: &[&str]) -> String`. Tests per sub-function: empty
    list, one entry, several entries; zero-classes vs. some; a `zoom == 1.0`
    special case emits no inline style attribute, a non-1.0 zoom does.

3.2 **[P]** `mudl-core::template::js_string_literal(s: &str) -> String` —
    produces a bare JS string literal (quotes included) safe to splice into
    an inline `<script>`. Tests: empty string, string containing double
    quotes, string containing a backslash, string containing Unicode.

3.3 **[S, depends on 3.1]** `mudl-core::template::select_assets(body_html:
    &str, options: &RenderOptions) -> AssetSelection` (`AssetSelection {
    stylesheets: Vec<&'static str>, scripts: Vec<&'static str> }`) — the
    pure decision logic for which bundled CSS/JS to include: math CSS only
    if `body_html` contains a math marker (`temml`/`mud-math-block`),
    Mermaid script only if a `language-mermaid` code block is present,
    highlight.js only if any code fence is present, find-feature CSS only
    in non-standalone (interactive) mode, narrow-layout and print CSS
    always included last, in a fixed order. Tests: each marker present/
    absent independently, standalone mode suppresses find CSS, order of
    the returned list is deterministic and matches the fixed convention.

3.4 **[P]** Copy `mud`'s theme CSS files (`mud.css`, `mud-up.css`,
    `mud-down.css`, `mud-narrow.css`, `mud-print.css`, `mud-find.css`,
    `mud-math.css`, `theme-austere.css`, `theme-blues.css`,
    `theme-earthy.css`, `theme-riot.css`, `theme-system.css`) into
    `mudl/resources/css/` verbatim as a starting point (they're static
    design assets, not Swift logic — no port needed, only later visual
    tweaks if Linux font-rendering differences call for them). Embed via
    `include_str!` in `mudl-server`/`mudl-core` at compile time rather than
    reading from disk at runtime (no runtime filesystem dependency for
    static assets at all).

3.5 **[P]** Copy `highlight.min.js`, `mermaid.min.js`, `temml.min.js`
    (third-party, already MIT/BSD-licensed and vendored by `mud`) into
    `mudl/resources/js/` unchanged, along with their license notices.

3.6 **[S, depends on 3.4/3.5]** Port/adapt `mud`'s `mud.js`, `mud-up.js`,
    `mud-down.js` client scripts: the parts responsible for invoking
    `highlight.min.js`/`temml.min.js`/`mermaid.min.js` on page load, scroll
    position save/restore, and body-class toggling driven from a small
    query-string or `data-*` attribute contract (replacing `mud`'s
    Swift-to-JS bridge calls, since there is no native bridge in this
    architecture — see Phase 10 for how the GTK shell instead talks to the
    page via simple URL navigations / injected `<script>` tags at load
    time). This step is JS engineering, not Rust, and isn't strict-TDD in
    the Rust sense; verify it with a manual checklist (open a doc with a
    fenced code block, a `$$ ... $$` math block, and a ` ```mermaid ` block;
    confirm all three render) once Phase 10's WebView exists.


## 10. Phase 4 — Local HTTP server (`mudl-server`)

4.1 **[P]** Pure request-line parsing:
    `mudl_server::http::parse_request_line(line: &str) -> Option<Request>`
    (`Request { method: Method, path: String, query: HashMap<String,
    String> }`). Tests: well-formed `GET /foo?a=1&b=2 HTTP/1.1`, missing
    HTTP version, missing method, empty path, path with percent-encoded
    characters (decode them), malformed query string, empty input.

4.2 **[P]** Pure response formatting:
    `mudl_server::http::format_response(status: u16, headers: &[(&str,
    &str)], body: &[u8]) -> Vec<u8>`. Tests: 200 with a body, 404 with no
    body, headers correctly joined with `\r\n`, `Content-Length` is
    computed from the actual body length (not trusted from a caller-supplied
    header).

4.3 **[P]** Pure MIME-type lookup:
    `mudl_server::mime::lookup(extension: &str) -> &'static str` covering
    `.html`, `.css`, `.js`, `.png`, `.jpg`/`.jpeg`, `.gif`, `.svg`, `.webp`,
    with a fallback of `application/octet-stream`. Tests: each known
    extension (including uppercase variants), unknown extension, empty
    extension.

4.4 **[P]** Pure route dispatch (no I/O): `mudl_server::routes::dispatch(req:
    &Request) -> Route` mapping `/` → `Route::Document`, `/assets/<name>`
    → `Route::Asset(name)`, `/local/<encoded-path>` → `Route::LocalFile(path)`
    (percent-decoded), `/wait?since=N` → `Route::WaitForChange(n)`, anything
    else → `Route::NotFound`. Tests: each route shape, malformed
    `/local/` path (missing/invalid encoding) → `NotFound`, missing `since`
    query param on `/wait` → treated as `since=0`.

4.5 **[S, depends on 4.1–4.4]** Wire the impure server loop:
    `TcpListener::bind("127.0.0.1:0")`, accept loop (thread-per-connection
    is fine at this scale — a markdown viewer serves a handful of
    concurrent requests, not production web traffic), reading one HTTP
    request per connection with `std::io::BufReader`, dispatching via 4.4,
    and producing a response via 4.2. Integration test: start the server in
    a test, connect with `std::net::TcpStream` directly, assert on raw
    bytes for a couple of routes (`/assets/mud.css` returns the embedded
    CSS with the right `Content-Type`; an unknown route returns 404).

4.6 **[S, depends on 4.5, mudl-watch Phase 7]** Implement `/wait` using the
    `Condvar`-based version counter described in §2; inject the
    `ChangeSource`/version state so tests can bump it programmatically and
    assert the long-poll request unblocks promptly, and separately assert
    it times out and returns the *unchanged* version when nothing happens.


## 11. Phase 5 — Local asset serving (images, `--standalone` export)

5.1 **[S, depends on 1.10]** `mudl-core::images::encode_data_uri(source:
    &str, base_dir: &Path, read: &dyn Fn(&Path) -> io::Result<Vec<u8>>) ->
    Option<String>` — the impure half of image handling, with the actual
    file read injected as a closure/trait object per the DI convention, so
    the test suite exercises it with an in-memory fake and never touches a
    real file. Combines Phase 1.10's pure classification with Phase 1.2's
    `base64_encode`. Tests (using a fake `read`): known extension + fake
    bytes → correct `data:image/...;base64,...` string; external source →
    `None`; fake `read` returning an error → `None`; empty file bytes →
    valid (empty) data URI, not a panic.

5.2 **[P, depends on 4.4]** For the (non-standalone) served-document path:
    rewrite relative image `src` attributes in rendered HTML to
    `/local/<percent-encoded-absolute-path>` at render time, and implement
    `Route::LocalFile` to read that path (via the injected `FileSystem`
    trait) and serve it with the Phase 4.3 MIME lookup. Test the pure
    rewrite function (`rewrite_local_image_srcs(html: &str, base_dir: &Path)
    -> String`) directly on HTML fixtures; test the route handler with the
    in-memory fake filesystem.


## 12. Phase 6 — File watching (`mudl-watch`)

6.1 **[P]** Define the `ChangeSource` trait (§5.2) and a
    `PollingChangeSource<F: FileSystem, C: Clock>` that checks
    `metadata_modified` on an interval (default 300ms — tunable, not
    hardcoded) and reports a change when the modification time advances.
    Tests (using `InMemoryFileSystem` + `FakeClock`, no real sleeping):
    first poll after construction reports no change (baseline), a
    subsequent poll after the fake mtime advances reports a change, an
    unchanged mtime across multiple polls reports nothing, a file that
    disappears (read error) is reported as a distinct `ChangeEvent::Removed`
    rather than silently swallowed, a file that reappears with a new mtime
    after being removed is reported as `ChangeEvent::Changed` again.

6.2 **[S, depends on 6.1]** Background-thread wiring:
    `PollingChangeSource::spawn(path, interval) -> WatchHandle` running the
    poll loop on a dedicated thread, feeding events into the `mudl-server`
    version counter from §2/Phase 4.6. Not unit-tested in the strict sense
    (it's a thin thread-spawning shell around 6.1's tested logic); verified
    by an integration test that writes to a real temp file and asserts the
    HTTP `/wait` endpoint unblocks within a bounded time.

6.3 **[P]** Document, in a code comment on the `ChangeSource` trait, the
    intended future `inotify`-backed implementation and why it's deferred
    (§3's dependency table) — so a later contributor doesn't need to
    re-derive the reasoning.


## 13. Phase 7 — Preferences (`mudl-config`)

7.1 **[P]** Design the on-disk format: a flat `key = value` text file (one
    per line, `#`-prefixed comments, blank lines ignored), at
    `~/.config/mudl/preferences`. Pure parser:
    `mudl_config::format::parse(text: &str) -> Vec<(String, String)>`.
    Tests: empty file, comment-only file, a single key, multiple keys,
    a line with extra whitespace around `=`, a malformed line with no `=`
    (skipped, not an error), a duplicate key (last occurrence wins), a
    value containing an `=` character (only the first `=` splits key from
    value), trailing/leading blank lines.

7.2 **[P]** Pure serialization (round-trip partner of 7.1):
    `mudl_config::format::serialize(entries: &[(String, String)]) ->
    String`. Test round-trip: `parse(serialize(entries)) == entries` for a
    representative set of entries, including one with a value containing
    `=` or `#` (must be preserved verbatim on round-trip — decide and test
    an explicit escaping/quoting rule if needed, don't leave it ambiguous).

7.3 **[S, depends on 7.1/7.2]** Typed preferences struct
    `mudl_config::Preferences` with pure `from_entries`/`to_entries`
    conversions and validated defaults, covering the subset of `mud`'s
    preference list that applies to a Linux core viewer (see Appendix B for
    the full mapping table). Tests: unknown keys are ignored (forward
    compatibility) rather than erroring; an out-of-range value (e.g.
    `changes_word_diff_threshold` outside `0.0..=1.0`, relevant once Phase
    13 lands) falls back to its default rather than panicking; missing keys
    fall back to documented defaults; each enum-typed preference (theme,
    lighting, folder-open-behavior, doc-c-alert-mode) rejects an unknown
    variant string by falling back to its default.

7.4 **[S, depends on 7.3]** Impure load/save:
    `mudl_config::load(fs: &dyn FileSystem, path: &Path) -> Preferences`
    (missing file → all defaults, no error) and `mudl_config::save(fs: &dyn
    FileSystem, path: &Path, prefs: &Preferences) -> io::Result<()>`
    (atomic: write to a temp file in the same directory, then rename).
    Tests with `InMemoryFileSystem`: missing file → defaults; a file with
    some keys present → those override defaults, rest stay default; save
    then load round-trips.


## 14. Phase 8 — CLI (`mudl-cli`)

8.1 **[P]** Pure flag parsing: `mudl_cli::args::parse(args: &[String]) ->
    Result<ParsedArgs, ArgError>` covering `--help`/`-h`, `--version`/`-v`,
    `--html-up`/`-u`, `--html-down`/`-d`, `--standalone`, `--fragment`/`-f`,
    `--line-numbers`, `--word-wrap`, `--readable-column`, `--theme NAME`
    (and `--theme=NAME`), positional file arguments, and "no render flag
    given → launch GUI" as a distinct `ParsedArgs::LaunchGui(files)`
    variant. Tests, one per flag combination and several error cases:
    unknown flag → `ArgError`; `--theme` with an invalid name → `ArgError`
    naming the valid set; both `-u` and `-d` given together → `ArgError`
    (mutually exclusive, matching `mud`); no files and no flags → GUI-launch
    with an empty file list; multiple files with `-u` → each rendered
    independently; `--fragment` combined with flags that are no-ops in
    fragment mode → still parses successfully (warning is a display
    concern, not a parse error).

8.2 **[S, depends on 8.1, mudl-core render functions]** Wire `main()`:
    dispatch parsed args to `mudl_core::render_up`/`render_down`
    (`--standalone` engages the Phase 5.1 data-URI image inlining; no flag
    → `mudl-gui::launch(files)`), reading files via real `std::fs`, reading
    stdin to EOF when no file arguments are given. Exit codes: `1` for
    arg-parse errors, `2` for I/O errors (file not found, unreadable, stdin
    decode failure), `0` otherwise (including `--help`/`--version`). Test
    this as a thin integration layer (a handful of `assert_cmd`-style
    process-spawn tests, or manual invocation) rather than unit tests —
    all the interesting logic already has unit coverage in 8.1 and
    `mudl-core`.


## 15. Phase 9 — Folder index & outline sidebar data

9.1 **[P]** `mudl_core::folder::walk(root: &Path, list_dir: &dyn
    Fn(&Path) -> io::Result<Vec<DirEntry>>, limit: usize) -> Tree` — pure
    given an injected directory-listing function (`DirEntry { name: String,
    is_dir: bool }`), recursively building a nested `Tree` of markdown files
    (`.md`/`.markdown`/`.mkd`), pruning empty branches, skipping hidden
    entries (dotfiles) and symlinked directories, capping total files
    visited at `limit` and setting a truncation flag if hit. Tests (with a
    scripted fake `list_dir`): flat directory of markdown files, nested
    directories, an empty directory → empty tree, a directory containing
    only non-markdown files → empty tree, hidden files/directories
    excluded, a symlinked subdirectory excluded, exactly at the `limit`
    boundary (no truncation flag) vs. one over (`truncated = true`).

9.2 **[P]** `mudl_core::folder::render_index_markdown(tree: &Tree) ->
    String` — renders a `Tree` as a nested Markdown link list, matching
    `mud`'s `FolderIndex.markdown(for:)`: Markdown-syntax-escaping for
    display names, percent-encoding for link targets. Tests: single file,
    nested structure with correct indentation, a filename containing
    Markdown special characters (`[`, `]`, `*`) is escaped in the display
    text, a path containing spaces is percent-encoded in the link target.

9.3 **[P]** `mudl_core::outline::build_tree(headings: &[OutlineHeading]) ->
    Vec<OutlineNode>` — pure nesting of a flat heading list into a tree by
    strict level comparison (any heading deeper than the current node
    becomes its child). Tests: flat single-level list, strictly increasing
    levels, a level jump (h1 → h3 directly) still nests correctly under h1,
    a level decrease partway through, empty input → empty tree, a single
    heading → a single root node with no children.


## 16. Phase 10 — GTK + WebKit2GTK shell (`mudl-gui`)

This phase is necessarily the most integration-heavy and the least
amenable to strict unit-level TDD (it's GTK signal wiring and WebKit
navigation, not algorithmic logic) — the standards' pure-function
extraction principle still applies everywhere a decision can be isolated
from the widget code; those extracted decisions get full unit coverage as
usual, and the remaining glue is verified by a manual smoke-test checklist
per step.

10.1 **[S]** Minimal window: a `gtk::ApplicationWindow` containing one
    `webkit2gtk::WebView` pointed at `mudl-server`'s `/` route for a file
    given on the command line. Smoke test: `mudl file.md` opens a window
    showing rendered HTML.

10.2 **[S, depends on 10.1]** Mode toggle (Space bar = Up/Down, matching
    `mud`): a pure function `mudl_gui::toggle::next_mode(current: Mode) ->
    Mode` (trivial two-state flip, but still extracted and tested rather
    than inlined in a GTK key-press handler) wired to a GTK key-press-event
    handler that re-navigates the WebView to `/` with a `?mode=down` query
    param, which `mudl-server` reads to choose `render_up` vs `render_down`.
    Scroll-position preservation: capture scroll fraction via a small
    injected JS snippet before navigating, restore it via an injected
    script in the `load-changed` signal handler after the new page loads
    (mirrors `mud`'s save-then-restore approach, without any native bridge
    needed for this specific case).

10.3 **[P, depends on 10.1]** Sidebar: a `gtk::Paned` split with a
    `gtk::TreeView` fed from Phase 9.1's `Tree` (folder mode) or Phase 9.3's
    `Vec<OutlineNode>` (outline mode), toggled by a preference
    (`sidebar_pane`). Row activation navigates the WebView (folder: opens
    that file; outline: scrolls to the heading's anchor via `#slug` in Up
    mode, or a `data-line` lookup + a small injected `scrollIntoView` call
    in Down mode).

10.4 **[P, depends on 10.1]** Toolbar: theme picker (dropdown of the 5
    themes, writes the choice to `mudl-config` and re-navigates with the
    new theme applied), zoom in/out (persisted per-file in `mudl-config`,
    applied via WebKit's own `WebView::set_zoom_level` — no custom CSS zoom
    logic needed, WebKit has this built in), word-wrap/line-numbers/
    readable-column toggle buttons (each maps to a body class added via a
    small injected script, matching `mud`'s `ViewToggle` → CSS-class
    pattern exactly).

10.5 **[P, depends on 10.1]** Find (Ctrl+F): a floating find bar
    (`gtk::SearchEntry` overlay) driving `webkit2gtk::WebView::find_text`/
    equivalent (`WebKit2GTK`'s built-in `WebKitFindController` covers
    "find in page" natively — likely no bundled JS find script is even
    needed here, unlike `mud`'s DOM-based approach, since WebKit2GTK
    exposes this as first-class API). Confirm this during implementation;
    fall back to a small JS-based find script only if
    `WebKitFindController`'s behavior doesn't match expectations (e.g.
    result-count display).

10.6 **[P, depends on 10.1]** Tabs: `gtk::Notebook` (or a manual tab strip
    if `Notebook`'s styling doesn't fit) for multiple open documents in one
    window, each tab backed by its own `WebView` instance pointed at a
    distinct `mudl-server` document route.

10.7 **[P, depends on 7.4]** Window geometry persistence: on close, save
    width/height/position keyed by file path into `mudl-config`; on open,
    restore if a saved entry exists, else center at a sensible default
    size.

10.8 **[S, depends on 10.1–10.6]** Live-reload wiring: inject the §2
    poll-forever client script into every served page (via
    `mudl-server`'s template assembly, not a separate WebKit-side
    mechanism), confirmed end-to-end by editing a watched file while the
    window is open and observing the reload.


## 17. Phase 11 — CLI installer & desktop integration

11.1 **[P]** `mudl_cli::installer::install(fs: &dyn FileSystem, home:
    &Path) -> io::Result<PathBuf>` — creates a symlink from
    `~/.local/bin/mudl` to the running binary's path (or copies it, if
    symlinking to an AppImage/relocatable path is awkward — decide based
    on the packaging format chosen in Phase 12). Pure decision logic
    (target path construction given `home`) is separated from the actual
    `symlink`/`io` call so it's testable with the `InMemoryFileSystem` fake.

11.2 **[P]** A `.desktop` file (`mudl.desktop`) for Ubuntu's application
    menu, plus a `.desktop` MIME association for `text/markdown` so
    `mudl` appears in "Open With" for `.md` files in the Files app —
    static config, no logic to test, just correctness-review it against
    the freedesktop.org desktop-entry spec.


## 18. Phase 12 — Packaging & documentation

12.1 **[P]** `mudl/README.md`: quick start, build instructions
    (`apt install` line for `libgtk-3-dev libwebkit2gtk-4.1-dev`, `cargo
    build --release`), CLI usage examples mirroring `mud`'s own README
    structure.

12.2 **[P]** Decide and document a distribution format (a plain `.deb` via
    `cargo-deb`, or an AppImage) — this is a packaging decision, not an
    architecture one; either sits entirely outside the crates above.

12.3 **[P]** Port `mud`'s `Doc/Examples/*.md` feature-showcase documents
    into `mudl`'s own `Doc/Examples/` as manual test fixtures / demo
    content, useful both for the golden-file tests referenced in §5.3 and
    for a real "here's what it looks like" demo.

**End of core-viewer scope.** At this point `mudl` opens, renders, and
live-reloads Markdown files with full GFM support, four themes plus dark
mode, folder/outline sidebars, find, zoom/wrap/line-number/readable-column
toggles, and a CLI rendering tool — functionally at parity with `mud`'s
core viewing experience.


## 19. Phase 13 — Change tracking (deferred, git-integrated diff overlay)

Only start this phase once Phases 0–12 are done and stable. It adds a new
`mudl-diff` crate; nothing above needs to change to support it, because the
core render functions already take a `RenderOptions` struct that can grow a
`waypoint: Option<Waypoint>` field without breaking existing callers.

13.1 **[P]** `mudl_diff::word::tokenize(text: &str) -> Vec<String>` and
    `extract_words(tokens: &[String]) -> Vec<WordPart>` — pure tokenization
    into alternating word/whitespace tokens. Tests: empty string,
    whitespace-only, single word, leading/trailing whitespace preserved as
    its own token, punctuation-adjacent words.

13.2 **[S, depends on 13.1]** `mudl_diff::word::diff(old: &[WordPart], new:
    &[WordPart]) -> Vec<WordSpan>` (an LCS-based diff — Rust's standard
    library has no built-in `CollectionDifference` equivalent; implement a
    small, pure Myers-diff or classic DP-based LCS over the token slices —
    this is squarely in "standard, well-known algorithm, hand-roll it"
    territory, not a dependency exception) plus `similarity(spans: &[WordSpan])
    -> f64` and `has_significant_changes(spans: &[WordSpan], threshold: f64)
    -> bool`. Tests ported directly from `mud`'s `WordDiffTests`: identical
    input → no changes; whitespace-only diff; hard-break preservation
    (trailing 2+ spaces vs. backslash); leading indent kept as its own
    span; a moved word doesn't duplicate trailing whitespace; similarity
    exactly at the 0.25 threshold boundary (inclusive/exclusive — pick one
    and test it explicitly, matching `mud`'s documented behavior).

13.3 **[P]** `mudl_diff::pairing::best_pairs(deleted: &[&str], inserted:
    &[&str]) -> Vec<(usize, usize)>` — greedy maximum-weight matching by
    shared-word-set intersection count (not LCS) between deleted/inserted
    line groups within a gap. Tests: 0/1/N lines on each side, a tie in
    score (deterministic tie-break rule — decide and test one), empty line
    text on either side.

13.4 **[S, depends on 13.1]** `mudl_diff::line::diff(old: &[&str], new:
    &[&str]) -> Option<Vec<LineChange>>` — line-level LCS diff, `None` when
    identical. Tests: identical arrays → `None`, all-deleted, all-inserted,
    interleaved changes.

13.5 **[S, depends on 13.2–13.4]** `mudl_diff::block::fingerprint(block:
    &LeafBlock) -> String` (whitespace-normalized prose fingerprint, with
    first-line indent and hard breaks preserved, comment-footnote
    references stripped) and `mudl_diff::block::match_blocks(old: &[LeafBlock],
    new: &[LeafBlock]) -> Vec<BlockMatch>`. Tests ported from
    `BlockMatcherTests`: re-wrapped paragraph is not a change, table
    pipe-padding differences are not a change, blockquote continuation
    prefix differences are not a change, ordered-list renumbering (5. → 4.)
    is not a change, an actually-different paragraph is a change.

13.6 **[S, depends on 13.5]** `mudl_diff::plan::ChangePlan::build(matches:
    &[BlockMatch]) -> ChangePlan` — sequential `change-N` ID minting, gap
    grouping into `group-N` badge IDs, code-block positional pairing
    excluding Mermaid/math blocks. Memoize with a small fixed-size LRU
    keyed by `(old_hash, new_hash, threshold, definition_policy)` (a
    `HashMap` + manual eviction is enough — no `lru` crate needed for an
    8-entry cache). Tests ported from `ChangePlanParityTests`/
    `ChangeIDParityTests`: ID stability across repeated calls with the same
    inputs, correct grouping of adjacent changes, code blocks paired
    positionally and excluded when they're Mermaid/math.

13.7 **[P]** `mudl_diff::git::GitRunner` trait (`fn run(&self, args: &[&str],
    cwd: &Path) -> io::Result<(i32, String)>`) with a real
    `std::process::Command`-backed implementation and a scripted fake for
    tests — mirrors `mud`'s existing DI pattern for `GitProvider`. Pure
    parsing of `git log`/`git show` output into waypoint candidates is
    extracted and tested separately from the process-spawning shell.

13.8 **[S, depends on 13.6, mudl-core render functions]** Wire `ChangePlan`
    into `render_up`/`render_down` as an optional overlay
    (`RenderOptions.waypoint`), injecting `<ins>`/`<del>` markers and change/
    group badge attributes into the existing HTML output. This is the one
    step that touches Phase 2's rendering code again — by design, it's an
    additive branch (`if let Some(waypoint) = &options.waypoint { ... }`),
    not a rewrite, confirming the earlier architecture choice to keep
    `RenderOptions` extensible paid off.

13.9 **[P]** Extend the GTK sidebar (Phase 10.3) with a "Changes" pane
    (mirrors `mud`'s `ChangesSidebarView`) listing `ChangeGroup`s, and a
    "Changes since…" popover for picking a waypoint or a `GitRunner`-sourced
    commit to diff against.


## 20. Phase 14 — Comments (deferred, footnote-based)

Also additive; also only touches Phase 2's rendering code by branching on
whether comment footnotes are present (which Phase 1.8's
`is_comment_label` already handles for the "ignore them in core viewer
mode" case).

14.1 **[P]** `mudl_comments::serialization::parse(footnote_body: &str) ->
    Comment` / `serialize(comment: &Comment) -> String` — quotation
    (leading blockquote) + message-group splitting (split at paragraphs
    matching `is_message_start`: begins with `💬` or `{`), attribution
    parsing (`parse_attribution`: split at the **last** `@` whose suffix
    parses as a timestamp, so an `@`-containing author handle isn't
    misread). Tests ported from `CommentSerializationTests`: every
    combination of author/timestamp present/absent, bare `💬` with no
    braces, quotation truncated with `…` vs. `...`, an attribution-like
    string appearing mid-paragraph (must not be split), message bodies
    round-trip byte-for-byte when unchanged.

14.2 **[P]** `mudl_comments::labels::next_label(existing: &[String]) ->
    String` — `comment-a`, ..., `comment-z`, `comment-za`, ... allocation
    scheme, never renumbering existing labels, ignoring anomalous existing
    suffixes. Tests: empty existing list → `comment-a`, `z` → `za` rollover,
    an anomalous existing label (e.g. `comment-1`) is ignored rather than
    breaking the sequence.

14.3 **[S, depends on 14.1]** `mudl_comments::anchor::locate(markdown: &str,
    quotation: &str, occurrence: usize) -> Option<usize>` (byte offset) —
    the "re-locate fresh from current content every time" approach (not
    position-tracking through edits), matching `mud`'s design exactly:
    parse current source, find a leaf block whose rendered text matches
    the (whitespace-normalized) quotation, disambiguated by occurrence
    index for duplicate text, then map that block's rendered-text position
    back to a raw byte offset. Tests ported from `CommentAnchorTests`:
    exact match, whitespace-normalized match, duplicate text disambiguated
    by occurrence, quotation no longer present → `None` (comment "orphaned"
    — decide and document the UI treatment of this case, likely: show the
    comment in a general "unanchored" list rather than dropping it).

14.4 **[P]** `mudl_comments::editor` — pure byte-surgical `insert`/
    `rewrite`/`delete` operating on raw UTF-8 byte slices, touching only
    the minimal span. Tests ported from `CommentEditorTests`: insert at
    start/middle/end of file, rewrite preserves untouched bytes elsewhere
    byte-for-byte, delete removes exactly the target footnote definition
    and its reference, trailing-newline normalization only applies when
    the edited comment is the file's last content.

14.5 **[S, depends on 14.1–14.4]** Impure write flow: re-read the file
    fresh from disk (never trust in-memory state, to tolerate concurrent
    external edits — matches `mud`'s `CommentController` design exactly),
    apply the pure editor functions, write atomically (temp file + rename,
    reusing Phase 7.4's `FileSystem::write_atomic`), surface a typed error
    (`anchor_failed` vs. `write_failed`) to the GUI layer.

14.6 **[S, depends on 14.1–14.5]** Wire footnotes and comments into
    `render_up`/`render_down` — the step the phase intro promises
    ("touches Phase 2's rendering code") but that wasn't itemized above.
    Necessary groundwork for 14.7: Phase 2.1 deliberately left
    `Options::ENABLE_FOOTNOTES` off ("deferred to Phase 14"), so plain GFM
    footnotes don't render at all yet either, not just comments. Enable it;
    render a `[^label]` reference as a superscript numbered backlink
    (authorial) or a `💬` marker (comment, via
    `mudl_core::footnotes::is_comment_label`); skip a definition's body
    where it appears inline (rendered separately below, not twice — swap
    `Renderer::out` to a scratch buffer for the duration of `Tag::
    FootnoteDefinition`'s subtree, discard it, matching the existing
    visitor's "consume the whole balanced subtree" shape rather than adding
    a new traversal mode); append a bottom Footnotes section (numbered,
    referenced-only entries) and a bottom Comments section (quotation +
    threaded messages, reusing `mudl_comments::serialization::
    format_timestamp`/`iso_timestamp` for each message's `<time>`) after
    the body. `mudl-core` depends on the new `mudl-comments` crate for
    this (`mudl_comments::document::parse_footnotes`/`parse_comments`,
    Phase 14.5's by-product) — the same dependency direction as
    `mudl-diff`, so `mudl-comments` still never depends on `mudl-core`.

14.7 **[P, depends on 14.6]** GTK comment column (mirrors `mud`'s comments
    sidebar/column): compose box, reply/edit/delete affordances, wired to
    14.5's write flow. Comment authoring UI is the one place a specific,
    isolated exception to "prefer polling over native OS hooks" might be
    worth revisiting (e.g. if typing latency during compose feels laggy
    under 300ms polling) — if so, that's the natural point to introduce the
    `inotify` swap noted in Phase 6.3, made easy precisely because
    `ChangeSource` was designed as a trait from the start.


## 21. Phase 15 — Menu bar

Source material: `docs/MENUS.md` (the menu/item/accelerator list, ported
from `mud`'s macOS menu bar). Additive to `mudl-gui`, same spirit as Phases
13–14: nothing in Phases 0–14 needs to change shape for this, since every
control the menu bar drives already exists as a function call or a widget
signal — the menu bar mostly just gives those existing code paths a second
entry point.

Three items are explicitly out of scope for this phase, shipped as visible
but disabled (`set_sensitive(false)`) menu entries rather than omitted, so
the menu's shape still matches `docs/MENUS.md`:

- **View > Show Comments** and **Edit > Add Comment** (they share the
  Ctrl-Shift-K accelerator) — both need the sidebar's pane
  (Outline/Changes/Comments — Appendix B's `sidebarPane`, today fixed
  per-tab at open time) to become switchable at runtime, which is a
  separate follow-up.
- **View > Hide Changes** — needs a new CSS class to hide the `<ins>`/
  `<del>` waypoint-diff overlay independently of clearing the waypoint
  (Phase 13.8's overlay); deferred.
- **Edit > Undo/Redo** — the only editable widget in the app is the
  Phase 14.7 comments compose `TextView`, and GTK3's `TextView` has no
  built-in undo/redo (a GTK4 feature); hand-rolling a stack is deferred.

15.1 **[P]** `mudl_gui::recent` — Open Recent's backing store, modeled on
    `mudl_gui::geometry` (Phase 10.7): its own on-disk file
    (`~/.config/mudl/recent-files`, one path per line) via
    `mudl_config::FileSystem` DI, re-read-fresh-before-write, kept out of
    `Preferences` because an open-ended ordered list doesn't fit that
    fixed-schema round-trip. `parse(text: &str) -> Vec<PathBuf>`,
    `serialize(paths: &[PathBuf]) -> String`, and the pure MRU update
    `record(existing: &[PathBuf], opened: &Path, max: usize) -> Vec<PathBuf>`
    (move-to-front dedup, capped at `max`). Tests: empty file; a path
    reopened moves to front without duplicating; the list is capped at
    `max` with the oldest entry dropped; recording into an empty list.
    Impure `load`/`save` wrappers tested with `InMemoryFileSystem`, same
    shape as `geometry::load`/`save`.

15.2 **[P]** `find::FindBar` gains `search_next`/`search_previous` methods,
    storing the `WebKitFindController` it already looks up in the struct
    instead of only closing over it locally — lets the menu's Find
    Next/Previous items and the find bar's own ▲/▼ buttons share one path.

15.3 **[P]** The toolbar's Zoom In/Out and Readable Column buttons are
    redundant with the menu's own View items (below) and are removed;
    `toolbar::build` returns `(gtk::Box, ToolbarWidgets)`
    (`ToolbarWidgets { theme_combo }`, now just the one widget the menu
    still drives) instead of just the root box. `toolbar::set_zoom(ctx:
    &Context, value: f64)` and `toolbar::step_zoom(ctx: &Context, delta:
    f64)` (delta-based, calling `set_zoom`) and
    `toolbar::set_readable_column(ctx: &Context, active: bool)` are `pub`
    functions the menu calls directly, extracted from the old zoom-button
    and readable-column-button closures.

15.4 **[S, depends on 15.2–15.3]** `window.rs`: introduce `TabHandle`
    (path, webview, `toolbar::Context`, `FindBar`, the toolbar's
    `theme_combo` widget, the sidebar's `ScrolledWindow`, and a Mark
    Up/Mark Down radio-item pair kept in sync with `mode`), returned from
    `build_tab` alongside its root widget. `OpenWindow.tab_paths:
    Vec<PathBuf>` becomes `tabs: Rc<RefCell<Vec<TabHandle>>>`;
    `focus_if_already_open` and `connect_registry_cleanup` are updated
    mechanically to read/compare through it instead. Extract
    `navigate_to_mode(webview, addr, mode, pending_scroll_fraction,
    target: Mode)` from `connect_mode_toggle`'s closure body
    (capture-scroll-then-navigate to a specific mode, not a flip); the
    Space-bar handler calls it with `next_mode(mode.get())`, the menu's
    Mark Up/Mark Down items call it with a fixed target. Remove
    `connect_find_shortcut`: Ctrl+F becomes the menu's own accelerator
    (step 15.6), so it isn't handled twice.

15.5 **[S, depends on 15.4]** `mudl_gui::menu` — `pub fn build(ctx: &Context)
    -> gtk::MenuBar`, one menu bar per window (not per tab, unlike the
    toolbar), acting on whichever tab `notebook.current_page()` currently
    selects. `Context` bundles `app`, `window`, `notebook`,
    `tabs: Rc<RefCell<Vec<TabHandle>>>`, `registry`, `prefs`, `prefs_path`,
    `recent_path`. Every item that duplicates existing toolbar behavior
    drives the existing widget or calls the existing `pub` function
    instead of reimplementing it: Readable Column calls
    `toolbar::set_readable_column`; Zoom In/Out/Actual Size call
    `toolbar::step_zoom`/`set_zoom`; the Theme submenu's radio items call
    `theme_combo.set_active_id(...)`. New logic needed for
    everything else: File > Open... (`gtk::FileChooserDialog`, reusing
    `open_files`'s existing "focus if already open, else start a tab and
    open a window" path — the same path `GApplication::connect_open`
    already uses); Open Recent (submenu rebuilt from `recent::load` on
    every parent-menu `show`, same open path, plus `recent::record`+`save`
    on every successful open, in both this menu and the existing
    `connect_open` handler); Open In Browser
    (`gtk::gio::AppInfo::launch_default_for_uri`, same call
    `connect_link_navigation`'s external-link branch already makes);
    Print... (`webkit2gtk::PrintOperation::new(&webview).run_dialog(Some(&window))`);
    Reload (`webview.reload_bypass_cache()`); Close (removes the current
    notebook page and its `TabHandle`, closing the window if that was the
    last tab); Cut/Copy/Paste/Delete/Select All (resolve
    `window.focused_widget()`; `gtk::Entry`/`gtk::TextView` use their own
    `Editable`/`TextBuffer` clipboard methods, the `WebView` only answers
    Copy/Select All via `execute_editing_command`, since page content isn't
    editable — GTK/WebKit glue, no pure core to extract, verified manually
    per the same principle Phase 10's intro states); Find (shows the
    current tab's find bar); Find Next/Previous (15.2's new methods);
    Hide Sidebar (toggles the current tab's sidebar `ScrolledWindow`
    visibility and `Preferences.sidebar_enabled` — an existing field
    nothing has read until now; `build_tab` starts a new tab's sidebar
    from it). Show Comments, Add Comment, Hide Changes, Undo, Redo are
    built `set_sensitive(false)` per the phase intro.

15.6 **[S, depends on 15.5]** Wire accelerators via a `gtk::AccelGroup`
    added to the window, one `add_accelerator("activate", ...)` per
    `docs/MENUS.md` shortcut, and pack the menu bar above the
    `gtk::Notebook` (the window's direct child becomes a `gtk::Box`
    wrapping both, not the `Notebook` alone). Not unit-tested (GTK signal
    wiring, per Phase 10's own framing); verified by the manual checklist
    in 15.7.

15.7 **[P]** Manual smoke-test checklist (`cargo run -p mudl-cli --
    somefile.md`): every File/Edit/View/Theme item behaves as designed
    above; Ctrl+F isn't double-handled; the Theme menu's checkmark stays in
    sync with the toolbar combo in both directions; Readable Column's and
    Mark Up/Mark Down's checkmarks are correct on every menu open and track
    a Space-bar toggle; the three disabled items are visibly disabled, not
    just missing.


## Appendix A — Step summary by parallelizability

Everything in Phase 1 (13 steps) can start immediately and run fully in
parallel — none of it depends on `pulldown-cmark`, the HTTP server, or the
GUI. Phase 2 is mostly serial within itself (each Markdown element/
extension builds on the visitor scaffolding) but Phase 3, 4, 6, 7, 9 can all
proceed in parallel with Phase 2 and each other once Phase 0 is done, since
none of them depend on the parser. Phase 5 depends on Phase 1's image
helpers and Phase 4's routing. Phase 8 (CLI) depends only on Phase 2's
render functions. Phase 10 (GUI) is the long pole — it depends on Phases
2–7 and 9 all being usable — but its own sub-steps (10.2–10.7) are largely
independent of each other once 10.1 exists. Phases 11–12 are cleanup/
packaging and can start as soon as the pieces they document/wrap exist.
Phases 13–14 are independent of each other and can be built in either order
or in parallel once Phase 12 is done. Phase 15 depends on Phase 10 (it
extends the same GTK shell) and, for its Theme/Readable Column items, on
Phase 10.4's toolbar controls existing to drive.


## Appendix B — Preference key mapping (`mud` → `mudl`, core-viewer scope)

| `mud` key | Type | Default | `mudl` treatment |
|---|---|---|---|
| `lighting` | enum auto/bright/dark | `auto` | Same; `auto` reads GTK's dark-mode setting. |
| `theme` | enum austere/blues/earthy/riot | `earthy` | Same 4 themes. |
| `folderOpenBehavior` | enum index/tabs | `index` | Same. |
| `upModeZoomLevel` / `downModeZoomLevel` | f64 | `1.0` | Same; applied via WebKit's native zoom API. |
| `upModeAllowRemoteContent` | bool | `true` | Same (`RenderOptions.block_remote_content`). |
| `downModeShowLineNumbers` | bool (ViewToggle) | `true` | Same, CSS body-class toggle. |
| `downModeWrapLines` | bool (ViewToggle) | `true` | Same. |
| `sidebarEnabled` / `sidebarPane` | bool / enum outline\|changes | `false` / `outline` | Same (`changes` inert until Phase 13). |
| `markdownDocCAlertMode` | enum off/common/extended | `extended` | Same. |
| `uiUseHeadingAsTitle` | bool | `true` | Same. |
| `uiShowReadableColumn` | bool (ViewToggle) | `false` | Same. |
| `uiFoldableHeadings` | bool (ViewToggle) | `true` | Same. |
| `quitOnClose` | bool | `true` | Same, mapped to GTK application lifecycle. |
| `enabledExtensions` | set of strings | all (`mermaid`, `copy-code`) | Same registry concept. |
| `openInDefaultBundleID` / `openInDefaultFormat` | macOS LaunchServices | — | Replaced by a plain configured command string (e.g. `$EDITOR`) or `xdg-open`; no app-registry lookup needed. |
| `uiFloatingControlsPosition` | enum | `bottomCenter` | Same, if a floating find bar is used (10.5) — otherwise drop if `WebKitFindController`'s own UI is used instead. |
| `changes*` / `comment*` keys | various | various | Deferred to Phases 13–14, added to the preferences schema at that point (7.3's "unknown keys ignored" rule means adding them later is backward compatible). |

Keys with no Linux equivalent, dropped: `cliInstalled`/`cliSymlinkPath`/
`cliInstalledAt` (replaced by Phase 11's simpler installer, which can just
check whether the symlink exists rather than tracking install state in
preferences), anything App-Store/sandbox-specific.


## Appendix C — CLI flag mapping (`mud` → `mudl`, core-viewer scope)

| `mud` flag | `mudl` behavior |
|---|---|
| `--help`/`-h`, `--version`/`-v` | Same. |
| `--html-up`/`-u`, `--html-down`/`-d` | Same; mutually exclusive. |
| `--browser`/`-b` | Same: render to a temp file, then `xdg-open` it (replacing macOS `open`). |
| `--standalone` | Same: inline images as data URIs via Phase 5.1. |
| `--fragment`/`-f` | Same: body-only HTML, no document wrapper. |
| `--line-numbers`, `--word-wrap`, `--readable-column` | Same: map to body classes. |
| `--theme NAME` | Same, validated against the 5 theme names. |
| `--primer` | Same: print a bundled authoring guide (port `mud`'s `Doc/Guides/primer.md` content, or a `mudl`-specific rewrite). |
| `--exclude-comments` | Deferred to Phase 14 (no-op / warns until comments exist). |
| No flags | Launch the GUI, matching `mud.sh`'s no-flag `open -a Mud.app` behavior, replaced with spawning `mudl-gui`. |
