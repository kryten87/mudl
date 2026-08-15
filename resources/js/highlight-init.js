// highlight.js initializer for Up mode code blocks.
// Injected as a script after highlight.min.js, mirroring mermaid-init.js's
// pattern for the other client-side renderer.
//
// mudl-core emits every fenced code block as plain escaped text tagged with
// a `language-X` class and nothing else (see `render_up` in
// `crates/mudl-core/src/render.rs`) — this file is what actually calls
// highlight.js on it. `language-mermaid` and `language-math` are excluded:
// those are handled by mermaid-init.js and math-init.js, which by the time
// this runs may already have replaced their `<pre><code>` elements with a
// `<div class="mermaid">` or a `<div class="mud-math-block">`. Excluding
// them by class (rather than relying on running after those two scripts)
// means the three init scripts' relative order doesn't matter.
//
// A code block with no language class is left alone — matching `mud`'s own
// CodeHighlighter, which skips highlighting entirely rather than falling
// back to hljs's language auto-detection.

(function () {
  "use strict";
  if (!document.querySelector(".up-mode-output")) return;
  if (typeof hljs === "undefined") return;

  document.querySelectorAll('code[class*="language-"]').forEach(
    function (code) {
      if (code.classList.contains("language-mermaid")) return;
      if (code.classList.contains("language-math")) return;
      hljs.highlightElement(code);
    }
  );
})();
