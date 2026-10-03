// mudl - Down Mode helpers.
// Auto-detects context via .down-mode-output; no-ops otherwise.
//
// Down mode's raw-source view is one `<div class="line" data-line="N">` per
// source line, each line HTML-escaped (see `render_down` in
// `crates/mudl-core/src/render.rs`). That function's doc comment already
// notes the design insight this file acts on: highlighting each line in
// total isolation from its neighbors is exactly what sidesteps `mud`'s
// `HTMLLineSplitter` problem (re-splitting an already-highlighted span that
// crosses a line boundary) rather than solving it after the fact — scoping
// each highlight.js call to a single line's own text means no highlighted
// `<span>` can ever straddle a line boundary, so there is nothing left for a
// line-splitter to do.
//
// `language: "markdown"` rather than `hljs.highlightAuto`: the source is
// always Markdown here (this is Down mode's raw-source view, not a fenced
// code block of unknown language), so there is nothing to detect.
// `highlightAuto` would spend time guessing on every line and could flip
// languages from one line to the next on short or ambiguous text (a lone
// word, a blank line), giving inconsistent coloring where naming the
// language explicitly gives none.
//
// The known tradeoff (accepted by the plan this implements): a fenced code
// block's interior lines lose their own language's highlighting and read as
// plain Markdown instead, since each line is highlighted with no memory of
// the lines around it.

// Change tracking: a changed row carries its changed words as
// `data-word-ins` / `data-word-del` character ranges rather than inline
// `<ins>`/`<del>` (which the `innerHTML` swap below would wipe), and
// mud-changes.js — loaded before this file whenever the page has change
// markup — wraps them once the row is highlighted. Without highlight.js the
// rows keep their escaped text and still get their markers.

(function () {
  "use strict";
  if (!document.querySelector(".down-mode-output")) return;
  var highlight = typeof hljs !== "undefined";
  var changes = window.Mud && window.Mud.changes;
  var markWords = changes && changes.applyWordMarkers;
  if (!highlight && !markWords) return;

  var lines = document.querySelectorAll(".down-mode-output .line");
  for (var i = 0; i < lines.length; i++) {
    var el = lines[i];
    if (highlight) {
      // .textContent decodes the entities render_down escaped, handing
      // highlight.js the plain source text it expects.
      var text = el.textContent;
      try {
        el.innerHTML = hljs.highlight(text, { language: "markdown" }).value;
      } catch (e) {
        // Leave the line's original (already-escaped) markup in place.
      }
    }
    if (markWords &&
        (el.hasAttribute("data-word-ins") || el.hasAttribute("data-word-del"))) {
      markWords(el);
    }
  }
})();
