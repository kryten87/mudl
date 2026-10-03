//! Change-tracking markup in `render_up` / `render_down`
//! (`_working/DIFF-IMPLEMENTATION.md` §9.2–9.3). Ported from `mud`'s
//! `UpModeChangeTrackingTests`, `UpModeCodeBlockDiffTests` and
//! `DownModeChangeTrackingTests`, adapted to mudl's markup: no syntax
//! highlighting server-side (so no `hljs-` or `code-header`), Down rows are
//! `<div class="line …">` with word ranges in `data-word-ins`/`-del`
//! attributes instead of inline `<ins>`/`<del>`.

use std::sync::Arc;

use mudl_core::changes::{compute_changes, TrackerMode, Waypoint};
use mudl_core::options::RenderOptions;
use mudl_core::render::{render_down, render_up};

fn options(old: &str, show_inline_deletions: bool, pct: u8) -> RenderOptions {
    RenderOptions {
        waypoint: Some(Waypoint {
            old_markdown: Arc::from(old),
            show_inline_deletions,
            word_diff_threshold_pct: pct,
        }),
        ..RenderOptions::default()
    }
}

fn up(old: &str, new: &str) -> String {
    render_up(new, &options(old, false, 25))
}

fn up_shown(old: &str, new: &str) -> String {
    render_up(new, &options(old, true, 25))
}

fn down(old: &str, new: &str) -> String {
    render_down(new, &options(old, false, 25))
}

