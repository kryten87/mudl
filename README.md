mudl: A Perfect Markdown Viewer (for Linux)
===============================================================================

`mudl` is a Linux port of [`mud`](https://github.com/joseph/mud), a
macOS-only Markdown viewer. `mudl` reimplements `mud`'s core viewing
experience for Ubuntu 22.04+: GitHub-flavored Markdown rendering, an
instant raw-source view, auto-reload on save, folder/outline sidebars,
themes, dark mode, zoom/word-wrap/readable-column toggles, find, and a
CLI rendering tool — see `docs/IMPLEMENTATION-PLAN.md` for the full build
plan and scope decisions.

`mudl` shows you both sides of the document:

- **Mark Up** renders your Markdown as styled HTML — GitHub-flavored, with
  syntax-highlighted code, Mermaid diagrams, and math.
- **Mark Down** shows the raw source with line numbers.

Hit Space to flip between them. Your scroll position carries over.


## Highlights

- GitHub-flavored Markdown: tables, task lists, strikethrough, autolinks,
  footnotes
- Syntax-highlighted code blocks, Mermaid diagrams, and math expressions
  (all rendered client-side, no network requests)
- Auto-reload every time the file is saved to disk
- Table of contents — document outliner in the sidebar
- Open a folder to get a tree of every Markdown document inside it
- Five color themes — Austere, Blues, Earthy, Riot, and System — with
  dark/bright/auto lighting
- Find (Ctrl+F)
- Zoom, readable column, word wrap, and line number toggles
- Collapsible YAML frontmatter
- GFM alerts and DocC-style aside styles
- A `mudl` command-line tool for rendering Markdown to HTML from a
  terminal or script


## Quick start

### Build from source

Requires Ubuntu 22.04+ (or any Linux with equivalent GTK 3/WebKit2GTK
packages) and a stable Rust toolchain.

```bash
sudo apt install libgtk-3-dev libwebkit2gtk-4.1-dev
cargo build --release
```

The built binary is at `target/release/mudl`. Install it as a `mudl`
command on your `$PATH`:

```bash
target/release/mudl --install-cli    # symlinks into ~/.local/bin/mudl
```

See `docs/PACKAGING.md` for pre-built `.deb` packaging.

### Git hooks

The repo ships a `pre-merge-commit` hook that runs the same checks as CI
(`just ci`: fmt check, clippy, tests) before a local merge commit is created.
Enable it once per clone:

```bash
git config core.hooksPath .githooks
```

### Desktop integration

Copy `resources/mudl.desktop` to `~/.local/share/applications/mudl.desktop`
(per-user) so `mudl` appears in the application menu and as an "Open With"
option for `.md` files, then refresh the desktop database:

```bash
cp resources/mudl.desktop ~/.local/share/applications/mudl.desktop
update-desktop-database ~/.local/share/applications
```

## Command line tool

```bash
mudl file.md                    # Open a file in the app
mudl -u file.md                 # Render to HTML (mark-up view)
mudl -d file.md                 # Render to HTML (mark-down view)
echo "# Hi" | mudl -u           # Pipe stdin to HTML
mudl -u --theme riot file.md    # Pick a theme
mudl -u --standalone file.md    # Inline local images as data URIs
mudl -u -f file.md              # Body-only HTML, no document wrapper
```

Run `mudl --help` for the full flag list.


## Documents

- [`docs/IMPLEMENTATION-PLAN.md`](docs/IMPLEMENTATION-PLAN.md) — the
  step-by-step build plan, architecture, and scope decisions
- [`docs/PACKAGING.md`](docs/PACKAGING.md) — distribution format and how
  to build a `.deb`
- [`docs/examples/`](docs/examples/) — feature-showcase Markdown documents
  used as manual test fixtures and demo content


## Third-party assets

Some of `mudl`'s bundled JS/CSS assets originate from `mud`, the macOS
project this is a reimplementation of, and retain their own upstream
licenses rather than `mudl`'s. In particular, `mud` vendors:

- [highlight.js](https://highlightjs.org/) (BSD-3-Clause) — syntax highlighting
- [Mermaid](https://mermaid.js.org/) (MIT) — diagram rendering
- [Temml](https://temml.org/) (MIT) — TeX-to-MathML math rendering

`mudl` carries these over unchanged (Phase 3, step 3.5), including their
license notices, in `mudl/resources/js/`. See `LICENSE.md` for `mudl`'s
own license.
