---
title: Front-matter showcase
author: Jane Doe
date: 2026-04-08
draft: false
version: "2.0"
category: 'Documentation'
tags: [markdown, yaml, metadata, front-matter]
keywords:
  - static sites
  - hugo
  - jekyll
layout: post
permalink: /blog/front-matter-showcase
description: >
  A comprehensive test of YAML front-matter rendering
  in mudl's Up and Down modes.
body: |
  This is a multi-line literal block scalar.
  It preserves newlines exactly as written.
  Even this third line.
config:
  toc: true
  syntax_highlight: true
  nested:
    depth: 3
    enabled: true
# This comment should not appear as a key
empty_value:
---

Front-matter showcase
===============================================================================

This document exercises YAML front-matter handling in both Up mode (rendered)
and Down mode (syntax-highlighted source).


## What to verify

### Up mode

- The front-matter block appears as a **collapsed `<details>` element** above
  this heading.
- Clicking "Front Matter" expands it to reveal a **key-value table**.
- **Scalar values** (`title`, `author`, `date`, `draft`, `version`) display as
  plain text. Quoted values (`version`, `category`) show their quotes.
- **Inline array** (`tags`) displays as a comma-separated list.
- **Block array** (`keywords`) displays in a `<pre>` block.
- **Folded scalar** (`description`, with `>`) displays in a `<pre>` block.
- **Literal scalar** (`body`, with `|`) displays in a `<pre>` block.
- **Nested mapping** (`config`) displays in a `<pre>` block.
- **Empty value** (`empty_value`) displays as an empty cell.
- The body content below renders normally — headings, paragraphs, code blocks,
  etc. are unaffected.


### Down mode

- The `---` delimiters appear with **dimmed opacity** (like code fences).
- YAML content lines have **syntax highlighting** (keys, strings, booleans,
  numbers in distinct colors).
- The front-matter region has a **code-block background** tint.
- Line numbers are **continuous** across front-matter and body.
- The rest of the document has normal Markdown syntax highlighting.


## Body content

The content below confirms that normal Markdown rendering is unaffected by
front-matter.


### Code block

```yaml
# This is a YAML code block in the body — not front-matter.
title: Not front-matter
tags: [this, is, body, content]
```


### Thematic break

The `---` below is a thematic break, not a front-matter delimiter:


-------------------------------------------------------------------------------


Text continues after the break.


### Other elements

> A blockquote to verify nothing is disrupted.

- List item one
- List item two
- List item three

| Key     | Value            |
| ------- | ---------------- |
| table   | renders normally |
| despite | front-matter     |
