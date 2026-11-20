//! Change-tracking parity corpus (`_working/DIFF-IMPLEMENTATION.md` §9.5).
//!
//! An edit corpus ported from `mud`'s `ChangeIDParityTests.corpus` and
//! `UpRenderingTests.diffEditCases`, plus mudl-specific cases. Every case
//! is checked for:
//!
//! - the **cross-consumer invariant**: the change IDs in the Up HTML are
//!   exactly the sidebar's (`compute_changes` under the Up policy), in
//!   order; the Down HTML's are the sidebar's under the Down policy, with
//!   insertions in the same order;
//! - well-formed Up HTML (balanced, properly nested tags);
//! - golden output under `tests/golden/changes/{up,down}-<label>.html`.
//!   Regenerate with `UPDATE_GOLDENS=1 cargo test -p mudl-core --test
//!   change_corpus`.
//!
//! It also proves that a waypoint identical to the document changes
//! nothing: the output is byte-identical to a plain render.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use mudl_core::changes::{
    compute_changes, compute_groups, TrackerMode, Waypoint, DEFAULT_WORD_DIFF_THRESHOLD,
};
use mudl_core::options::RenderOptions;
use mudl_core::render::{render_down, render_up};

struct EditCase {
    label: &'static str,
    old: &'static str,
    new: &'static str,
}

const fn case(label: &'static str, old: &'static str, new: &'static str) -> EditCase {
    EditCase { label, old, new }
}

/// `ChangeIDParityTests.corpus`.
const ID_PARITY: &[EditCase] = &[
    case(
        "paragraph edit",
        "Alpha one.\n\nBeta.\n",
        "Alpha two.\n\nBeta.\n",
    ),
    case("insertion only", "Alpha.\n", "Alpha.\n\nAdded.\n"),
    case("deletion only (trailing)", "Alpha.\n\nGone.\n", "Alpha.\n"),
    case(
        "code block edit alone",
        "```\nkeep\nold\n```\n",
        "```\nkeep\nnew\n```\n",
    ),
    case(
        "code block edit then paragraph edit",
        "```\nkeep\nold\n```\n\nMiddle.\n\nTail one.\n",
        "```\nkeep\nnew\n```\n\nMiddle.\n\nTail two.\n",
    ),
    case(
        "paragraph edit then code block edit",
        "Head one.\n\nMiddle.\n\n```\nkeep\nold\n```\n",
        "Head two.\n\nMiddle.\n\n```\nkeep\nnew\n```\n",
    ),
    case(
        "two code block edits",
        "```\na\nold1\n```\n\nMid.\n\n```\nb\nold2\n```\n",
        "```\na\nnew1\n```\n\nMid.\n\n```\nb\nnew2\n```\n",
    ),
    case(
        "code block edit and insertion in the same gap",
        "```\nkeep\nold\n```\n\nTail.\n",
        "```\nkeep\nnew\n```\n\nAdded.\n\nTail.\n",
    ),
    case(
        "multi-cluster code block edit then paragraph edit",
        "```\na\nb\nc\nd\ne\n```\n\nTail one.\n",
        "```\na\nB\nc\nD\ne\n```\n\nTail two.\n",
    ),
    case(
        "paragraph and code block swap positions in one gap",
        "Intro.\n\nAlpha beta gamma.\n\n```\nkeep\nold\n```\n\nTail.\n",
        "Intro.\n\n```\nkeep\nnew\n```\n\nDelta epsilon zeta.\n\nTail.\n",
    ),
    case(
        "paragraph deleted and code block edited in one gap",
        "Gone paragraph.\n\n```\nkeep\nold\n```\n\nTail.\n",
        "```\nkeep\nnew\n```\n\nTail.\n",
    ),
];

