Change Tracking
===============================================================================

mudl marks what changed in a document since a point in time (a *waypoint*):
when the file is edited while it's open, the next reload shows the changes.
This page is something to edit while it's open, so you can watch that happen.

The Changes bar, the Changes sidebar pane and the "Changes since…" popover
are covered in the user guide, [`../CHANGE-TRACKING.md`](../CHANGE-TRACKING.md).


## What to try

Open this file in mudl, then edit it in another editor and save.

1. **Change one word** in the paragraph below. An orange overlay with badge 1
   covers it, and the new word is highlighted. Clicking the badge shows the
   old paragraph struck through in red above the new one in green.
2. **Add a paragraph.** A green overlay marks it; its badge can't be clicked.
3. **Delete a paragraph.** A red badge sits in the gap where it was. Clicking
   it reveals the deleted text struck through.
4. **Delete the middle table row, or the second list item.** Only that row or
   item is a change. Delete the whole list and no empty list box is left
   behind while it's collapsed.
5. **Edit a line in the Rust block.** The block shows line-level changes (and
   is left unhighlighted while it has them).
6. **Switch to Down mode.** Changed lines are tinted, deleted lines show "–"
   in place of a line number, and changed words are marked inside the
   highlighted source.
7. **Turn on foldable headings and fold this section**, then navigate to a
   change in it: the section unfolds first.
8. **Print.** Overlays, badges and deletions don't print.

`change-tracking-word-markers.html`, next to this file, is a self-test for the
Down mode word markers: open it directly in a browser.


## Material to edit

The quick brown fox jumps over the lazy dog, and then naps in the afternoon
sun while the dog keeps watch.

A second paragraph, here only so there's something to delete.

| Animal | Sound |
| ------ | ----- |
| Fox    | Yip   |
| Dog    | Woof  |
| Cat    | Meow  |

- Alpha
- Beta
- Gamma

```rust
fn main() {
    let greeting = "hello";
    println!("{greeting}, world");
}
```
