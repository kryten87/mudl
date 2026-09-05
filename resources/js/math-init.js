// Temml math initializer for Up mode.
// Injected as a script after temml.min.js, mirroring mermaid-init.js's
// pattern for the other client-side renderer.
//
// mudl-core (`crates/mudl-core/src/render.rs`) tags each of GitHub's three
// math forms with plain, escaped-TeX markup for this script to upgrade:
//
//   - A fenced ```math block, or a `$$ ... $$` paragraph (rendered as the
//     identical markup — see `push_math_fence`) — both as
//     `<pre><code class="language-math">`, upgraded below with
//     `displayMode: true` and swapped for a `mud-math-block` div.
//   - `` $`…`$ `` inline math (`push_inline_math`) — as
//     `<code class="language-math-inline">` with no `<pre>` wrapper (it
//     has to stay in the text flow), upgraded with `displayMode: false`
//     and swapped for a `mud-math-inline` span.
//
// `throwOnError: false` in both cases means invalid TeX doesn't throw at
// all: Temml embeds its own `<span class="temml-error">` directly in the
// returned MathML string (the same in-place error style GitHub uses),
// which ends up inside the wrapper below like any other render.
//
// The try/catch in each loop is for the JS layer itself failing (temml
// missing, or `renderToString` throwing for some reason other than
// malformed TeX) — mirroring `mud`'s `MathRenderer.swift`'s own `nil`
// fallback: escaped raw TeX text, wrapped in the same `temml-error` class
// Temml's own internal error handling uses, so both failure paths read
// identically to a reader and to `mudl-server`'s asset-selection logic.
//
// `mudl-server`'s `select_assets` keys off the literal substrings
// `language-math`, `<math`, `mud-math-block`, and `temml-error` appearing
// in the rendered HTML — the wrapper classes below (both the ones this
// script consumes and the ones it produces) are exactly that contract,
// not a styling choice.

(function () {
  "use strict";
  if (!document.querySelector(".up-mode-output")) return;
  if (typeof temml === "undefined") return;

  function renderMath(code, displayMode, wrapperTag, wrapperClass) {
    var tex = code.textContent;
    try {
      var mathml = temml.renderToString(tex, {
        displayMode: displayMode,
        throwOnError: false,
      });
      var wrapper = document.createElement(wrapperTag);
      wrapper.className = wrapperClass;
      wrapper.innerHTML = mathml;
      return wrapper;
    } catch (e) {
      var span = document.createElement("span");
      span.className = "temml-error";
      span.textContent = tex;
      return span;
    }
  }

  document.querySelectorAll("pre > code.language-math").forEach(
    function (code) {
      var pre = code.parentElement;
      if (!pre || pre.tagName !== "PRE") return;
      var replacement = renderMath(code, true, "div", "mud-math-block");
      pre.parentNode.replaceChild(replacement, pre);
    }
  );

  document.querySelectorAll("code.language-math-inline").forEach(
    function (code) {
      var replacement = renderMath(code, false, "span", "mud-math-inline");
      code.parentNode.replaceChild(replacement, code);
    }
  );
})();