/// `UpRenderingTests.diffEditCases`.
const DIFF_EDITS: &[EditCase] = &[
    case(
        "word change in paragraph",
        "The quick fox.\n",
        "The slow fox.\n",
    ),
    case(
        "word change with inline code",
        "Call `foo()` and wait.\n",
        "Call `foo()` and sleep.\n",
    ),
    case(
        "word change across soft break",
        "The quick brown\nfox jumps over.\n",
        "The slow brown\nfox jumps over.\n",
    ),
    case(
        "word change with divergent formatting",
        "Hello **bold** world.\n",
        "Hello *italic* world.\n",
    ),
    case(
        "low-similarity paragraph replacement",
        "The quick brown fox jumps.\n",
        "A slow red dog leaps.\n",
    ),
    case(
        "heading edit",
        "# Title one\n\nBody.\n",
        "# Title two\n\nBody.\n",
    ),
    case(
        "deleted formatted paragraph",
        "Keep.\n\n**Bold gone.**\n",
        "Keep.\n",
    ),
    case(
        "inserted heading",
        "Paragraph.\n",
        "Paragraph.\n\n## New heading\n",
    ),
    case(
        "deleted list item",
        "- Alpha\n- Beta\n- Gamma\n",
        "- Alpha\n- Gamma\n",
    ),
    case("inserted list item", "- Alpha\n", "- Alpha\n- Beta\n"),
    case(
        "list item word change",
        "- The quick fox\n",
        "- The slow fox\n",
    ),
    case(
        "ordered list item deleted (renumbering absorbed)",
        "1. First\n2. Second\n3. Third\n",
        "1. First\n3. Third\n",
    ),
    case(
        "deleted item before complex item",
        "1. First\n2. Second\n3. Third\n   - Sub A\n   - Sub B\n",
        "1. First\n3. Third\n   - Sub A\n   - Sub B\n",
    ),
    case(
        "paragraph edited inside tight complex item",
        "- Item one\n- Item two\n  - Nested\n",
        "- Item one\n- Item two edited\n  - Nested\n",
    ),
    case(
        "loose list item inserted",
        "- Alpha\n\n- Beta\n",
        "- Alpha\n\n- Beta\n\n- Gamma\n",
    ),
    case(
        "table row inserted",
        "| A |\n|---|\n| 1 |\n",
        "| A |\n|---|\n| 1 |\n| 2 |\n",
    ),
    case(
        "table row deleted mid-body",
        "| A |\n|---|\n| 1 |\n| 2 |\n| 3 |\n",
        "| A |\n|---|\n| 1 |\n| 3 |\n",
    ),
    case(
        "last table row deleted with following block",
        "| A |\n|---|\n| 1 |\n| 2 |\n\nAfter.\n",
        "| A |\n|---|\n| 1 |\n\nAfter.\n",
    ),
    case(
        "last table row deleted trailing",
        "| A |\n|---|\n| 1 |\n| 2 |\n",
        "| A |\n|---|\n| 1 |\n",
    ),
    case(
        "paragraph deleted after table",
        "| A |\n|---|\n| 1 |\n\nGone after table.\n\nTail.\n",
        "| A |\n|---|\n| 1 |\n\nTail.\n",
    ),
    case(
        "whole table deleted",
        "Intro.\n\n| A |\n|---|\n| 1 |\n\nTail.\n",
        "Intro.\n\nTail.\n",
    ),
    case(
        "whole table inserted",
        "Intro.\n\nTail.\n",
        "Intro.\n\n| A |\n|---|\n| 1 |\n\nTail.\n",
    ),
    case(
        "paragraph deleted before table",
        "Gone.\n\n| A |\n|---|\n| 1 |\n",
        "| A |\n|---|\n| 1 |\n",
    ),
    case(
        "GFM alert inserted",
        "Keep.\n",
        "Keep.\n\n> [!NOTE]\n> Added alert.\n",
    ),
    case(
        "GFM alert deleted",
        "> [!NOTE]\n> Important info.\n\nKeep.\n",
        "Keep.\n",
    ),
    case(
        "DocC aside replaced",
        "> Status: Planning\n",
        "> Status: Underway\n",
    ),
    case(
        "DocC aside deleted",
        "> Status: Planning\n\nKeep.\n",
        "Keep.\n",
    ),
    case(
        "DocC aside inserted",
        "Keep.\n",
        "Keep.\n\n> Note: New aside.\n",
    ),
    case(
        "long DocC aside deleted",
        "> Warning: This DocC aside body is long enough that the renderer keeps it\n\
         > roman in its own paragraph instead of bolding it.\n\nKeep.\n",
        "Keep.\n",
    ),
    case(
        "blockquote paragraph edited",
        "> Quoted one.\n\nTail.\n",
        "> Quoted two.\n\nTail.\n",
    ),
    case(
        "thematic break deleted",
        "Alpha.\n\n---\n\nBeta.\n",
        "Alpha.\n\nBeta.\n",
    ),
    case(
        "thematic break inserted",
        "Alpha.\n\nBeta.\n",
        "Alpha.\n\n---\n\nBeta.\n",
    ),
    case(
        "HTML block deleted",
        "Keep.\n\n<div>\nraw\n</div>\n",
        "Keep.\n",
    ),
    case(
        "HTML block inserted",
        "Keep.\n",
        "Keep.\n\n<div>\nraw\n</div>\n",
    ),
    case(
        "mermaid diagram replaced",
        "```mermaid\ngraph TD\nA-->B\n```\n",
        "```mermaid\ngraph TD\nA-->C\n```\n",
    ),
    case("all content deleted", "Gone.\n", ""),
    case("all content inserted", "", "Brand new.\n"),
    case(
        "footnote paragraph replaced (low similarity)",
        "Alpha beta[^a].\n\n[^a]: Body.\n",
        "Entirely different words now[^a].\n\n[^a]: Body.\n",
    ),
    case(
        "paragraph referencing second footnote deleted",
        "First[^a].\n\nGone with second[^b].\n\nTail.\n\n[^a]: A.\n\n[^b]: B.\n",
        "First[^a].\n\nTail.\n\n[^a]: A.\n\n[^b]: B.\n",
    ),
    case(
        "paragraph with repeated reference deleted",
        "Twice[^a] and again[^a].\n\nGone third[^a].\n\nTail.\n\n[^a]: A.\n",
        "Twice[^a] and again[^a].\n\nTail.\n\n[^a]: A.\n",
    ),
    case(
        "footnote definition body edited",
        "Text with a footnote[^a].\n\n[^a]: Old body.\n",
        "Text with a footnote[^a].\n\n[^a]: New body.\n",
    ),
    case(
        "comment definition body edited",
        "Text with a comment[^comment-a].\n\n[^comment-a]: > a comment\n\n    \
         💬 {Tester @ 2026-07-08 12:00:00}:\n\n    Old thread body.\n",
        "Text with a comment[^comment-a].\n\n[^comment-a]: > a comment\n\n    \
         💬 {Tester @ 2026-07-08 12:00:00}:\n\n    New thread body.\n",
    ),
    case(
        "comment paragraph replaced (low similarity)",
        "Alpha beta[^comment-a].\n\n[^comment-a]: > Alpha\n\n    \
         💬 {Tester @ 2026-07-08 12:00:00}:\n\n    A note.\n",
        "Entirely different words now[^comment-a].\n\n[^comment-a]: > Alpha\n\n    \
         💬 {Tester @ 2026-07-08 12:00:00}:\n\n    A note.\n",
    ),
];