/// The first element whose opening tag contains `class`, through the
/// first matching close tag after it (`extractBlock` in the Swift tests).
fn extract_block(html: &str, class: &str) -> Option<String> {
    let at = html.find(class)?;
    let start = html[..at].rfind('<')?;
    let name: String = html[start + 1..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    let close = format!("</{name}>");
    let end = at + html[at..].find(&close)? + close.len();
    Some(html[start..end].to_string())
}

fn extract_all_blocks(html: &str, class: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = html[from..].find(class) {
        if let Some(block) = extract_block(&html[from..], class) {
            out.push(block);
        }
        from += at + class.len();
    }
    out
}

fn count(html: &str, needle: &str) -> usize {
    html.matches(needle).count()
}

/// `data-group-id` values, in order.
fn group_ids(html: &str) -> Vec<String> {
    html.split("data-group-id=\"")
        .skip(1)
        .map(|rest| rest[..rest.find('"').unwrap()].to_string())
        .collect()
}

/// Down rows (`<div …>…</div>`).
fn rows(html: &str) -> Vec<&str> {
    html.split("</div>").filter(|r| !r.is_empty()).collect()
}

mod up_mode {
    use super::*;

    // MARK: No-op when the waypoint is absent or identical

    #[test]
    fn no_waypoint_produces_no_markers() {
        let html = render_up("Hello.\n", &RenderOptions::default());
        assert!(!html.contains("mud-change"));
        assert!(!html.contains("data-change-id"));
    }

    #[test]
    fn identical_content_produces_no_markers() {
        assert!(!up("Hello.\n", "Hello.\n").contains("mud-change"));
    }

    // MARK: Insertions on native elements

    #[test]
    fn inserted_paragraph_has_attributes_on_p() {
        let html = up("First.\n", "First.\n\nAdded.\n");
        assert!(html.contains("<p class=\"mud-change-ins\""));
        assert!(html.contains("Added."));
        assert!(html.contains("data-change-id"));
        assert!(
            !html.contains("<ins"),
            "insertions must not use <ins> wrappers"
        );
    }

    #[test]
    fn inserted_heading_has_attributes_on_h_after_its_id() {
        let html = up("Paragraph.\n", "Paragraph.\n\n## New heading\n");
        assert!(html.contains("<h2 id=\"new-heading\" class=\"mud-change-ins\" data-change-id="));
        assert!(!html.contains("<ins"));
    }

    #[test]
    fn inserted_code_block_has_attributes_on_pre() {
        let html = up("Before.\n", "Before.\n\n```\ncode\n```\n");
        assert!(html.contains("<pre class=\"mud-change-ins\" data-change-id="));
        assert!(html.contains("<code>code\n</code></pre>"));
        assert!(!html.contains("<ins"));
    }

    #[test]
    fn inserted_list_item_has_attributes_on_li() {
        let html = up("- Alpha\n", "- Alpha\n- Beta\n");
        assert!(html.contains("<li class=\"mud-change-ins\""));
        assert!(html.contains("Beta"));
        assert!(!html.contains("<ins"));
    }

    #[test]
    fn inserted_thematic_break_is_wrapped_in_a_div() {
        let html = up("Alpha.\n\nBeta.\n", "Alpha.\n\n---\n\nBeta.\n");
        assert!(html.contains("<div class=\"mud-change-ins\""));
        assert!(html.contains("><hr /></div>\n"));
    }

    #[test]
    fn inserted_html_block_is_wrapped_in_a_mud_html_block_div() {
        let html = up("Keep.\n", "Keep.\n\n<div>\nraw\n</div>\n");
        assert!(html.contains("<div class=\"mud-html-block mud-change-ins\" data-change-id="));
    }

    #[test]
    fn inserted_table_head_has_attributes_on_its_row() {
        let html = up("Intro.\n", "Intro.\n\n| A |\n|---|\n| 1 |\n");
        assert!(html.contains("<thead>\n<tr class=\"mud-change-ins\""));
    }

    #[test]
    fn inserted_display_math_is_wrapped_so_math_init_keeps_attributes() {
        let html = up("Intro.\n", "Intro.\n\n$$ x = 1 $$\n");
        assert!(html.contains(
            "<div class=\"mud-change-ins\" data-change-id=\"change-1\" data-group-id=\"group-1\" \
             data-group-type=\"ins\" data-group-index=\"1\"><pre><code class=\"language-math\">x = 1\n"
        ));
    }

    // MARK: Deletions as native elements

    #[test]
    fn deleted_paragraph_emitted_as_native_p() {
        let html = up("Keep.\n\nRemoved.\n", "Keep.\n");
        assert!(html.contains("<p class=\"mud-change-del\""));
        assert!(html.contains("Removed."));
        assert!(
            !html.contains("<del"),
            "deletions must not use <del> wrappers"
        );
    }

    #[test]
    fn deleted_unordered_list_item_emitted_as_native_li() {
        let html = up("- Alpha\n- Beta\n- Gamma\n", "- Alpha\n- Gamma\n");
        assert!(html.contains("<li class=\"mud-change-del\""));
        assert!(html.contains("Beta"));
        assert!(!html.contains("<del"));
    }

    #[test]
    fn deleted_ordered_list_item_emitted_as_native_li() {
        let html = up("1. First\n2. Second\n3. Third\n", "1. First\n3. Third\n");
        assert!(html.contains("<li class=\"mud-change-del\""));
        assert!(html.contains("Second"));
        assert!(!html.contains("<del"));
    }

    #[test]
    fn deleted_list_item_content_does_not_leak_into_survivor() {
        let html = up("1. First\n2. Second\n3. Third\n", "1. First\n3. Third\n");
        assert!(html.contains("<li>Third</li>"));
    }

    #[test]
    fn deleted_item_before_complex_item_renders_as_sibling() {
        let html = up(
            "1. First\n2. Second\n3. Third\n   - Sub A\n   - Sub B\n",
            "1. First\n3. Third\n   - Sub A\n   - Sub B\n",
        );
        let del = html.find("<li class=\"mud-change-del\"").unwrap();
        let third = html.find("<li>Third").unwrap();
        assert!(
            del < third,
            "deleted item must be a sibling before the item"
        );
        assert!(html[del..third].contains("Second</li>"));
    }

    #[test]
    fn deleted_last_list_item_outside_the_list_is_wrapped_in_a_list() {
        let html = up("- Alpha\n- Beta\n\nAfter.\n", "- Alpha\n\nAfter.\n");
        assert!(html.contains("<ul>\n<li class=\"mud-change-del\""));
    }

    #[test]
    fn deletion_appears_before_surviving_block() {
        let html = up("First.\n\nRemoved.\n\nLast.\n", "First.\n\nLast.\n");
        assert!(html.find("mud-change-del").unwrap() < html.find("Last.").unwrap());
    }

    #[test]
    fn trailing_deletion_appears_at_end() {
        let html = up("Keep.\n\nTrailing.\n", "Keep.\n");
        assert!(html.find("Keep.").unwrap() < html.find("Trailing.").unwrap());
    }

    #[test]
    fn trailing_deletion_precedes_the_footnotes_section() {
        let html = up(
            "Keep[^a].\n\nTrailing.\n\n[^a]: Note.\n",
            "Keep[^a].\n\n[^a]: Note.\n",
        );
        assert!(
            html.find("Trailing.").unwrap() < html.find("<section class=\"footnotes\"").unwrap()
        );
    }

    #[test]
    fn multiple_deletions_before_one_block() {
        let html = up("Gone A.\n\nGone B.\n\nSurvivor.\n", "Survivor.\n");
        let (a, b, s) = (
            html.find("Gone A.").unwrap(),
            html.find("Gone B.").unwrap(),
            html.find("Survivor.").unwrap(),
        );
        assert!(a < b && b < s);
    }

    #[test]
    fn deleted_thematic_break_is_a_native_hr() {
        let html = up("Alpha.\n\n---\n\nBeta.\n", "Alpha.\n\nBeta.\n");
        assert!(html.contains("<hr class=\"mud-change-del\" data-change-id=\"change-1\""));
    }

    #[test]
    fn deleted_html_block_is_a_mud_html_block_div() {
        let html = up("Keep.\n\n<div>\nraw\n</div>\n", "Keep.\n");
        assert!(html.contains("<div class=\"mud-html-block mud-change-del\""));
    }

    #[test]
    fn deleted_heading_has_no_id() {
        let html = up("# Gone\n\nKeep.\n", "Keep.\n");
        assert!(html.contains("<h1 class=\"mud-change-del\" data-change-id=\"change-1\""));
    }

    // MARK: Replacements

    #[test]
    fn replacement_old_version_precedes_new_version() {
        let html = up("Original.\n", "Changed.\n");
        assert!(html.find("Original.").unwrap() < html.find("Changed.").unwrap());
    }

    #[test]
    fn deletion_contains_rendered_html() {
        let html = up("Keep.\n\n**Bold deleted.**\n", "Keep.\n");
        assert!(html.contains("<strong>Bold deleted.</strong>"));
        assert!(!html.contains("**Bold deleted.**"));
    }

    // MARK: Change IDs and groups

    #[test]
    fn change_ids_present() {
        assert!(up("First.\n", "First.\n\nSecond.\n").contains("data-change-id=\"change-"));
    }

    #[test]
    fn multiple_changes_have_distinct_ids() {
        let html = up("Alpha.\n\nBeta.\n", "Alpha changed.\n\nBeta changed.\n");
        let ids: Vec<&str> = html
            .split("data-change-id=\"")
            .skip(1)
            .map(|r| &r[..r.find('"').unwrap()])
            .collect();
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len());
        assert!(unique.len() >= 4);
    }

    #[test]
    fn changed_elements_carry_group_id() {
        assert!(up("First.\n", "First.\n\nAdded.\n").contains("data-group-id=\"group-"));
    }

    #[test]
    fn first_element_in_group_carries_group_index() {
        let html = up("First.\n", "First.\n\nAdded A.\n\nAdded B.\n");
        assert_eq!(count(&html, "data-group-index=\"1\""), 1);
        assert!(html.contains("data-group-type=\"ins\" data-group-index=\"1\">Added A."));
        assert!(html.contains("data-group-type=\"ins\">Added B."));
    }

    #[test]
    fn group_attributes_consistent_across_multi_block_group() {
        let html = up("Keep.\n", "Keep.\n\nAdded A.\n\nAdded B.\n\nAdded C.\n");
        let ids = group_ids(&html);
        assert_eq!(ids.len(), 3);
        assert!(ids.iter().all(|id| id == "group-1"));
    }

    #[test]
    fn separate_groups_get_different_ids() {
        let html = up(
            "Alpha.\n\nKeep.\n\nGamma.\n",
            "Alpha changed.\n\nKeep.\n\nGamma changed.\n",
        );
        let mut ids = group_ids(&html);
        ids.dedup();
        assert_eq!(ids, ["group-1", "group-2"]);
    }

    #[test]
    fn deletion_elements_carry_group_id() {
        assert!(!group_ids(&up("Keep.\n\nRemoved.\n", "Keep.\n")).is_empty());
    }

    #[test]
    fn mixed_group_type_on_both_sides_of_a_replacement() {
        let html = up("Original.\n", "Changed.\n");
        assert_eq!(count(&html, "data-group-type=\"mix\""), 2);
    }

    // MARK: Table rows

    #[test]
    fn inserted_table_row_has_attributes_on_tr() {
        let html = up("| A |\n|---|\n| 1 |\n", "| A |\n|---|\n| 1 |\n| 2 |\n");
        assert!(html.contains("<tr class=\"mud-change-ins\""));
        assert!(!html.contains("<ins"));
    }

    #[test]
    fn deleted_table_row_emitted_as_native_tr() {
        let html = up("| A |\n|---|\n| 1 |\n| 2 |\n", "| A |\n|---|\n| 1 |\n");
        assert!(html.contains("<tr class=\"mud-change-del\""));
        assert!(!html.contains("<del"));
    }

    #[test]
    fn deleted_table_row_appears_as_sibling_in_tbody() {
        let html = up(
            "| A |\n|---|\n| 1 |\n| 2 |\n| 3 |\n",
            "| A |\n|---|\n| 1 |\n| 3 |\n",
        );
        let (open, close, del) = (
            html.find("<tbody>").unwrap(),
            html.find("</tbody>").unwrap(),
            html.find("mud-change-del").unwrap(),
        );
        assert!(open < del && del < close);
    }

    fn inside_open_tbody(html: &str) {
        assert!(html.contains("<tr class=\"mud-change-del\""));
        assert_eq!(
            count(html, "<table>"),
            1,
            "deleted row must be in the original table"
        );
        let before = &html[..html.find("mud-change-del").unwrap()];
        assert!(count(before, "<tbody>") > count(before, "</tbody>"));
    }

    #[test]
    fn deleted_last_table_row_inside_original_table() {
        inside_open_tbody(&up(
            "| A |\n|---|\n| 1 |\n| 2 |\n\nAfter.\n",
            "| A |\n|---|\n| 1 |\n\nAfter.\n",
        ));
    }

    #[test]
    fn deleted_last_table_row_as_trailing_deletion() {
        inside_open_tbody(&up("| A |\n|---|\n| 1 |\n| 2 |\n", "| A |\n|---|\n| 1 |\n"));
    }

    #[test]
    fn whole_deleted_table_rows_are_wrapped_in_their_own_table() {
        let html = up(
            "Intro.\n\n| A |\n|---|\n| 1 |\n\nTail.\n",
            "Intro.\n\nTail.\n",
        );
        assert!(html.contains("<table>\n<tbody>\n<tr class=\"mud-change-del\""));
        assert!(html.contains("<th>A</th>"));
    }

    #[test]
    fn deletion_keyed_to_a_table_head_is_hoisted_above_the_table() {
        let html = up("Gone.\n\n| A |\n|---|\n| 1 |\n", "| A |\n|---|\n| 1 |\n");
        assert!(html.starts_with("<p class=\"mud-change-del\""));
    }

    #[test]
    fn non_row_deletion_keyed_to_a_body_row_is_deferred_after_the_table() {
        // Old: a paragraph between two tables; new: one table with the
        // second table's rows. The paragraph's deletion is keyed to a row.
        let html = up(
            "| A |\n|---|\n| 1 |\n\nGone.\n\nKeep.\n",
            "| A |\n|---|\n| 1 |\n| 2 |\n\nKeep.\n",
        );
        let table_end = html.find("</table>").unwrap();
        let gone = html.find("Gone.").unwrap();
        assert!(gone > table_end, "{html}");
    }

    // MARK: Edge cases

    #[test]
    fn all_content_deleted() {
        let html = up("Gone.\n", "");
        assert!(html.contains("<p class=\"mud-change-del\""));
        assert!(html.contains("Gone."));
        assert!(!html.contains("<del"));
    }

    #[test]
    fn all_content_inserted() {
        let html = up("", "Brand new.\n");
        assert!(html.contains("<p class=\"mud-change-ins\""));
        assert!(!html.contains("<ins"));
    }

    // MARK: Word-level diffs in the insertion

    #[test]
    fn single_word_changed_shows_inline_markers() {
        let html = up("The quick fox.\n", "The slow fox.\n");
        assert!(html.contains("<del>quick</del>") && html.contains("<ins>slow</ins>"));
    }

    #[test]
    fn word_added_shows_ins_only() {
        let block = extract_block(&up("The fox.\n", "The brown fox.\n"), "mud-change-ins").unwrap();
        assert!(block.contains("<ins>") && block.contains("brown"));
    }

    #[test]
    fn word_removed_shows_del_only() {
        let html = up_shown("The brown fox.\n", "The fox.\n");
        let block = extract_block(&html, "mud-change-ins").unwrap();
        assert!(block.contains("<del>") && block.contains("brown"));
    }

    #[test]
    fn formatted_text_same_structure_shows_inline_markers() {
        let html = up(
            "Hello **beautiful** world.\n",
            "Hello **wonderful** world.\n",
        );
        let block = extract_block(&html, "mud-change-ins").unwrap();
        assert!(
            block.contains("<strong><ins>wonderful</ins></strong>"),
            "{block}"
        );
    }

    #[test]
    fn formatting_only_change_falls_back_to_block_level() {
        let html = up("Hello **bold** world.\n", "Hello *bold* world.\n");
        let block = extract_block(&html, "mud-change-ins").unwrap();
        assert!(!block.contains("<ins>") && !block.contains("<del>"));
    }

    #[test]
    fn word_changed_with_divergent_formatting_shows_inline_markers() {
        let html = up_shown("Hello **bold** world.\n", "Hello *italic* world.\n");
        let block = extract_block(&html, "mud-change-ins").unwrap();
        assert!(block.contains("<del>") && block.contains("<ins>"));
        assert!(block.contains("bold") && block.contains("italic"));
    }

    #[test]
    fn red_block_with_divergent_formatting_shows_del_only() {
        let html = up("Hello **bold** world.\n", "Hello *italic* world.\n");
        let block = extract_block(&html, "mud-change-del").unwrap();
        assert!(block.contains("<del>") && block.contains("bold"));
        assert!(!block.contains("<ins>") && !block.contains("italic"));
    }

    #[test]
    fn blue_block_with_formatting_added_shows_markers_in_new_formatting() {
        let html = up_shown("The quick fox.\n", "The *slow* fox.\n");
        let block = extract_block(&html, "mud-change-ins").unwrap();
        assert!(block.contains("<em>") && block.contains("<ins>") && block.contains("<del>"));
        assert!(block.contains("slow") && block.contains("quick"));
    }

    #[test]
    fn red_block_with_formatting_added_preserves_old_formatting() {
        let html = up("The quick fox.\n", "The *slow* fox.\n");
        let block = extract_block(&html, "mud-change-del").unwrap();
        assert!(!block.contains("<em>"));
        assert!(block.contains("<del>quick</del>"));
        assert!(!block.contains("<ins>"));
    }

    #[test]
    fn multi_line_paragraph_word_change_preserves_full_text() {
        let html = up(
            "The quick brown\nfox jumps over.\n",
            "The slow brown\nfox jumps over.\n",
        );
        let block = extract_block(&html, "mud-change-ins").unwrap();
        assert!(
            block.contains("<ins>slow</ins> brown\nfox jumps over.</p>"),
            "{block}"
        );
    }

    #[test]
    fn hard_break_consumes_one_character() {
        let html = up(
            "First line  \nsecond line here.\n",
            "First line  \nsecond line there.\n",
        );
        let block = extract_block(&html, "mud-change-ins").unwrap();
        assert!(
            block.contains("First line<br />\nsecond line <ins>there.</ins></p>"),
            "{block}"
        );
    }

    #[test]
    fn image_alt_text_is_consumed_silently() {
        let html = up(
            "See ![a cat](cat.png) here.\n",
            "See ![a cat](cat.png) there.\n",
        );
        let block = extract_block(&html, "mud-change-ins").unwrap();
        assert!(
            block.contains("<img src=\"cat.png\" alt=\"a cat\" /> <ins>there.</ins>"),
            "{block}"
        );
    }

    #[test]
    fn emoji_shortcodes_are_replaced_inside_spans() {
        let html = up("Party :tada: time now.\n", "Party :tada: time later.\n");
        let block = extract_block(&html, "mud-change-ins").unwrap();
        assert!(
            block.contains("Party \u{1F389} time <ins>later.</ins>"),
            "{block}"
        );
    }

    #[test]
    fn html_is_escaped_inside_spans() {
        let html = up("Use a < b & c here.\n", "Use a < b & d here.\n");
        let block = extract_block(&html, "mud-change-ins").unwrap();
        assert!(
            block.contains("Use a &lt; b &amp; <ins>d</ins> here."),
            "{block}"
        );
    }

    #[test]
    fn non_ascii_word_change_is_aligned() {
        let html = up("Café crème 😀 brûlée.\n", "Café crème 😀 glacée.\n");
        let block = extract_block(&html, "mud-change-ins").unwrap();
        assert!(
            block.contains("Café crème 😀 <ins>glacée.</ins>"),
            "{block}"
        );
    }

    #[test]
    fn footnote_reference_consumes_nothing() {
        let html = up(
            "Alpha[^a] beta gamma delta.\n\n[^a]: Note.\n",
            "Alpha[^a] beta gamma epsilon.\n\n[^a]: Note.\n",
        );
        let block = extract_block(&html, "mud-change-ins").unwrap();
        assert!(
            block.contains("<a href=\"#fn-a\">1</a></sup> beta gamma <ins>epsilon.</ins>"),
            "{block}"
        );
    }

    #[test]
    fn inline_math_blocks_take_no_word_spans() {
        let html = up("Euler $`e`$ is neat.\n", "Euler $`e`$ is lovely.\n");
        assert!(!html.contains("<ins>") && !html.contains("<del>"));
        assert!(html.contains("language-math-inline"));
    }

    // MARK: Truncation guards (the cursor must consume exactly the stream)

    fn survives(old: &str, new: &str, needles: &[&str]) {
        let html = up(old, new);
        let block = extract_block(&html, "mud-change-ins").unwrap();
        for needle in needles {
            assert!(block.contains(needle), "{needle} missing from {block}");
        }
    }

    #[test]
    fn inline_code_does_not_cause_truncation() {
        survives(
            "Call `foo()` and wait.\n",
            "Call `foo()` and sleep.\n",
            &["<code>foo()</code>", "sleep"],
        );
    }

    #[test]
    fn multiple_inline_codes_do_not_cause_truncation() {
        survives(
            "Use `foo()` then `bar()` to finish.\n",
            "Use `foo()` then `bar()` to start.\n",
            &["foo()", "bar()", "start"],
        );
    }

    #[test]
    fn bold_text_does_not_cause_truncation() {
        survives(
            "The **important** value is high.\n",
            "The **important** value is low.\n",
            &["important", "low"],
        );
    }

    #[test]
    fn emphasis_does_not_cause_truncation() {
        survives(
            "The *special* value is high.\n",
            "The *special* value is low.\n",
            &["special", "low"],
        );
    }

    #[test]
    fn mixed_formatting_does_not_cause_truncation() {
        survives(
            "Call `render()` on the **main** thread now.\n",
            "Call `render()` on the **main** thread later.\n",
            &["render()", "main", "later"],
        );
    }

    #[test]
    fn inline_code_at_start_does_not_cause_truncation() {
        survives(
            "`foo` is a function.\n",
            "`foo` is a method.\n",
            &["<code>foo</code>", "method"],
        );
    }

    #[test]
    fn inline_code_at_end_does_not_cause_truncation() {
        survives(
            "The function is `foo`.\n",
            "The method is `foo`.\n",
            &["method", "<code>foo</code>."],
        );
    }

    // MARK: Word-level diffs in the deletion

    #[test]
    fn red_block_shows_deleted_words_only() {
        let block =
            extract_block(&up("The quick fox.\n", "The slow fox.\n"), "mud-change-del").unwrap();
        assert!(block.contains("<del>quick</del>"));
        assert!(!block.contains("<ins>"));
    }

    #[test]
    fn red_block_word_added_in_new_has_no_markers() {
        let block = extract_block(&up("The fox.\n", "The brown fox.\n"), "mud-change-del").unwrap();
        assert!(!block.contains("brown"));
    }

    #[test]
    fn green_block_has_no_word_level_markers() {
        let html = up("Keep.\n", "Keep.\n\nBrand new paragraph.\n");
        for block in extract_all_blocks(&html, "mud-change-ins") {
            assert!(!block.contains("<ins>") && !block.contains("<del>"));
        }
    }

    // MARK: Word-level diffs in list items

    #[test]
    fn unordered_list_item_word_change_shows_inline_markers() {
        let html = up_shown("- The quick fox\n", "- The slow fox\n");
        let block = extract_block(&html, "mud-change-ins").unwrap();
        assert!(block.contains("<del>quick</del><ins>slow</ins>"), "{block}");
    }

    #[test]
    fn ordered_list_item_word_change_shows_inline_markers() {
        let html = up_shown("1. The quick fox\n", "1. The slow fox\n");
        let block = extract_block(&html, "mud-change-ins").unwrap();
        assert!(block.contains("<del>quick</del><ins>slow</ins>"), "{block}");
    }

    #[test]
    fn list_item_word_append_shows_inline_markers() {
        let block = extract_block(
            &up("- Alpha done\n", "- Alpha done and more\n"),
            "mud-change-ins",
        )
        .unwrap();
        assert!(block.contains("<ins>") && block.contains("more"));
    }

    #[test]
    fn loose_list_item_word_change_marks_inside_its_paragraph() {
        let html = up(
            "- Alpha one\n\n- The quick fox\n",
            "- Alpha one\n\n- The slow fox\n",
        );
        let block = extract_block(&html, "mud-change-ins").unwrap();
        assert!(block.contains("<p>The <ins>slow</ins> fox</p>"), "{block}");
    }

    #[test]
    fn changed_paragraph_of_a_tight_complex_item_gets_a_span_wrapper() {
        let html = up(
            "- Item one\n- Item two\n  - Nested\n",
            "- Item one\n- Item two edited\n  - Nested\n",
        );
        assert!(html.contains("<li><p class=\"mud-change-del\""), "{html}");
        assert!(html.contains("<span class=\"mud-change-ins\""), "{html}");
        assert!(
            html.contains("Item two<ins> edited</ins></span><ul>"),
            "{html}"
        );
    }

    #[test]
    fn task_item_toggle_is_a_change() {
        let html = up("- [ ] Write tests\n", "- [x] Write tests\n");
        assert!(html.contains("<li class=\"mud-change-ins\""));
        assert!(html.contains("<li class=\"mud-change-del\""));
    }

    // MARK: Inline deletions setting

    #[test]
    fn inline_deletions_off_hides_del_in_blue_block() {
        let block =
            extract_block(&up("The quick fox.\n", "The slow fox.\n"), "mud-change-ins").unwrap();
        assert!(!block.contains("<del>") && !block.contains("quick"));
        assert!(block.contains("<ins>slow</ins>"));
    }

    #[test]
    fn inline_deletions_on_shows_del_in_blue_block() {
        let html = up_shown("The quick fox.\n", "The slow fox.\n");
        let block = extract_block(&html, "mud-change-ins").unwrap();
        assert!(block.contains("<del>quick</del>"));
    }

    // MARK: Word diff threshold

    #[test]
    fn low_similarity_block_suppresses_word_markers() {
        let html = up("The quick brown fox jumps.\n", "A slow red dog leaps.\n");
        let block = extract_block(&html, "mud-change-ins").unwrap();
        assert!(!block.contains("<ins>") && !block.contains("<del>"));
    }

    #[test]
    fn high_similarity_block_shows_word_markers() {
        let block =
            extract_block(&up("The quick fox.\n", "The slow fox.\n"), "mud-change-ins").unwrap();
        assert!(block.contains("<ins>"));
    }

    #[test]
    fn custom_threshold_overrides_default() {
        let html = render_up("The slow fox.\n", &options("The quick fox.\n", false, 80));
        let block = extract_block(&html, "mud-change-ins").unwrap();
        assert!(!block.contains("<ins>"));
    }

    // MARK: Alerts and asides

    #[test]
    fn inserted_gfm_alert_has_change_attributes_on_blockquote() {
        let html = up("Keep.\n", "Keep.\n\n> [!NOTE]\n> Added alert.\n");
        assert!(
            html.contains("<blockquote class=\"alert alert-note mud-change-ins\" data-change-id=")
        );
        assert!(html.contains("Added alert."));
        assert!(!html.contains("<ins"));
    }

    #[test]
    fn replaced_docc_aside_has_change_attributes() {
        let html = up("> Status: Planning\n", "> Status: Underway\n");
        assert!(html.contains("mud-change-ins") && html.contains("data-change-id"));
    }

    #[test]
    fn inserted_docc_aside_has_change_attributes() {
        let html = up("Keep.\n", "Keep.\n\n> Note: New aside.\n");
        assert!(html.contains("<blockquote class=\"alert alert-note mud-change-ins\""));
        assert!(html.contains("New aside."));
    }

    #[test]
    fn deleted_aside_rendered_as_alert() {
        let html = up("> Status: Planning\n\nKeep.\n", "Keep.\n");
        assert!(html.contains("<blockquote class=\"alert alert-status mud-change-del\""));
        assert!(html.contains("alert-title"));
    }

    #[test]
    fn replaced_aside_deleted_version_rendered_as_alert() {
        let html = up("> Status: Planning\n", "> Status: Underway\n");
        assert!(html.contains("<blockquote class=\"alert alert-status mud-change-del\""));
        assert!(html.contains("Planning"));
    }

    #[test]
    fn replaced_aside_shows_word_level_diff_in_inserted_block() {
        let html = up("> Status: Planning\n", "> Status: Underway\n");
        let block = extract_block(&html, "mud-change-ins").unwrap();
        assert!(block.contains("<p><ins>Underway</ins></p>"), "{block}");
    }

    #[test]
    fn replaced_aside_shows_word_level_diff_in_deleted_block() {
        let html = up("> Status: Planning\n", "> Status: Underway\n");
        let block = extract_block(&html, "mud-change-del").unwrap();
        assert!(block.contains("<del>Planning</del>"));
        assert!(!block.contains("<ins>"));
    }

    #[test]
    fn deleted_gfm_alert_rendered_as_alert() {
        let html = up("> [!NOTE]\n> Important info.\n\nKeep.\n", "Keep.\n");
        assert!(html.contains("<blockquote class=\"alert alert-note mud-change-del\""));
        assert!(html.contains("alert-title"));
    }

    // MARK: Footnotes and comments

    #[test]
    fn deleted_block_uses_the_old_documents_footnote_numbers() {
        let html = up(
            "First[^a].\n\nGone with second[^b].\n\nTail.\n\n[^a]: A.\n\n[^b]: B.\n",
            "First[^a].\n\nTail.\n\n[^a]: A.\n\n[^b]: B.\n",
        );
        let block = extract_block(&html, "mud-change-del").unwrap();
        assert!(block.contains("<a href=\"#fn-b\">2</a>"), "{block}");
    }

    #[test]
    fn deleted_copy_emits_no_comment_marker() {
        let comment =
            "[^comment-a]: > Alpha\n\n    💬 {Tester @ 2026-07-08 12:00:00}:\n\n    A note.\n";
        let old = format!("Alpha beta[^comment-a].\n\n{comment}");
        let new = format!("Entirely different words now[^comment-a].\n\n{comment}");
        let html = up(&old, &new);
        assert_eq!(count(&html, "mud-comment-marker"), 1, "{html}");
        let block = extract_block(&html, "mud-change-del").unwrap();
        assert!(!block.contains("mud-comment-marker"));
    }

    #[test]
    fn footnote_and_comment_sections_carry_no_change_markup() {
        let html = up(
            "Text[^a].\n\n[^a]: Old body.\n",
            "Text[^a] more.\n\n[^a]: New body.\n",
        );
        let section = &html[html.find("<section class=\"footnotes\"").unwrap()..];
        assert!(!section.contains("mud-change"), "{section}");
    }

    #[test]
    fn definition_only_edit_renders_no_up_markup() {
        let html = up("Text[^a].\n\n[^a]: Old.\n", "Text[^a].\n\n[^a]: New.\n");
        assert!(!html.contains("mud-change"));
    }

    // MARK: Frontmatter

    #[test]
    fn frontmatter_is_never_diffed() {
        let html = up("---\ntitle: A\n---\nBody.\n", "---\ntitle: B\n---\nBody.\n");
        assert!(!html.contains("mud-change"));
    }

    // MARK: Sidebar parity

    #[test]
    fn up_ids_match_compute_changes() {
        let (old, new) = ("Keep.\n\nOriginal.\n", "Keep.\n\nRevised.\n\nAdded.\n");
        let html = up(old, new);
        for change in compute_changes(old, new, TrackerMode::Up.policy(), 0.25) {
            assert!(html.contains(&format!("data-change-id=\"{}\"", change.id)));
        }
    }
}

