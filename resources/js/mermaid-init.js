// Mermaid diagram initializer for Up mode.
// Injected as a script after mermaid.min.js, alongside highlight-init.js and
// math-init.js — the same "explicit init script" pattern for each
// client-side renderer, rather than relying on Mermaid's own
// `startOnLoad` auto-scan (which looks for `.mermaid` elements, not the
// `<pre><code class="language-mermaid">` mudl-core actually emits — see
// `render_up` in `crates/mudl-core/src/render.rs`).
//
// Each fenced ```mermaid block is unwrapped from its `<pre><code>` into a
// bare `<div class="mermaid">` holding the raw (entity-decoded) diagram
// source, which is the element shape Mermaid's `run()` expects to replace
// with rendered SVG in place.

(function () {
  "use strict";
  if (!document.querySelector(".up-mode-output")) return;
  if (typeof mermaid === "undefined") return;

  var blocks = [];
  document.querySelectorAll("pre > code.language-mermaid").forEach(
    function (code) {
      var pre = code.parentElement;
      if (!pre || pre.tagName !== "PRE") return;

      // .textContent decodes the entities render_up escaped, handing
      // Mermaid the plain diagram source it expects.
      var source = code.textContent;
      var div = document.createElement("div");
      div.className = "mermaid";
      div.textContent = source;
      pre.parentNode.replaceChild(div, pre);
      blocks.push(div);
    }
  );

  if (!blocks.length) return;

  mermaid.initialize({ startOnLoad: false });
  mermaid.run({ nodes: blocks }).catch(function () {
    // A malformed diagram is left as Mermaid's own error rendering inside
    // the same .mermaid element — nothing more to do here.
  });
})();