/// mudl-specific cases: pulldown-cmark shapes, coordinates, encodings.
const MUDL_EDITS: &[EditCase] = &[
    case(
        "non-ascii word change",
        "Café crème brûlée 😀 à la carte.\n",
        "Café crème brûlée 😀 au menu.\n",
    ),
    case(
        "cjk line edit",
        "漢字 かな 混じり 文\n",
        "漢字 カナ 混じり 文\n",
    ),
    case(
        "frontmatter document edited",
        "---\ntitle: Doc\ntags: [a]\n---\n# Title\n\nBody one.\n",
        "---\ntitle: Doc\ntags: [a]\n---\n# Title\n\nBody two.\n",
    ),
    case(
        "task item checked",
        "- [ ] Write tests\n- [ ] Ship\n",
        "- [x] Write tests\n- [ ] Ship\n",
    ),
    case(
        "word change with emoji shortcode",
        "Party :tada: time now.\n",
        "Party :tada: time later.\n",
    ),
    case(
        "word change around link and image",
        "See [the docs](https://example.org) and ![a cat](cat.png) here.\n",
        "See [the docs](https://example.org) and ![a cat](cat.png) there.\n",
    ),
    case(
        "word change with hard break",
        "First line  \nsecond line here.\n",
        "First line  \nsecond line there.\n",
    ),
    case(
        "inline math paragraph edited",
        "Euler $`e^{i\\pi}`$ is neat.\n",
        "Euler $`e^{i\\pi}`$ is lovely.\n",
    ),
    case(
        "display math replaced",
        "Intro.\n\n$$ x = 1 $$\n\nTail.\n",
        "Intro.\n\n$$ x = 2 $$\n\nTail.\n",
    ),
    case(
        "math fence replaced",
        "```math\nx = 1\n```\n",
        "```math\nx = 2\n```\n",
    ),
    case(
        "code block with language edited",
        "```rust\nfn main() {\n    let x = 1;\n}\n```\n",
        "```rust\nfn main() {\n    let x = 2;\n}\n```\n",
    ),
    case(
        "code block in list item edited",
        "- Item\n\n  ```\n  keep\n  old\n  ```\n",
        "- Item\n\n  ```\n  keep\n  new\n  ```\n",
    ),
    case(
        "nested list item inserted",
        "- Parent\n  - Child one\n",
        "- Parent\n  - Child one\n  - Child two\n",
    ),
    case(
        "last list item deleted before paragraph",
        "- Alpha\n- Beta\n\nAfter.\n",
        "- Alpha\n\nAfter.\n",
    ),
    case(
        "whole list deleted",
        "Intro.\n\n1. One\n2. Two\n\nTail.\n",
        "Intro.\n\nTail.\n",
    ),
    case(
        "table head edited",
        "| A | B |\n|---|---|\n| 1 | 2 |\n",
        "| A | C |\n|---|---|\n| 1 | 2 |\n",
    ),
    case(
        "table cell word change",
        "| Name | Note |\n|---|---|\n| x | the quick fox |\n",
        "| Name | Note |\n|---|---|\n| x | the slow fox |\n",
    ),
    case(
        "multi-line paragraph middle line edited",
        "Line one.\nLine two.\nLine three.\n",
        "Line one.\nLine TWO.\nLine three.\n",
    ),
    case(
        "comment added",
        "A paragraph.\n",
        "A paragraph[^comment-x].\n\n[^comment-x]: > A paragraph\n\n    \
         💬 {Tester @ 2026-07-08 12:00:00}:\n\n    Note.\n",
    ),
    case(
        "deleted paragraph with comment marker",
        "Keep.\n\nGone with comment[^comment-a].\n\n[^comment-a]: > Gone\n\n    \
         💬 {Tester @ 2026-07-08 12:00:00}:\n\n    A note.\n",
        "Keep.\n\n[^comment-a]: > Gone\n\n    \
         💬 {Tester @ 2026-07-08 12:00:00}:\n\n    A note.\n",
    ),
    case(
        "heading level changed",
        "## Section\n\nText.\n",
        "### Section\n\nText.\n",
    ),
    case(
        "loose list item word change",
        "- Alpha one\n\n- The quick fox\n",
        "- Alpha one\n\n- The slow fox\n",
    ),
    case(
        "docc aside second line edited",
        "> Note: First line\n> the quick fox.\n",
        "> Note: First line\n> the slow fox.\n",
    ),
    case(
        "list in blockquote item edited",
        "> - one two three\n> - four\n",
        "> - one TWO three\n> - four\n",
    ),
    case(
        "html escaping in changed text",
        "Use a < b & c > d here.\n",
        "Use a < b & c > e here.\n",
    ),
];