mod up_code_blocks {
    use super::*;

    #[test]
    fn changed_code_block_has_mud_code_diff_class() {
        assert!(up("```\nold line\n```\n", "```\nnew line\n```\n")
            .contains("<pre class=\"mud-code-diff\">"));
    }

    #[test]
    fn diffed_code_block_pre_has_no_block_level_change_id() {
        let html = up("```\nold\n```\n", "```\nnew\n```\n");
        let pre = &html[html.find("<pre").unwrap()..];
        let pre = &pre[..pre.find('>').unwrap()];
        assert!(!pre.contains("data-change-id"));
    }

    #[test]
    fn unchanged_lines_wrapped_in_cl_span() {
        assert!(up("```\nkeep\nold\n```\n", "```\nkeep\nnew\n```\n")
            .contains("<span class=\"cl\">keep\n</span>"));
    }

    #[test]
    fn inserted_line_has_cl_ins_class() {
        assert!(
            up("```\nkeep\n```\n", "```\nkeep\nadded\n```\n").contains("<span class=\"cl cl-ins\"")
        );
    }

    #[test]
    fn deleted_line_has_cl_del_class() {
        assert!(up("```\nkeep\nremoved\n```\n", "```\nkeep\n```\n")
            .contains("<span class=\"cl cl-del\""));
    }

