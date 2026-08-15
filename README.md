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
