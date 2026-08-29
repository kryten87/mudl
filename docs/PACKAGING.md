# Packaging (Phase 12, step 12.2)

`mudl` distributes as a `.deb`, built with [`cargo-deb`](https://github.com/kornelski/cargo-deb).

## Why `.deb` over an AppImage

This is a packaging decision, not an architecture one — nothing in
`crates/` depends on which format wins, and switching later would only mean
changing this document plus the release workflow, per the scope decision in
`docs/IMPLEMENTATION-PLAN.md` §1.

- **`mudl` targets Ubuntu 22.04+ specifically**, not "any Linux." A `.deb`
  is the native, expected format there; an AppImage's main selling point
  (distro-independent portability) isn't a requirement this project has.
- **`mudl-gui` links GTK 3 and WebKit2GTK dynamically.** An AppImage would
  either have to bundle both (WebKit2GTK alone is well over 100MB, and
  bundling a web engine separately from the host's is exactly the kind of
  "re-derive a large, correctness-critical dependency by hand" the
  project's dependency policy (§3) warns against) or still depend on the
  host having a compatible GTK/WebKit — at which point it's carrying
  AppImage's packaging overhead for no portability win. A `.deb` just
  declares `libgtk-3-0`/`libwebkit2gtk-4.1-0` as runtime dependencies and
  lets `apt` resolve them from the distro's own repos.
- **Desktop integration is native to `.deb`.** `dpkg` installs
  `resources/mudl.desktop` straight into `/usr/share/applications` and
  triggers `update-desktop-database` automatically; AppImage requires
  either a separate integration daemon (`appimaged`) or the user manually
  registering the `.desktop` file themselves, which the "Open With" MIME
  association (Phase 11.2) depends on.
- **No custom build tooling.** `cargo-deb` reads ordinary Cargo package
  metadata (`[package.metadata.deb]` in `crates/mudl-cli/Cargo.toml`) and
  produces a package from `cargo build --release`'s own output — no
  separate manifest format, no AppDir staging step to maintain.

## Building a `.deb`

```bash
cargo install cargo-deb   # one-time, if not already installed
cargo build --release
cargo deb -p mudl-cli
```

This produces `target/debian/mudl_<version>-1_<arch>.deb`, containing:

- `usr/bin/mudl` — the release binary
- `usr/share/applications/mudl.desktop` — the desktop entry and
  `text/markdown` MIME association from Phase 11.2
- `usr/share/doc/mudl/copyright` — generated from `LICENSE.md`

Install/uninstall like any other package:

```bash
sudo dpkg -i target/debian/mudl_*.deb
sudo apt remove mudl
```

## Runtime dependencies

`depends = "$auto"` in `[package.metadata.deb]` has `cargo-deb` run `ldd`
against the built binary and translate shared libraries into their owning
apt packages automatically, rather than this file hand-maintaining a
version-pinned list that drifts from what's actually linked. **Build on
the same Ubuntu release you intend to ship for** — the exact dependency
strings differ across releases (e.g. Ubuntu 24.04+ renames some packages
with a `t64` suffix for the 64-bit `time_t` transition), and `$auto`
reflects whatever the build machine's `ldd` reports, not the target release.
The project's CI runner (Ubuntu 22.04, per Phase 0.3) is the canonical
build environment for release artifacts.

## Known gap: application icon

`resources/mudl.desktop`'s `Icon=mudl` currently has no corresponding
`mudl.svg`/`.png` in this repo (noted in that file's own header comment).
Until one exists, installed `.deb`s show a generic fallback icon in the
application menu. Adding one is icon-design work, not a packaging or
architecture change — this file's `assets` list just needs one more entry
(`resources/mudl.svg` → `usr/share/icons/hicolor/scalable/apps/mudl.svg`)
once it exists.