    #[test]
    fn changed_lines_carry_data_attributes() {
        let html = up("```\nold\n```\n", "```\nnew\n```\n");
        let tags: Vec<&str> = html
            .split("<span class=\"cl cl-")
            .skip(1)
            .map(|r| &r[..r.find('>').unwrap()])
            .collect();
        assert_eq!(tags.len(), 2);
        for tag in tags {
            assert!(tag.contains("data-change-id") && tag.contains("data-group-id"));
            assert!(tag.contains("data-group-type=\"mix\""));
        }
    }

    #[test]
    fn first_line_in_group_carries_group_index() {
        let html = up("```\nold\n```\n", "```\nnew\n```\n");
        assert_eq!(count(&html, "data-group-index=\"1\""), 1);
    }

    #[test]
    fn language_class_kept_on_the_code_element() {
        let html = up("```swift\nlet x = 1\n```\n", "```swift\nlet y = 2\n```\n");
        assert!(html.contains("<pre class=\"mud-code-diff\"><code class=\"language-swift\">"));
    }

    #[test]
    fn unchanged_code_block_has_no_cl_spans() {
        let html = up("```\ncode\n```\n", "```\ncode\n```\n");
        assert!(!html.contains("mud-code-diff") && !html.contains("class=\"cl\""));
    }

