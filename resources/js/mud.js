// mudl - Shared client-side helpers (find, scroll, zoom).
// Exposed on window.Mud; loaded as a plain <script> in every served page.
// There is no native Swift-style bridge in this architecture (see
// docs/IMPLEMENTATION-PLAN.md Phase 10.2) — the GTK shell talks to the page
// via plain navigation and small injected scripts, not persistent
// message-handler calls, so every function here is just called directly
// from the page (or, later, from an injected script) rather than through
// `evaluateJavaScript`.

(function () {
  "use strict";

  // What Find searches. Up mode is two roots, not one: the bottom Comments
  // section is a `<footer>` beside the article rather than inside it, and its
  // text is still part of the document a reader searches.
  function CONTAINERS() {
    return document.querySelector(".up-mode-output")
        ? ".up-mode-output, footer.comments"
        : ".down-mode-output";
  }
  var MATCH_CLASS = "mud-match";
  var ACTIVE_CLASS = "mud-match-active";

  var marks = [];       // current <mark> elements in DOM order
  var activeIndex = -1; // index of the currently-active match

  // -- Highlight helpers ---------------------------------------------------

  // Walk all text nodes inside the search roots, split at case-insensitive
  // matches, and wrap each match in <mark class="mud-match">.
  function highlightAll(text) {
    clearHighlights();
    if (!text) return;

    var roots = document.querySelectorAll(CONTAINERS());
    if (!roots.length) return;

    var pattern = new RegExp(
      text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"),
      "gi"
    );

    // Collect text nodes first (mutating the DOM while walking is unsafe).
    // Roots come back in document order, so the marks do too.
    var nodes = [];
    var node;
    for (var r = 0; r < roots.length; r++) {
      var walker = document.createTreeWalker(
        roots[r],
        NodeFilter.SHOW_TEXT,
        null
      );
      while ((node = walker.nextNode())) nodes.push(node);
    }

    for (var i = 0; i < nodes.length; i++) {
      var textNode = nodes[i];
      var value = textNode.nodeValue;
      var match;
      var lastIndex = 0;
      var parts = [];
      pattern.lastIndex = 0;

      while ((match = pattern.exec(value)) !== null) {
        if (match.index > lastIndex) {
          parts.push(document.createTextNode(
            value.slice(lastIndex, match.index)
          ));
        }
        var mark = document.createElement("mark");
        mark.className = MATCH_CLASS;
        mark.textContent = match[0];
        parts.push(mark);
        lastIndex = pattern.lastIndex;
        // Guard against zero-length matches.
        if (match[0].length === 0) pattern.lastIndex++;
      }

      if (parts.length === 0) continue;

      if (lastIndex < value.length) {
        parts.push(document.createTextNode(value.slice(lastIndex)));
      }

      var parent = textNode.parentNode;
      for (var j = 0; j < parts.length; j++) {
        parent.insertBefore(parts[j], textNode);
      }
      parent.removeChild(textNode);
    }

    marks = [];
    for (var k = 0; k < roots.length; k++) {
      marks = marks.concat(Array.prototype.slice.call(
        roots[k].querySelectorAll("mark." + MATCH_CLASS)
      ));
    }
  }

  function activateMatch(n) {
    if (marks.length === 0) return;
    if (activeIndex >= 0 && activeIndex < marks.length) {
      marks[activeIndex].classList.remove(ACTIVE_CLASS);
    }
    activeIndex = ((n % marks.length) + marks.length) % marks.length;
    var el = marks[activeIndex];
    el.classList.add(ACTIVE_CLASS);
    // A match inside a folded section has nothing to scroll to until the
    // section opens. Matches stay counted while folded, so stepping through
    // them with Cmd+G still walks the whole document.
    if (window.Mud.folds) window.Mud.folds.reveal(el);
    el.scrollIntoView({ block: "center", behavior: "smooth" });
  }

  function clearHighlights() {
    for (var i = 0; i < marks.length; i++) {
      var mark = marks[i];
      var parent = mark.parentNode;
      if (!parent) continue;
      parent.replaceChild(document.createTextNode(mark.textContent), mark);
      parent.normalize();
    }
    marks = [];
    activeIndex = -1;
  }

  function result() {
    return { total: marks.length, current: activeIndex + 1 };
  }

  // -- Find API ------------------------------------------------------------

  function findFromTop(text) {
    highlightAll(text);
    if (marks.length > 0) activateMatch(0);
    return result();
  }

  function findRefine(text) {
    // Remember the active match's viewport position so we can pick the
    // nearest match after re-highlighting.
    var refY = null;
    if (activeIndex >= 0 && activeIndex < marks.length) {
      refY = marks[activeIndex].getBoundingClientRect().top;
    }

    highlightAll(text);

    if (marks.length === 0) return result();

    if (refY !== null) {
      // Pick the match closest to the previous active position.
      var best = 0;
      var bestDist = Infinity;
      for (var i = 0; i < marks.length; i++) {
        var d = Math.abs(marks[i].getBoundingClientRect().top - refY);
        if (d < bestDist) { bestDist = d; best = i; }
      }
      activateMatch(best);
    } else {
      activateMatch(0);
    }
    return result();
  }

  function findAdvance(text, direction) {
    // If highlights are stale or absent, rebuild them.
    if (marks.length === 0) {
      highlightAll(text);
      if (marks.length === 0) return result();
      activateMatch(0);
      return result();
    }

    var delta = direction === "backward" ? -1 : 1;
    activateMatch(activeIndex + delta);
    return result();
  }

  function findClear() {
    clearHighlights();
  }

  // -- Scroll --------------------------------------------------------------

  function getScrollY() {
    return window.scrollY;
  }

  function setScrollY(y) {
    window.scrollTo(0, y);
  }

  function getScrollFraction() {
    var maxScroll = document.documentElement.scrollHeight - window.innerHeight;
    if (maxScroll <= 0) return 0;
    return window.scrollY / maxScroll;
  }

  function setScrollFraction(f) {
    var maxScroll = document.documentElement.scrollHeight - window.innerHeight;
    window.scrollTo(0, f * maxScroll);
  }

  // -- Outline navigation ---------------------------------------------------

  function scrollToHeading(slug) {
    var el = document.getElementById(slug);
    if (!el) return;
    // Navigating to a folded heading opens it, and opens whatever it sits in.
    if (window.Mud.folds) window.Mud.folds.revealHeading(slug);
    el.scrollIntoView({ behavior: "smooth", block: "start" });
  }

  function scrollToLine(lineNumber) {
    var lines = document.querySelectorAll(".down-lines .dl");
    var idx = lineNumber - 1;
    if (idx >= 0 && idx < lines.length) {
      lines[idx].scrollIntoView({ behavior: "smooth", block: "start" });
    }
  }

  // -- Body classes ---------------------------------------------------------

  function setClass(name, enabled) {
    if (enabled) {
      document.documentElement.classList.add(name);
    } else {
      document.documentElement.classList.remove(name);
    }
    if (name === "is-auto-expand-changes" && Mud.applyAutoExpandChanges) {
      Mud.applyAutoExpandChanges(enabled);
    }
    if (name === "is-comments-column" && Mud.comments && Mud.comments.setVisible) {
      Mud.comments.setVisible(enabled);
    }
    if (name === "show-comment-markers" && Mud.comments &&
        Mud.comments.setMarkersShown) {
      Mud.comments.setMarkersShown(enabled);
    }
    if (name === "is-foldable-headings" && Mud.folds) {
      Mud.folds.setEnabled(enabled);
    }
  }

  // -- Blocked remote images --------------------------------------------------

  // A remote http(s) image that fails to load usually means `img-src` left
  // it out (`docs/SECURITY.md` Finding 4 — remote images are off by default
  // so a document can't beacon the reader's IP and open-time to whoever
  // wrote it), not a dead link. The browser's own fallback for a failed
  // image is just the bare alt text with no visible box, which gives no
  // hint that anything was hidden — so a failed *remote* image (same-origin
  // `/local/...`/`/assets/...` failures are left to the browser's ordinary
  // broken-image rendering, since those aren't this feature's doing) is
  // swapped for a placeholder that says so and names the menu item that
  // turns it back on.

  function isRemoteImage(img) {
    if (!/^https?:\/\//i.test(img.src)) return false;
    try {
      return new URL(img.src).origin !== window.location.origin;
    } catch (e) {
      return false;
    }
  }

  // Reads the answer back out of the page's own CSP `<meta>` tag rather than
  // threading a separate flag through the template — that tag is already
  // the single source of truth for whether this page's `img-src` admits
  // `https:`/`http:` (`crates/mudl-server/src/document.rs`'s `csp_img_src`).
  function remoteImagesCurrentlyAllowed() {
    var meta = document.querySelector(
      'meta[http-equiv="Content-Security-Policy"]'
    );
    var match = meta && /img-src([^;]*)/.exec(meta.content);
    return !!match && /https?:/.test(match[1]);
  }

  // Capture phase: `error` doesn't bubble, so a listener on `document` only
  // sees it at all if attached for the capture phase.
  document.addEventListener("error", function (event) {
    var img = event.target;
    if (!img || img.tagName !== "IMG" || img.dataset.mudBlockedShown) return;
    if (!isRemoteImage(img)) return;
    img.dataset.mudBlockedShown = "1";

    var label = img.getAttribute("alt") || img.src;
    var note = document.createElement("span");
    note.className = "mud-blocked-image";
    note.textContent = remoteImagesCurrentlyAllowed()
      ? "Image failed to load: " + label
      : "External image hidden (" + label + ") — enable via View > Show External Images";
    img.replaceWith(note);
  }, true);

  // -- Theme ----------------------------------------------------------------

  function setTheme(cssString) {
    var el = document.getElementById("mud-theme");
    if (el) el.textContent = cssString;
  }

  // -- Zoom ----------------------------------------------------------------

  function setZoom(level) {
    document.documentElement.style.zoom = level;
  }

  // -- Geometry ------------------------------------------------------------

  // Shared zoom/position helpers for the app-only overlay and comment-column
  // files (mud-changes.js, mud-comments-edit.js). The document `zoom` on <html>
  // scales the whole layout, so getBoundingClientRect reports zoomed viewport
  // pixels while overlays and capsules position in pre-zoom layout pixels; these
  // convert between the two. (mud-comments.js keeps its own offsetParent-based
  // layoutTop instead of using this — it is inlined into exports without mud.js,
  // so it must stay self-contained.)
  var geometry = {
    // The current document zoom factor (1 when unset).
    zoom: function () {
      return parseFloat(document.documentElement.style.zoom) || 1;
    },
    // A rect's top as an absolute position from the document top, in layout
    // pixels — the same space mud-comments.js's layoutTop returns, via the
    // viewport rect and page scroll (used where an offsetParent walk isn't
    // handy, e.g. a selection range).
    layoutTopFromRect: function (rect) {
      return (rect.top + window.scrollY) / geometry.zoom();
    },
    // A viewport Y coordinate converted to a position relative to a container's
    // scrolled content: subtract the container's viewport top, undo the zoom,
    // then add the container's scrollTop.
    viewportToLayout: function (viewportY, containerRect, scrollTop) {
      return (viewportY - containerRect.top) / geometry.zoom() + scrollTop;
    }
  };

  // -- Public namespace ----------------------------------------------------

  // Merge rather than assign, so the namespace is built the same defensive way
  // in every file and injection order is not a silent requirement. mud.js is
  // still loaded first (see the document template's script order), because it
  // seeds the shared helpers the other files call at runtime.
  window.Mud = window.Mud || {};
  Object.assign(window.Mud, {
    findFromTop: findFromTop,
    findRefine: findRefine,
    findAdvance: findAdvance,
    findClear: findClear,
    getScrollY: getScrollY,
    setScrollY: setScrollY,
    getScrollFraction: getScrollFraction,
    setScrollFraction: setScrollFraction,
    setTheme: setTheme,
    setClass: setClass,
    setZoom: setZoom,
    scrollToHeading: scrollToHeading,
    scrollToLine: scrollToLine,
    geometry: geometry
  });
})();
