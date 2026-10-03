// mudl - Change tracking: overlays, expand/collapse, navigation, and Down
// mode's word markers. Ported from `mud`'s mud-changes.js.
//
// Loaded only when the rendered body carries change markup
// (`data-change-id`, see `select_assets` in `crates/mudl-core/src/template.rs`),
// and placed right after mud.js so `Mud.changes.applyWordMarkers` exists by
// the time mud-down.js highlights its lines. The markup contract it reads is
// emitted by `crates/mudl-core/src/changes/` (Up: `mud-change-ins`/`-del`
// blocks with `data-group-*`, `.cl` code-diff lines; Down: `.line.dl-ins` /
// `.line.dl-del` rows with `data-word-ins` / `data-word-del` ranges).
//
// The GTK shell calls the public functions at the bottom through
// `evaluate_javascript`.

(function () {
  "use strict";

  window.Mud = window.Mud || {};

  // -- Down mode: word markers -----------------------------------------------

  // Parses a `data-word-ins` / `data-word-del` value ("4-9,15-18": UTF-16
  // code-unit offsets into the row's text, end exclusive) into markers.
  function parseRanges(value, tag, out) {
    if (!value) return;
    var parts = value.split(",");
    for (var i = 0; i < parts.length; i++) {
      var m = /^\s*(\d+)-(\d+)\s*$/.exec(parts[i]);
      if (!m) continue;
      var start = +m[1];
      var end = +m[2];
      if (end > start) out.push({ start: start, end: end, tag: tag });
    }
  }

  function collectTextNodes(node, out) {
    for (var c = node.firstChild; c; c = c.nextSibling) {
      if (c.nodeType === 3) {
        out.push(c);
      } else if (c.nodeType === 1) {
        collectTextNodes(c, out);
      }
    }
  }

  // Wraps the row's `data-word-ins` / `data-word-del` ranges in <ins> / <del>.
  // Runs after mud-down.js has replaced the row's markup with highlight.js's,
  // so it walks the row's text nodes rather than its HTML: each text node's
  // slice of a range gets its own wrapper, which closes a marker before an
  // element boundary and reopens it after, as `mud`'s
  // `DownHTMLVisitor.injectMarkers` does on the HTML string. JS string
  // indices are UTF-16 code units, the same unit the ranges use.
  function applyWordMarkers(el) {
    if (!el || !el.getAttribute) return;
    var markers = [];
    parseRanges(el.getAttribute("data-word-ins"), "ins", markers);
    parseRanges(el.getAttribute("data-word-del"), "del", markers);
    if (!markers.length) return;
    markers.sort(function (a, b) { return a.start - b.start; });

    var doc = el.ownerDocument || document;
    var nodes = [];
    collectTextNodes(el, nodes);

    var offset = 0;
    var m = 0;
    for (var i = 0; i < nodes.length && m < markers.length; i++) {
      var node = nodes[i];
      var text = node.nodeValue;
      var nodeStart = offset;
      var nodeEnd = offset + text.length;
      offset = nodeEnd;

      while (m < markers.length && markers[m].end <= nodeStart) m++;
      if (m >= markers.length || markers[m].start >= nodeEnd) continue;

      var pieces = [];
      var pos = nodeStart;
      while (m < markers.length && markers[m].start < nodeEnd) {
        var mk = markers[m];
        var s = Math.max(mk.start, pos);
        var e = Math.min(mk.end, nodeEnd);
        if (s > pos) {
          pieces.push({ text: text.slice(pos - nodeStart, s - nodeStart) });
        }
        if (e > s) {
          pieces.push({
            text: text.slice(s - nodeStart, e - nodeStart),
            tag: mk.tag
          });
        }
        if (e > pos) pos = e;
        // A marker running past this node continues in the next one.
        if (mk.end > nodeEnd) break;
        m++;
      }
      if (pos < nodeEnd) pieces.push({ text: text.slice(pos - nodeStart) });

      var parent = node.parentNode;
      for (var p = 0; p < pieces.length; p++) {
        var piece = pieces[p];
        var replacement = doc.createTextNode(piece.text);
        if (piece.tag) {
          var wrapper = doc.createElement(piece.tag);
          wrapper.appendChild(replacement);
          replacement = wrapper;
        }
        parent.insertBefore(replacement, node);
      }
      parent.removeChild(node);
    }
  }

  window.Mud.changes = { applyWordMarkers: applyWordMarkers };

  // Nothing else here applies outside a served page (the self-test page under
  // docs/examples/ loads this file for applyWordMarkers alone).
  if (typeof document === "undefined" || !document.documentElement) return;

  // -- Shared ----------------------------------------------------------------

  var geo = window.Mud.geometry || {
    zoom: function () {
      return parseFloat(document.documentElement.style.zoom) || 1;
    },
    viewportToLayout: function (viewportY, containerRect, scrollTop) {
      return (viewportY - containerRect.top) / this.zoom() + scrollTop;
    }
  };

  function upContainer() {
    return document.querySelector(".up-mode-output");
  }

  function rootHas(cls) {
    return document.documentElement.classList.contains(cls);
  }

  // On screen: laid out, and not one of our own overlays.
  function isShown(el) {
    return el.offsetParent !== null && !el.classList.contains("mud-overlay");
  }

  // Inside a folded section (mud-up.js). The hiding rule only applies while
  // foldable headings are on.
  function isFoldHidden(el) {
    return rootHas("is-foldable-headings") && !!el.closest(".is-fold-hidden");
  }

  function groupMembers(container, gid) {
    return container.querySelectorAll(
      '[data-group-id="' + gid + '"]:not(.mud-overlay)'
    );
  }

  // -- Overlays (Up mode) ----------------------------------------------------

  var _overlays = {};       // groupID → overlay element
  var _expandedGroups = {}; // groupID → true when expanded
  var _groupTypes = {};     // groupID → "ins" | "del" | "mix"

  // Sub-overlays: temporary red/green overlays created when a mixed group is
  // expanded. Each entry: { overlay, els, groupId }.
  var _subOverlays = [];
  // Group IDs whose original overlay is suppressed (replaced by sub-overlays).
  var _suppressedGroups = {};

  function buildOverlays() {
    var container = upContainer();
    if (!container) return;

    var old = container.querySelectorAll(".mud-overlay");
    for (var i = 0; i < old.length; i++) old[i].remove();
    _overlays = {};
    _expandedGroups = {};
    _groupTypes = {};
    _subOverlays = [];
    _suppressedGroups = {};

    // The group's type is computed server-side and carried on every member,
    // so members hidden from the DOM (a collapsed deletion) still count.
    var els = container.querySelectorAll("[data-group-id]");
    var groups = {};
    var order = [];
    for (var j = 0; j < els.length; j++) {
      var id = els[j].dataset.groupId;
      if (!groups[id]) {
        groups[id] = { index: "", type: "" };
        order.push(id);
      }
      if (!groups[id].index && els[j].dataset.groupIndex) {
        groups[id].index = els[j].dataset.groupIndex;
      }
      if (!groups[id].type && els[j].dataset.groupType) {
        groups[id].type = els[j].dataset.groupType;
      }
    }

    var autoExpand = rootHas("is-auto-expand-changes");

    for (var k = 0; k < order.length; k++) {
      var gid = order[k];
      var g = groups[gid];
      var type = g.type || "ins";
      _groupTypes[gid] = type;

      var div = document.createElement("div");
      div.className = "mud-overlay mud-overlay-" + type;
      div.dataset.groupId = gid;
      div.dataset.groupIndex = g.index;

      var btn = document.createElement("button");
      btn.type = "button";
      btn.className = "mud-expando";
      btn.textContent = g.index;
      div.appendChild(btn);

      if (type === "ins") {
        // Insertion-only groups are always expanded; the badge is a label.
        btn.classList.add("mud-expando-expanded");
        btn.disabled = true;
        btn.setAttribute("aria-expanded", "true");
      } else {
        btn.setAttribute("aria-expanded", "false");
        btn.setAttribute("aria-label", "Show deleted content");
        btn.addEventListener("click", (function (target) {
          return function () { toggleGroup(target); };
        })(gid));
      }

      // Deletion-only groups start as a zero-height marker at the gap.
      if (type === "del" && !autoExpand) {
        div.classList.add("mud-overlay-collapsed");
      }

      container.appendChild(div);
      _overlays[gid] = div;
    }

    if (autoExpand) {
      for (var a = 0; a < order.length; a++) {
        var t = _groupTypes[order[a]];
        if (t === "del" || t === "mix") expandGroup(order[a]);
      }
    }

    positionOverlays();
  }

  // Span `overlay` from the first to the last shown element of `els`.
  function positionOverlay(overlay, els, containerRect, scrollTop) {
    var visible = [];
    for (var i = 0; i < els.length; i++) {
      if (isShown(els[i])) visible.push(els[i]);
    }
    if (visible.length === 0) {
      overlay.style.display = "none";
      return;
    }
    var firstRect = visible[0].getBoundingClientRect();
    var lastRect = visible[visible.length - 1].getBoundingClientRect();
    overlay.style.display = "";
    overlay.style.top =
      geo.viewportToLayout(firstRect.top, containerRect, scrollTop) + "px";
    overlay.style.height =
      ((lastRect.bottom - firstRect.top) / geo.zoom()) + "px";
  }

  // Where a collapsed deletion group's hidden blocks would sit: the bottom of
  // the nearest shown block before them, else the top of the nearest one
  // after. A deletion can be the only content of its parent — the orphan
  // `<table>` / `<ul>` wrappers the renderer adds around deleted rows and
  // items, which CSS hides while they hold nothing visible — so when a level
  // has no shown sibling, look again one level up.
  function collapsedTop(els, container, containerRect, scrollTop) {
    var first = els[0];
    var last = els[els.length - 1];
    while (first && first !== container) {
      var prev = first.previousElementSibling;
      while (prev && !isShown(prev)) prev = prev.previousElementSibling;
      if (prev) {
        return geo.viewportToLayout(
          prev.getBoundingClientRect().bottom, containerRect, scrollTop);
      }
      var next = last.nextElementSibling;
      while (next && !isShown(next)) next = next.nextElementSibling;
      if (next) {
        return geo.viewportToLayout(
          next.getBoundingClientRect().top, containerRect, scrollTop);
      }
      var parent = first.parentElement;
      if (!parent || parent === container) break;
      if (isShown(parent)) {
        return geo.viewportToLayout(
          parent.getBoundingClientRect().top, containerRect, scrollTop);
      }
      first = last = parent;
    }
    return scrollTop;
  }

  function positionCollapsedOverlay(overlay, gid, container, containerRect, scrollTop) {
    var els = groupMembers(container, gid);
    if (els.length === 0 || isFoldHidden(els[0])) {
      overlay.style.display = "none";
      return;
    }
    overlay.style.display = "";
    overlay.style.top =
      collapsedTop(els, container, containerRect, scrollTop) + "px";
  }

  function positionOverlays() {
    var container = upContainer();
    if (!container) return;
    var containerRect = container.getBoundingClientRect();
    var scrollTop = container.scrollTop;

    for (var gid in _overlays) {
      if (_suppressedGroups[gid]) continue;
      var overlay = _overlays[gid];
      if (overlay.classList.contains("mud-overlay-collapsed")) {
        positionCollapsedOverlay(overlay, gid, container, containerRect, scrollTop);
        continue;
      }
      positionOverlay(overlay, groupMembers(container, gid), containerRect, scrollTop);
    }

    for (var i = 0; i < _subOverlays.length; i++) {
      var sub = _subOverlays[i];
      positionOverlay(sub.overlay, sub.els, containerRect, scrollTop);
    }

    // Make consecutive sub-overlays of one group continuous: extend each
    // overlay's bottom to the next one's top. Non-last overlays are "cont",
    // the last "tail".
    for (var j = 0; j < _subOverlays.length; j++) {
      var cur = _subOverlays[j];
      var nxt = _subOverlays[j + 1];
      var isLast = !nxt || nxt.groupId !== cur.groupId;
      cur.overlay.classList.toggle("mud-overlay-cont", !isLast);
      cur.overlay.classList.toggle("mud-overlay-tail", isLast);
      if (isLast) continue;
      if (cur.overlay.style.display === "none") continue;
      if (nxt.overlay.style.display === "none") continue;
      var curTop = parseFloat(cur.overlay.style.top);
      var nextTop = parseFloat(nxt.overlay.style.top);
      if (nextTop > curTop) {
        cur.overlay.style.height = (nextTop - curTop) + "px";
      }
    }
  }

  // -- Expand / collapse -----------------------------------------------------

  function expandGroup(gid) {
    var overlay = _overlays[gid];
    if (!overlay) return;
    _expandedGroups[gid] = true;
    var container = upContainer();
    var type = _groupTypes[gid];

    var els = groupMembers(container, gid);
    for (var i = 0; i < els.length; i++) {
      els[i].classList.add("mud-change-revealed");
    }

    if (type === "del") {
      overlay.classList.remove("mud-overlay-collapsed");
      overlay.classList.add("mud-change-revealed");
    } else if (type === "mix") {
      // Hide the mixed overlay; show red/green sub-overlays per run instead.
      overlay.style.display = "none";
      _suppressedGroups[gid] = true;

      var runs = [];
      var cur = null;
      for (var k = 0; k < els.length; k++) {
        var t = (els[k].classList.contains("mud-change-del") ||
                 els[k].classList.contains("cl-del")) ? "del" : "ins";
        if (cur && cur.type === t) {
          cur.els.push(els[k]);
        } else {
          cur = { type: t, els: [els[k]] };
          runs.push(cur);
        }
      }

      var firstSub = null;
      for (var r = 0; r < runs.length; r++) {
        var div = document.createElement("div");
        div.className = "mud-overlay mud-overlay-" + runs[r].type;
        div.dataset.groupId = gid;
        div.dataset.groupIndex = overlay.dataset.groupIndex;
        div.style.opacity = "0";
        div.setAttribute("aria-hidden", "true");
        container.appendChild(div);
        _subOverlays.push({ overlay: div, els: runs[r].els, groupId: gid });
        if (!firstSub) firstSub = div;
      }

      // The badge moves to the first run; it stays clickable there.
      var moved = overlay.querySelector(".mud-expando");
      if (moved && firstSub) {
        firstSub.removeAttribute("aria-hidden");
        firstSub.appendChild(moved);
      }
    }

    var btn = expandoFor(gid);
    if (btn) {
      btn.classList.add("mud-expando-expanded");
      btn.setAttribute("aria-expanded", "true");
      btn.setAttribute("aria-label", "Hide deleted content");
    }

    positionOverlays();

    if (type === "mix") {
      requestAnimationFrame(function () {
        for (var s = 0; s < _subOverlays.length; s++) {
          if (_subOverlays[s].groupId === gid) {
            _subOverlays[s].overlay.style.opacity = "";
          }
        }
      });
    }
  }

  function collapseGroup(gid) {
    var overlay = _overlays[gid];
    if (!overlay) return;
    delete _expandedGroups[gid];
    var container = upContainer();
    var type = _groupTypes[gid];

    var els = groupMembers(container, gid);
    for (var i = 0; i < els.length; i++) {
      els[i].classList.remove("mud-change-revealed");
    }

    if (type === "del") {
      overlay.classList.add("mud-overlay-collapsed");
      overlay.classList.remove("mud-change-revealed");
    } else if (type === "mix") {
      var remaining = [];
      for (var j = 0; j < _subOverlays.length; j++) {
        if (_subOverlays[j].groupId === gid) {
          var movedBtn = _subOverlays[j].overlay.querySelector(".mud-expando");
          if (movedBtn) overlay.appendChild(movedBtn);
          _subOverlays[j].overlay.remove();
        } else {
          remaining.push(_subOverlays[j]);
        }
      }
      _subOverlays = remaining;
      delete _suppressedGroups[gid];
      overlay.style.display = "";
    }

    var btn = overlay.querySelector(".mud-expando");
    if (btn) {
      btn.classList.remove("mud-expando-expanded");
      btn.setAttribute("aria-expanded", "false");
      btn.setAttribute("aria-label", "Show deleted content");
    }

    positionOverlays();
  }

  function toggleGroup(gid) {
    if (_expandedGroups[gid]) {
      collapseGroup(gid);
    } else {
      expandGroup(gid);
    }
  }

  // The group's badge, wherever it currently lives (the group overlay, or the
  // first sub-overlay of an expanded mixed group).
  function expandoFor(gid) {
    var overlay = _overlays[gid];
    var btn = overlay ? overlay.querySelector(".mud-expando") : null;
    if (btn) return btn;
    for (var i = 0; i < _subOverlays.length; i++) {
      if (_subOverlays[i].groupId === gid) {
        btn = _subOverlays[i].overlay.querySelector(".mud-expando");
        if (btn) return btn;
      }
    }
    return null;
  }

  // -- Public API ------------------------------------------------------------

  // Collapse every expanded deletion/mixed group.
  function collapseAllChanges() {
    var ids = Object.keys(_expandedGroups);
    for (var i = 0; i < ids.length; i++) collapseGroup(ids[i]);
  }

  // Scroll to the first of `ids` (change IDs, "change-N") present on the
  // page: unfold any folded section around it, center it, and ripple its
  // group's badge. A target hidden as a collapsed deletion scrolls to the
  // badge instead. Accepts a single ID string too.
  function scrollToChange(ids) {
    if (typeof ids === "string") ids = [ids];
    if (!ids || !ids.length) return;
    var first = null;
    for (var i = 0; i < ids.length && !first; i++) {
      first = document.querySelector(
        '[data-change-id="' + ids[i] + '"]:not(.mud-overlay)');
    }
    if (!first) return;

    // Open the folded section first — otherwise the target's missing layout
    // box reads as a collapsed deletion and we'd scroll to the badge. Opening
    // it moves everything below, so reposition before measuring.
    if (window.Mud.folds && window.Mud.folds.reveal(first)) positionOverlays();

    var gid = first.dataset.groupId;
    var btn = gid ? expandoFor(gid) : null;

    if (!isShown(first) && btn) {
      btn.scrollIntoView({ behavior: "smooth", block: "center" });
    } else {
      first.scrollIntoView({ behavior: "smooth", block: "center" });
    }

    if (!btn) return;
    var stale = document.querySelectorAll(".mud-expando.mud-change-active");
    for (var s = 0; s < stale.length; s++) {
      stale[s].classList.remove("mud-change-active");
    }
    void btn.offsetWidth; // restart the animation
    btn.classList.add("mud-change-active");
    btn.addEventListener("animationend", function () {
      btn.classList.remove("mud-change-active");
    }, { once: true });
  }

  // Expand (on) or collapse (off) every deletion/mixed group. mud.js's
  // setClass calls this when the `is-auto-expand-changes` root class flips;
  // the same class decides the initial state in buildOverlays.
  function applyAutoExpandChanges(enabled) {
    for (var gid in _overlays) {
      var type = _groupTypes[gid];
      if (type !== "del" && type !== "mix") continue;
      if (enabled && !_expandedGroups[gid]) {
        expandGroup(gid);
      } else if (!enabled && _expandedGroups[gid]) {
        collapseGroup(gid);
      }
    }
  }

  window.Mud.scrollToChange = scrollToChange;
  window.Mud.collapseAllChanges = collapseAllChanges;
  window.Mud.applyAutoExpandChanges = applyAutoExpandChanges;
  window.Mud.changes.positionOverlays = positionOverlays;

  // -- Startup ---------------------------------------------------------------

  // Build now (the script runs at the end of <body>), and follow every later
  // layout change of the article: Mermaid/Temml rendering, folding, font
  // loading, window width.
  var container = upContainer();
  if (container) {
    buildOverlays();
    if (typeof ResizeObserver !== "undefined") {
      new ResizeObserver(positionOverlays).observe(container);
    }
  }
})();