fn all_cases() -> impl Iterator<Item = &'static EditCase> {
    ID_PARITY.iter().chain(DIFF_EDITS).chain(MUDL_EDITS)
}

fn options(old: &str, show_inline_deletions: bool) -> RenderOptions {
    RenderOptions {
        waypoint: Some(Waypoint {
            old_markdown: Arc::from(old),
            show_inline_deletions,
            word_diff_threshold_pct: 25,
        }),
        ..RenderOptions::default()
    }
}

/// Distinct change IDs following `marker` in `html`, in document order.
fn id_sequence(html: &str, marker: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    let mut rest = html;
    while let Some(at) = rest.find(marker) {
        rest = &rest[at + marker.len()..];
        let Some(close) = rest.find('"') else { break };
        let id = &rest[..close];
        if id.starts_with("change-") && seen.insert(id.to_string()) {
            ids.push(id.to_string());
        }
        rest = &rest[close..];
    }
    ids
}

fn unique_in_order(ids: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    ids.into_iter()
        .filter(|id| seen.insert(id.clone()))
        .collect()
}

#[test]
fn up_ids_match_the_sidebar_in_order() {
    for c in all_cases() {
        for show in [false, true] {
            let html = render_up(c.new, &options(c.old, show));
            let up = id_sequence(&html, "data-change-id=\"");
            let sidebar = unique_in_order(
                compute_changes(
                    c.old,
                    c.new,
                    TrackerMode::Up.policy(),
                    DEFAULT_WORD_DIFF_THRESHOLD,
                )
                .into_iter()
                .map(|change| change.id),
            );
            assert_eq!(up, sidebar, "{}: Up IDs vs sidebar\n{html}", c.label);
        }
    }
}

