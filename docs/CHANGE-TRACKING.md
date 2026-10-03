Change tracking
===============================================================================

`mudl` can track changes to your Markdown documents while you edit them in
another editor. When a document is opened, `mudl` keeps a snapshot of its
content in memory. Every time the file is saved, the reloaded page shows what
changed since that snapshot — insertions, deletions and modifications, down to
the word level, in both Up and Down modes.

Everything is held in memory, per tab, and discarded when the tab closes.
`mudl` never writes to the file, never runs `git`, and never reads anything
except the document it is already showing.

To try it out, open [`examples/change-tracking.md`](examples/change-tracking.md)
and follow its "What to try" list.


## Turning it on

Change tracking is on by default. Toggle it with **View → Show Changes**
(Ctrl-Shift-C). The setting is saved to your preferences and applies to every
window; when it's off, `mudl` still keeps its snapshots, so turning it back on
shows the changes made in the meantime.


## The Changes bar

The Changes bar floats over the document. It shows:

- A **badge** with the number of change groups. Green means insertions only,
  red deletions only, orange a mix of both, and gray no changes.
- A **status button** such as "3 changes since 10:19am". Click it to open
  the **Changes since…** popover (see [Choosing a waypoint](#choosing-a-waypoint)).
- **Previous / Next** buttons to step through the change groups, wrapping
  around at either end. Previous appears once there are two or more groups.
- An **Accept** button that accepts all current changes.

Where the bar sits is set by the `ui_floating_controls_position` preference
(`bottom-center` by default; also `top-left`, `top-center`, `top-right`,
`bottom-left` and `bottom-right`).


## How changes appear

### Up mode

Changes appear as tinted overlays on the rendered document, each change group
with a numbered badge:

- **Green overlays** mark inserted content.
- **Red overlays** mark deleted content. Click the badge to reveal the
  deleted text, struck through.
- **Orange overlays** mark mixed changes (something deleted and replaced).
  Click the badge to see the old version in red above the new one in green.
  Where an old and new block are similar enough, the individual words that
  changed are highlighted.

Only the rows of a table or the items of a list that changed are marked, not
the whole table or list. A code block with changes shows line-level
differences, and is left without syntax highlighting while it has them.

If a change is inside a folded heading, navigating to it unfolds the section
first.


### Down mode

In Down mode, changes appear on the raw source:

- Inserted lines are tinted green.
- Deleted lines are put back in place, tinted red, with "–" instead of a line
  number.
- Word-level markers highlight the specific words that changed within a line.


### Exports

Change markup never leaves the app: **Open In Browser**, **Print**, and the
`mudl -u` / `mudl -d` command-line renderer all produce the document without
overlays, badges or deleted text.


## The Changes sidebar

Open the sidebar (**View → Hide Sidebar** toggles it, Ctrl-Shift-S) and pick
**Changes** in the switcher at its top to list every change group. Each row
shows the group's numbered badge and a summary of its changes, such as
"+ Added a new paragraph" or "− Removed the old heading"; long runs of the same
kind are condensed to "and N more insertions".

Click a row to scroll to that change and select it. When change tracking is
off, the pane offers a **Show changes** button instead.


## Choosing a waypoint

A *waypoint* is a snapshot to compare against. By default `mudl` compares
against the last time you accepted changes — or, if you haven't accepted yet,
against the content when the document was opened.

Click the status button on the Changes bar to open the **Changes since…**
popover, which lists the available waypoints:

- **Milestones**
  - **since last accepted** — the most recent accept point
  - **since document opened** — the content when the file was first loaded
- **Recent** — automatic snapshots taken as the file is saved, such as
  "since 1 minute ago" or "since 5 minutes ago". Saves within the same minute
  share a snapshot, and only the ten most recent are kept. Snapshots that
  would show no changes are left out.

Each row shows how many changes it would display. Pick one to compare against
it; the active waypoint is checked.

Edits that don't change the document's content — re-wrapping a paragraph,
renumbering a list, re-padding a table's pipes, adding or editing a comment —
aren't counted as changes and don't create a snapshot.


## Accepting changes

Click **Accept** on the Changes bar, or choose **Edit → Accept Changes**, to
accept all current changes. The current content becomes the new baseline: the
change count drops to zero, and later edits are compared against it.

Accepting is local and lightweight. It doesn't modify the file on disk — it
just moves the comparison point forward.


## Keyboard shortcuts

| Shortcut      | Action                        |
| ------------- | ----------------------------- |
| Ctrl-Shift-C  | Show / hide changes           |
| Ctrl-.        | Next change                   |
| Ctrl-,        | Previous change               |
| Ctrl-Shift-S  | Show / hide sidebar           |

**Edit → Accept Changes** has no shortcut.


## Preferences

These live in `~/.config/mudl/preferences`, a plain `key = value` file. The
app writes `changes_enabled` itself when you use **View → Show Changes**; edit
the others by hand, then restart `mudl`.

| Key | Default | Meaning |
| --- | --- | --- |
| `changes_enabled` | `true` | Master switch. When `false`, no change markup is shown (snapshots are still kept). |
| `changes_show_inline_deletions` | `false` | In Up mode, also show deleted words, struck through, inside an edited block's new version. |
| `changes_auto_expand_groups` | `false` | Start deletion and mixed groups expanded instead of collapsed. |
| `changes_word_diff_threshold` | `0.25` | How similar (0.0–1.0) an old and new block must be for word-level highlights. Below it, the edit is shown as a whole old block and a whole new block. |
| `sidebar_pane` | `outline` | Set to `changes` to open the sidebar on the Changes pane. |

For example:

```
changes_auto_expand_groups = true
changes_word_diff_threshold = 0.4
```


## Differences from `mud`

- There are no Git waypoints. `mud`'s direct-distribution build can compare
  against the git index and recent commits; `mudl` deliberately does not run
  `git` inside a document's directory (see finding 6 in
  [`SECURITY.md`](SECURITY.md)).
- Next Change and Previous Change have shortcuts (Ctrl-. and Ctrl-,).
- There is no toolbar Changes button; use the View menu or Ctrl-Shift-C.