    #[test]
    fn new_code_block_uses_block_level_not_line_diff() {
        let html = up("Before.\n", "Before.\n\n```\nnew code\n```\n");
        assert!(html.contains("mud-change-ins") && !html.contains("mud-code-diff"));
    }

    #[test]
    fn newline_inside_line_span() {
        assert!(up("```\nold\n```\n", "```\nnew\n```\n").contains("\n</span>"));
    }

    #[test]
    fn changed_mermaid_block_uses_block_level_not_line_diff() {
        let html = up(
            "```mermaid\ngraph LR\n  A-->B\n```\n",
            "```mermaid\ngraph LR\n  A-->C\n```\n",
        );
        assert!(!html.contains("mud-code-diff"));
        assert!(html.contains("<div class=\"mud-change-ins\""));
        assert!(html.contains("<pre><code class=\"language-mermaid\">"));
    }

    #[test]
    fn changed_mermaid_block_old_version_shows_placeholder() {
        let html = up(
            "```mermaid\ngraph LR\n  A-->B\n```\n",
            "```mermaid\ngraph LR\n  A-->C\n```\n",
        );
        assert!(html.contains("<pre class=\"mud-change-del\""));
        assert!(html.contains("<code><em>[revised diagram]</em></code>"));
        assert!(!html.contains("A--&gt;B"));
        assert!(html.contains("A--&gt;C"));
    }