#[test]
fn down_ids_match_the_sidebar() {
    for c in all_cases() {
        let html = render_down(c.new, &options(c.old, false));
        let changes = compute_changes(
            c.old,
            c.new,
            TrackerMode::Down.policy(),
            DEFAULT_WORD_DIFF_THRESHOLD,
        );
        let down: HashSet<String> = id_sequence(&html, "data-change-id=\"")
            .into_iter()
            .collect();
        let sidebar: HashSet<String> = changes.iter().map(|c| c.id.clone()).collect();
        assert_eq!(down, sidebar, "{}: Down ID set vs sidebar\n{html}", c.label);

        let down_ins = insertion_ids_in_down(&html);
        let sidebar_ins = unique_in_order(
            changes
                .iter()
                .filter(|c| c.ty == mudl_diff::list::ChangeType::Insertion)
                .map(|c| c.id.clone()),
        );
        assert_eq!(
            down_ins, sidebar_ins,
            "{}: Down insertion order\n{html}",
            c.label
        );
    }
}

/// Distinct change IDs of Down insertion rows, in order.
fn insertion_ids_in_down(html: &str) -> Vec<String> {
    unique_in_order(
        html.split("<div ")
            .filter(|row| row.starts_with("class=\"line dl-ins\""))
            .filter_map(|row| id_sequence(row, "data-change-id=\"").into_iter().next()),
    )
}

/// Cases whose edits the Up policy does not see.
const UP_INVISIBLE: &[&str] = &[
    "footnote definition body edited",
    "comment definition body edited",
    "comment added",
];

#[test]
fn every_case_produces_changes_except_comment_only_edits() {
    for c in all_cases() {
        let up = compute_changes(c.old, c.new, TrackerMode::Up.policy(), 0.25);
        let down = compute_changes(c.old, c.new, TrackerMode::Down.policy(), 0.25);
        // Up never sees footnote definitions; neither mode sees comments.
        let invisible_up = UP_INVISIBLE.contains(&c.label);
        let invisible_down = invisible_up && c.label != "footnote definition body edited";
        assert_eq!(up.is_empty(), invisible_up, "Up: {}", c.label);
        assert_eq!(down.is_empty(), invisible_down, "Down: {}", c.label);
    }
}

#[test]
fn up_html_is_well_formed() {
    for c in all_cases() {
        for show in [false, true] {
            let html = render_up(c.new, &options(c.old, show));
            if let Err(e) = check_nesting(&html) {
                panic!("{}: {e}\n{html}", c.label);
            }
        }
    }
}

#[test]
fn identical_waypoint_renders_byte_identically() {
    for c in all_cases() {
        for md in [c.old, c.new] {
            let plain = RenderOptions::default();
            assert_eq!(
                render_up(md, &options(md, true)),
                render_up(md, &plain),
                "{}",
                c.label
            );
            assert_eq!(
                render_down(md, &options(md, true)),
                render_down(md, &plain),
                "{}",
                c.label
            );
        }
    }
}

#[test]
fn comment_only_edits_render_byte_identically() {
    for c in all_cases().filter(|c| UP_INVISIBLE.contains(&c.label)) {
        let plain = RenderOptions::default();
        assert_eq!(
            render_up(c.new, &options(c.old, false)),
            render_up(c.new, &plain)
        );
    }
}

/// Checks that every non-void element is closed in order (valid nesting),
/// and that `<tr>`/`<li>` only appear inside their native parents.
fn check_nesting(html: &str) -> Result<(), String> {
    const VOID: [&str; 5] = ["br", "hr", "img", "input", "meta"];
    let mut stack: Vec<String> = Vec::new();
    let mut rest = html;
    while let Some(lt) = rest.find('<') {
        rest = &rest[lt + 1..];
        let gt = rest.find('>').ok_or("unterminated tag")?;
        let tag = &rest[..gt];
        rest = &rest[gt + 1..];
        if tag.starts_with('!') {
            continue;
        }
        if let Some(name) = tag.strip_prefix('/') {
            let name = name.trim();
            match stack.pop() {
                Some(open) if open == name => {}
                other => return Err(format!("</{name}> closes {other:?}")),
            }
            continue;
        }
        let name: String = tag
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        if VOID.contains(&name.as_str()) || tag.ends_with('/') {
            continue;
        }
        let parent = stack.last().map(String::as_str);
        match name.as_str() {
            "tr" if !matches!(parent, Some("tbody" | "thead" | "table")) => {
                return Err(format!("<tr> inside {parent:?}"));
            }
            "li" if !matches!(parent, Some("ul" | "ol")) => {
                return Err(format!("<li> inside {parent:?}"));
            }
            _ => {}
        }
        stack.push(name);
    }
    if stack.is_empty() {
        Ok(())
    } else {
        Err(format!("unclosed {stack:?}"))
    }
}

