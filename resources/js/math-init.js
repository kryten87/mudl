// Temml math initializer for Up mode.
// Injected as a script after temml.min.js, mirroring mermaid-init.js's
// pattern for the other client-side renderer.
//
// mudl-core emits a ```math fenced block as plain escaped text tagged
// `language-math` and nothing else (see `render_up` in
// `crates/mudl-core/src/render.rs`) — the client-side mirror of `mud`'s own
// `MathRenderer.swift`, which runs this same Temml call server-side in a
// JSContext. The options passed to `renderToString` match
// `MathRenderer.swift` exactly:
//
//   - `displayMode: true`  — this is always the block-level ```math form
//     (the only math form mudl-core emits today; `mud`'s inline `` $`…`$ ``
//     and `$$…$$`-paragraph forms aren't implemented yet).
//   - `throwOnError: false` — so invalid TeX doesn't throw at all: Temml
//     embeds its own `<span class="temml-error">` directly in the returned
//     MathML string (the same in-place error style GitHub uses), which ends
//     up inside the `mud-math-block` wrapper below like any other render.
//
// The try/catch below is for the case `MathRenderer.swift`'s `nil` return
// covers — the JS layer itself failing (temml missing, or `renderToString`
// throwing for some reason other than malformed TeX) — and mirrors its
// fallback: escaped raw TeX text, here wrapped in the same `temml-error`
// class Temml's own internal error handling uses, so both failure paths
// read identically to a reader and to `mudl-server`'s asset-selection logic.
//
// `mudl-server`'s `select_assets` keys off the literal substrings `<math`,
// `mud-math-block`, and `temml-error` appearing in the rendered HTML — the
// two wrapper classes below are exactly that contract, not a styling choice.

(function () {
  "use strict";
  if (!document.querySelector(".up-mode-output")) return;
  if (typeof temml === "undefined") return;

  document.querySelectorAll("pre > code.language-math").forEach(
    function (code) {
      var pre = code.parentElement;
      if (!pre || pre.tagName !== "PRE") return;

      // .textContent decodes the entities render_up escaped, handing Temml
      // the plain TeX source it expects.
      var tex = code.textContent;

      try {
        var mathml = temml.renderToString(tex, {
          displayMode: true,
          throwOnError: false,
        });
        var block = document.createElement("div");
        block.className = "mud-math-block";
        block.innerHTML = mathml;
        pre.parentNode.replaceChild(block, pre);
      } catch (e) {
        var span = document.createElement("span");
        span.className = "temml-error";
        span.textContent = tex;
        pre.parentNode.replaceChild(span, pre);
      }
    }
  );
})();