    #[test]
    fn changed_math_fence_is_never_line_diffed() {
        let html = up("```math\nx = 1\n```\n", "```math\nx = 2\n```\n");
        assert!(!html.contains("mud-code-diff"));
        assert!(html.contains("<div class=\"mud-change-del\""));
        assert!(html.contains("<div class=\"mud-change-ins\""));
    }

    #[test]
    fn multiple_groups_in_one_code_block() {
        let html = up("```\na\nb\nc\nd\ne\n```\n", "```\na\nB\nc\nD\ne\n```\n");
        let mut ids = group_ids(&html);
        ids.dedup();
        assert!(ids.len() >= 2);
    }

    #[test]
    fn modified_line_shows_word_level_markers() {
        let html = up("```\nlet x = 1\n```\n", "```\nlet x = 2\n```\n");
        assert!(html.contains("let x = <del>1</del>\n</span>"), "{html}");
        assert!(html.contains("let x = <ins>2</ins>\n</span>"), "{html}");
    }

    #[test]
    fn added_line_has_no_word_markers() {
        let html = up("```\na\n```\n", "```\na\nb\n```\n");
        assert!(!html.contains("<ins>") && !html.contains("<del>"));
    }

    #[test]
    fn deletion_lines_before_insertion_lines_in_gap() {
        let html = up("```\na\nold\nc\n```\n", "```\na\nnew\nc\n```\n");
        assert!(html.find("cl-del").unwrap() < html.find("cl-ins").unwrap());
    }

    #[test]
    fn code_line_text_is_escaped() {
        let html = up("```\nif a < b { x }\n```\n", "```\nif a < b { y }\n```\n");
        assert!(html.contains("if a &lt; b { <ins>y</ins> }"), "{html}");
    }