// MARK: - Goldens

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/changes")
}

fn slug(label: &str) -> String {
    let mut out = String::new();
    for c in label.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

#[test]
fn goldens() {
    let update = std::env::var_os("UPDATE_GOLDENS").is_some();
    let dir = golden_dir();
    if update {
        std::fs::create_dir_all(&dir).unwrap();
    }
    let mut failures = Vec::new();
    let mut names = HashSet::new();
    for c in all_cases() {
        let name = slug(c.label);
        assert!(names.insert(name.clone()), "duplicate golden name {name}");
        let opts = options(c.old, false);
        for (mode, html) in [
            ("up", render_up(c.new, &opts)),
            ("down", render_down(c.new, &opts)),
        ] {
            let path = dir.join(format!("{mode}-{name}.html"));
            if update {
                std::fs::write(&path, &html).unwrap();
                continue;
            }
            match std::fs::read_to_string(&path) {
                Ok(expected) if expected == html => {}
                Ok(_) => failures.push(format!("{} differs", path.display())),
                Err(_) => failures.push(format!("{} missing", path.display())),
            }
        }
    }
    assert!(
        failures.is_empty(),
        "golden mismatches (UPDATE_GOLDENS=1 to regenerate):\n{}",
        failures.join("\n")
    );
}

// MARK: - Real documents

/// The `docs/examples` documents (the app's own showcase corpus).
fn example_documents() -> Vec<(String, String)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/examples");
    let mut docs: Vec<(String, String)> = std::fs::read_dir(&dir)
        .expect("docs/examples")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .map(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            (name, std::fs::read_to_string(&p).unwrap())
        })
        .collect();
    docs.sort();
    assert!(!docs.is_empty());
    docs
}

#[test]
fn example_documents_render_byte_identically_against_themselves() {
    for (name, md) in example_documents() {
        let plain = RenderOptions::default();
        assert_eq!(
            render_up(&md, &options(&md, true)),
            render_up(&md, &plain),
            "{name}"
        );
        assert_eq!(
            render_down(&md, &options(&md, true)),
            render_down(&md, &plain),
            "{name}"
        );
    }
}

/// A mechanical edit of a real document: drop every 7th line and reword
/// every 4th, exercising every block shape the showcase uses.
fn mangle(md: &str) -> String {
    md.lines()
        .enumerate()
        .filter(|(i, _)| i % 7 != 6)
        .map(|(i, line)| {
            if i % 4 == 3 {
                line.replace(" the ", " a ").replace(" a ", " the ")
                    + if line.trim().is_empty() {
                        ""
                    } else {
                        " edited"
                    }
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn edited_example_documents_keep_the_invariants() {
    for (name, md) in example_documents() {
        let edited = mangle(&md);
        for (old, new) in [(&md, &edited), (&edited, &md)] {
            for show in [false, true] {
                let html = render_up(new, &options(old, show));
                if let Err(e) = check_nesting(&html) {
                    panic!("{name}: {e}");
                }
                // Same set; the order may differ where the placer defers
                // a non-row deletion keyed to a table row past `</table>`.
                let up: HashSet<String> = id_sequence(&html, "data-change-id=\"")
                    .into_iter()
                    .collect();
                let sidebar: HashSet<String> =
                    compute_changes(old, new, TrackerMode::Up.policy(), 0.25)
                        .into_iter()
                        .map(|c| c.id)
                        .collect();
                assert_eq!(up, sidebar, "{name}: Up IDs vs sidebar");
            }
            // Down: a paired block whose line diff only removes (or only
            // adds) lines draws no row for the other side's ID (as in
            // `mud`), so the set may be smaller — but every ID drawn is the
            // sidebar's, and every group has a scroll target.
            let html = render_down(new, &options(old, false));
            let down: HashSet<String> = id_sequence(&html, "data-change-id=\"")
                .into_iter()
                .collect();
            let policy = TrackerMode::Down.policy();
            let sidebar: HashSet<String> = compute_changes(old, new, policy, 0.25)
                .into_iter()
                .map(|c| c.id)
                .collect();
            assert!(
                down.is_subset(&sidebar),
                "{name}: Down IDs not in the sidebar"
            );
            for group in compute_groups(old, new, policy, 0.25) {
                assert!(
                    group.change_ids.iter().any(|id| down.contains(id)),
                    "{name}: {} has no Down target",
                    group.group_id
                );
            }
        }
    }
}