    #[test]
    fn no_waypoint_code_block_renders_normally() {
        let html = render_up("```swift\nlet x = 1\n```\n", &RenderOptions::default());
        assert_eq!(
            html,
            "<pre><code class=\"language-swift\">let x = 1\n</code></pre>\n"
        );
    }

    #[test]
    fn code_pair_ids_match_compute_changes() {
        let (old, new) = ("```\nkeep\nold\n```\n", "```\nkeep\nnew\n```\n");
        let html = up(old, new);
        for change in compute_changes(old, new, TrackerMode::Up.policy(), 0.25) {
            assert!(html.contains(&format!("data-change-id=\"{}\"", change.id)));
        }
    }
}

mod down_mode {
    use super::*;

    #[test]
    fn no_waypoint_produces_no_markers() {
        let html = render_down("Hello.\n", &RenderOptions::default());
        assert!(
            !html.contains("dl-ins")
                && !html.contains("dl-del")
                && !html.contains("data-change-id")
        );
    }

    #[test]
    fn identical_content_produces_no_markers() {
        assert_eq!(
            down("Hello.\n", "Hello.\n"),
            render_down("Hello.\n", &RenderOptions::default())
        );
    }

    #[test]
    fn inserted_paragraph_line_marked_as_insertion() {
        let html = down("First.\n", "First.\n\nAdded.\n");
        assert!(html.contains(
            "<div class=\"line dl-ins\" data-line=\"3\" data-change-id=\"change-1\">Added.</div>"
        ));
    }

    #[test]
    fn multi_line_insertion_marks_all_lines() {
        let html = down("Keep.\n", "Keep.\n\nLine one.\nLine two.\n");
        assert_eq!(count(&html, "dl-ins"), 2);
    }

    #[test]
    fn deleted_paragraph_line_marked_as_deletion() {
        let html = down("Keep.\n\nRemoved.\n", "Keep.\n");
        assert!(
            html.contains("<div class=\"line dl-del\" data-change-id=\"change-1\">Removed.</div>")
        );
    }

    #[test]
    fn deleted_lines_have_no_line_number() {
        let html = down("Keep.\n\nRemoved.\n", "Keep.\n");
        let row = rows(&html)
            .into_iter()
            .find(|r| r.contains("dl-del"))
            .unwrap();
        assert!(!row.contains("data-line"));
    }

    #[test]
    fn deletion_appears_at_correct_position() {
        let html = down("First.\n\nMiddle.\n\nLast.\n", "First.\n\nLast.\n");
        let (f, m, l) = (
            html.find("First.").unwrap(),
            html.find("Middle.").unwrap(),
            html.find("Last.").unwrap(),
        );
        assert!(f < m && m < l);
    }

    #[test]
    fn trailing_deletion_appears_at_end() {
        let html = down("Keep.\n\nTrailing.\n", "Keep.\n");
        assert!(html
            .ends_with("<div class=\"line dl-del\" data-change-id=\"change-1\">Trailing.</div>"));
    }

    #[test]
    fn replaced_block_shows_old_and_new_lines() {
        let html = down("Original text.\n", "Revised text.\n");
        assert!(html.contains("dl-del") && html.contains("Original"));
        assert!(html.contains("dl-ins") && html.contains("Revised"));
    }

    #[test]
    fn replacement_old_lines_precede_new_lines() {
        let html = down("Original.\n", "Changed.\n");
        assert!(html.find("Original.").unwrap() < html.find("Changed.").unwrap());
    }

    #[test]
    fn surviving_lines_keep_new_document_line_numbers() {
        let html = down("First.\n\nRemoved.\n\nThird.\n", "First.\n\nThird.\n");
        assert!(html.contains("data-line=\"1\">First."));
        assert!(html.contains("data-line=\"3\">Third."));
    }

    #[test]
    fn change_ids_on_changed_lines() {
        assert!(down("First.\n", "First.\n\nSecond.\n").contains("data-change-id=\"change-"));
    }

    #[test]
    fn deleted_lines_are_escaped_raw_source() {
        let html = down("# Heading <b>\n\nKeep.\n", "Keep.\n");
        assert!(html.contains(
            "<div class=\"line dl-del\" data-change-id=\"change-1\"># Heading &lt;b&gt;</div>"
        ));
    }

    // MARK: Word-level ranges

    #[test]
    fn word_changed_in_paired_block_carries_ranges() {
        let html = down("The quick fox.\n", "The slow fox.\n");
        assert!(
            html.contains("data-word-del=\"4-9\">The quick fox.</div>"),
            "{html}"
        );
        assert!(
            html.contains("data-word-ins=\"4-8\">The slow fox.</div>"),
            "{html}"
        );
        assert!(!html.contains("<ins>") && !html.contains("<del>"));
    }

    #[test]
    fn deletion_line_in_paired_block_has_no_ins_ranges() {
        let html = down("The quick fox.\n", "The slow fox.\n");
        for row in rows(&html).into_iter().filter(|r| r.contains("dl-del")) {
            assert!(!row.contains("data-word-ins"));
        }
    }

    #[test]
    fn unpaired_insertion_has_no_word_ranges() {
        let html = down("Keep.\n", "Keep.\n\nAdded.\n");
        assert!(!html.contains("data-word-"));
    }

    #[test]
    fn ranges_are_utf16_code_units() {
        let html = down("é 😀 old 漢\n", "é 😀 new 漢\n");
        assert!(html.contains("data-word-del=\"5-8\""), "{html}");
        assert!(html.contains("data-word-ins=\"5-8\""), "{html}");
    }

    // MARK: Edge cases

    #[test]
    fn all_content_deleted() {
        let html = down("Gone.\n", "");
        assert_eq!(
            html,
            "<div class=\"line dl-del\" data-change-id=\"change-1\">Gone.</div>"
        );
    }

    #[test]
    fn all_content_inserted() {
        let html = down("", "Brand new.\n");
        assert_eq!(
            html,
            "<div class=\"line dl-ins\" data-line=\"1\" data-change-id=\"change-1\">Brand new.</div>"
        );
    }

    // MARK: Line-level diffs within paired blocks

    #[test]
    fn multi_line_paragraph_only_changed_lines_marked() {
        let html = down(
            "Line one.\nLine two.\nLine three.\n",
            "Line one.\nLine TWO.\nLine three.\n",
        );
        assert_eq!(count(&html, "dl-ins"), 1);
        assert_eq!(count(&html, "dl-del"), 1);
    }

    #[test]
    fn unchanged_lines_in_modified_block_are_normal() {
        let html = down(
            "Line one.\nLine two.\nLine three.\n",
            "Line one.\nLine TWO.\nLine three.\n",
        );
        assert!(html.contains("<div class=\"line\" data-line=\"1\">Line one.</div>"));
    }

    #[test]
    fn line_level_deletion_precedes_insertion() {
        let html = down(
            "Keep.\nOld line.\nKeep too.\n",
            "Keep.\nNew line.\nKeep too.\n",
        );
        assert!(html.find("Old").unwrap() < html.find("New").unwrap());
    }

    #[test]
    fn word_ranges_on_line_paired_content() {
        let html = down(
            "Start.\nThe quick fox.\nEnd.\n",
            "Start.\nThe slow fox.\nEnd.\n",
        );
        assert!(
            html.contains("data-line=\"2\" data-change-id=\"change-2\" data-word-ins=\"4-8\""),
            "{html}"
        );
    }

    #[test]
    fn word_ranges_pair_by_best_match_not_position() {
        let html = down(
            "Unrelated first line of text.\nAnother different line here.\nThe quick brown fox jumps over.\n",
            "The quick red fox jumps over.\n",
        );
        // Only "brown" → "red" differs against the best-matching line.
        assert!(
            html.contains("data-word-ins=\"10-13\">The quick red fox jumps over."),
            "{html}"
        );
    }

    #[test]
    fn all_lines_changed_degenerates_to_full_replacement() {
        let html = down("Alpha.\nBeta.\nGamma.\n", "One.\nTwo.\nThree.\n");
        assert_eq!(count(&html, "dl-ins"), 3);
        assert_eq!(count(&html, "dl-del"), 3);
    }

    #[test]
    fn single_line_block_behavior_unchanged() {
        let html = down("Original.\n", "Changed.\n");
        assert_eq!((count(&html, "dl-ins"), count(&html, "dl-del")), (1, 1));
    }

    #[test]
    fn change_id_only_on_changed_lines() {
        let html = down("Alpha.\nBeta.\nGamma.\n", "Alpha.\nBETA.\nGamma.\n");
        assert_eq!(count(&html, "data-change-id"), 2);
    }

    #[test]
    fn fenced_code_block_line_level_diff() {
        let html = down(
            "```\nalpha\nbeta\ngamma\n```\n",
            "```\nalpha\nBETA\ngamma\n```\n",
        );
        assert_eq!((count(&html, "dl-ins"), count(&html, "dl-del")), (1, 1));
        assert!(html.contains("data-line=\"3\" data-change-id"));
    }

    #[test]
    fn fenced_code_block_nested_in_a_list_maps_lines_past_the_fence() {
        let html = down(
            "- Item\n\n  ```\n  keep\n  old\n  ```\n",
            "- Item\n\n  ```\n  keep\n  new\n  ```\n",
        );
        assert!(
            html.contains(
                "data-line=\"5\" data-change-id=\"change-3\" data-word-ins=\"2-5\">  new"
            ),
            "{html}"
        );
    }

    #[test]
    fn multi_line_list_item_line_level_diff() {
        let html = down(
            "- First line\n  second line\n  third line\n",
            "- First line\n  SECOND line\n  third line\n",
        );
        assert_eq!((count(&html, "dl-ins"), count(&html, "dl-del")), (1, 1));
    }

    #[test]
    fn word_ranges_align_on_indented_list_continuation() {
        let html = down(
            "- First line\n  second line\n  third line\n",
            "- First line\n  SECOND line\n  third line\n",
        );
        assert!(
            html.contains("data-word-ins=\"2-8\">  SECOND line"),
            "{html}"
        );
        assert!(
            html.contains("data-word-del=\"2-8\">  second line"),
            "{html}"
        );
    }

    #[test]
    fn lines_added_within_paired_block() {
        let html = down("First.\nSecond.\n", "First.\nInserted.\nSecond.\n");
        assert_eq!((count(&html, "dl-ins"), count(&html, "dl-del")), (1, 0));
    }

    #[test]
    fn lines_deleted_within_paired_block() {
        let html = down("First.\nMiddle.\nLast.\n", "First.\nLast.\n");
        assert_eq!((count(&html, "dl-ins"), count(&html, "dl-del")), (0, 1));
    }

    #[test]
    fn code_block_deletion_lines_have_no_ins_ranges() {
        let html = down(
            "```\nalpha\nbeta gamma\n```\n",
            "```\nalpha\nBETA gamma\n```\n",
        );
        for row in rows(&html).into_iter().filter(|r| r.contains("dl-del")) {
            assert!(!row.contains("data-word-ins"));
        }
        for row in rows(&html).into_iter().filter(|r| r.contains("dl-ins")) {
            assert!(!row.contains("data-word-del"));
        }
    }

    #[test]
    fn frontmatter_lines_are_never_marked_and_numbering_spans_the_file() {
        let html = down(
            "---\ntitle: A\n---\nBody one.\n",
            "---\ntitle: B\n---\nBody two.\n",
        );
        assert!(html.contains("<div class=\"line\" data-line=\"2\">title: B</div>"));
        assert!(
            html.contains("<div class=\"line dl-ins\" data-line=\"4\" data-change-id=\"change-2\"")
        );
    }

    #[test]
    fn footnote_definitions_are_diffed_in_down_mode() {
        let html = down(
            "Text[^a].\n\n[^a]: Old body.\n",
            "Text[^a].\n\n[^a]: New body.\n",
        );
        assert!(html.contains("dl-del") && html.contains("dl-ins"));
    }

    #[test]
    fn down_emits_no_group_attributes() {
        let html = down("Alpha.\n\nBeta.\n", "Alpha changed.\n\nBeta.\n\nAdded.\n");
        assert!(!html.contains("data-group"));
    }

    // MARK: Sidebar parity

    #[test]
    fn sidebar_and_down_html_change_ids_match() {
        let (old, new) = ("Keep.\n\nOriginal.\n", "Keep.\n\nRevised.\n");
        let html = down(old, new);
        for change in compute_changes(old, new, TrackerMode::Down.policy(), 0.25) {
            assert!(html.contains(&format!("data-change-id=\"{}\"", change.id)));
        }
    }

    #[test]
    fn code_block_sidebar_and_down_html_change_ids_match() {
        let (old, new) = ("```\nkeep\nold\n```\n", "```\nkeep\nnew\n```\n");
        let html = down(old, new);
        for change in compute_changes(old, new, TrackerMode::Down.policy(), 0.25) {
            assert!(html.contains(&format!("data-change-id=\"{}\"", change.id)));
        }
    }
}
