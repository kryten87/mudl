//! Folder index data: a pure walk of a directory tree down to its Markdown
//! files (for the sidebar's folder pane, Phase 9), plus rendering that tree
//! as a nested Markdown link list (`mud`'s `FolderIndex.markdown(for:)`
//! equivalent).
use std::io;
use std::path::Path;

use crate::template::percent_encode;

/// One entry from an injected directory listing. Mirrors just enough of
/// `std::fs::DirEntry` for `walk` to make its pure decisions (recurse, skip,
/// or include as a file) without ever touching the filesystem itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    pub is_symlink: bool,
}

/// A single entry in a folder [`Tree`], after hidden entries, symlinked
/// directories, and non-Markdown files have already been filtered out and
/// empty directory branches pruned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeNode {
    File {
        name: String,
    },
    Directory {
        name: String,
        children: Vec<TreeNode>,
    },
}

/// The result of [`walk`]: the filtered/pruned tree rooted at the walked
/// directory, plus whether the `limit` on total files visited was hit
/// (in which case the tree is a partial view, not the full directory).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tree {
    pub children: Vec<TreeNode>,
    pub truncated: bool,
}

struct WalkState<'a> {
    list_dir: &'a dyn Fn(&Path) -> io::Result<Vec<DirEntry>>,
    limit: usize,
    visited: usize,
    truncated: bool,
}

/// Recursively builds a [`Tree`] of Markdown files (`.md`/`.markdown`/`.mkd`)
/// under `root`, given an injected directory-listing function — no
/// filesystem access happens in this function itself, which is what makes it
/// exhaustively testable with a scripted fake.
///
/// Hidden entries (dotfiles) and symlinked directories are skipped
/// entirely (not descended into, not included). Directories with no
/// Markdown files anywhere beneath them are pruned from the result rather
/// than appearing as empty nodes. At most `limit` files are visited in
/// total; if more exist, `Tree::truncated` is `true` and the remainder is
/// left out.
pub fn walk(
    root: &Path,
    list_dir: &dyn Fn(&Path) -> io::Result<Vec<DirEntry>>,
    limit: usize,
) -> Tree {
    let mut state = WalkState {
        list_dir,
        limit,
        visited: 0,
        truncated: false,
    };
    let children = walk_dir(root, &mut state);
    Tree {
        children,
        truncated: state.truncated,
    }
}

fn walk_dir(path: &Path, state: &mut WalkState<'_>) -> Vec<TreeNode> {
    if state.truncated {
        return Vec::new();
    }

    let Ok(entries) = (state.list_dir)(path) else {
        return Vec::new();
    };

    let mut nodes = Vec::new();
    for entry in entries {
        if state.truncated {
            break;
        }
        if is_hidden(&entry.name) {
            continue;
        }

        if entry.is_dir {
            if entry.is_symlink {
                continue;
            }
            let children = walk_dir(&path.join(&entry.name), state);
            if !children.is_empty() {
                nodes.push(TreeNode::Directory {
                    name: entry.name,
                    children,
                });
            }
        } else if is_markdown_file(&entry.name) {
            if state.visited >= state.limit {
                state.truncated = true;
                break;
            }
            state.visited += 1;
            nodes.push(TreeNode::File { name: entry.name });
        }
    }
    nodes
}

fn is_hidden(name: &str) -> bool {
    name.starts_with('.')
}

fn is_markdown_file(name: &str) -> bool {
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase());
    matches!(ext.as_deref(), Some("md") | Some("markdown") | Some("mkd"))
}

/// Renders a [`Tree`] as a nested Markdown link list, one `- [name](target)`
/// item per file and one `- name` item per directory, indented two spaces
/// per nesting level. Display names are Markdown-escaped (so a filename
/// like `[note].md` doesn't get parsed as link syntax); link targets are the
/// file's path relative to the walked root, percent-encoded.
pub fn render_index_markdown(tree: &Tree) -> String {
    let mut out = String::new();
    render_nodes(&tree.children, Path::new(""), 0, &mut out);
    out
}

fn render_nodes(nodes: &[TreeNode], prefix: &Path, depth: usize, out: &mut String) {
    let indent = "  ".repeat(depth);
    for node in nodes {
        match node {
            TreeNode::File { name } => {
                let display = escape_markdown_text(name);
                let target = prefix.join(name);
                let target = percent_encode(&target.to_string_lossy());
                out.push_str(&format!("{indent}- [{display}]({target})\n"));
            }
            TreeNode::Directory { name, children } => {
                let display = escape_markdown_text(name);
                out.push_str(&format!("{indent}- {display}\n"));
                render_nodes(children, &prefix.join(name), depth + 1, out);
            }
        }
    }
}

/// Backslash-escapes the characters that could make a display name be
/// mistaken for Markdown syntax inside a `[text](target)` link: brackets
/// (which would prematurely close/reopen the link text), emphasis
/// delimiters, backtick (inline code), and a literal backslash itself
/// (which must be escaped first so it isn't read as escaping the character
/// after it).
fn escape_markdown_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | '[' | ']' | '*' | '_' | '`') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod walk_tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn fake_list_dir(
        entries: HashMap<PathBuf, Vec<DirEntry>>,
    ) -> impl Fn(&Path) -> io::Result<Vec<DirEntry>> {
        move |path: &Path| {
            entries
                .get(path)
                .cloned()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such directory"))
        }
    }

    fn file(name: &str) -> DirEntry {
        DirEntry {
            name: name.to_string(),
            is_dir: false,
            is_symlink: false,
        }
    }

    fn dir(name: &str) -> DirEntry {
        DirEntry {
            name: name.to_string(),
            is_dir: true,
            is_symlink: false,
        }
    }

    fn symlinked_dir(name: &str) -> DirEntry {
        DirEntry {
            name: name.to_string(),
            is_dir: true,
            is_symlink: true,
        }
    }

    #[test]
    fn flat_directory_of_markdown_files() {
        let root = PathBuf::from("/root");
        let mut map = HashMap::new();
        map.insert(
            root.clone(),
            vec![file("a.md"), file("b.markdown"), file("c.mkd")],
        );
        let tree = walk(&root, &fake_list_dir(map), 100);
        assert_eq!(
            tree.children,
            vec![
                TreeNode::File {
                    name: "a.md".to_string()
                },
                TreeNode::File {
                    name: "b.markdown".to_string()
                },
                TreeNode::File {
                    name: "c.mkd".to_string()
                },
            ]
        );
        assert!(!tree.truncated);
    }

    #[test]
    fn nested_directories() {
        let root = PathBuf::from("/root");
        let mut map = HashMap::new();
        map.insert(root.clone(), vec![dir("sub"), file("top.md")]);
        map.insert(root.join("sub"), vec![file("nested.md")]);
        let tree = walk(&root, &fake_list_dir(map), 100);
        assert_eq!(
            tree.children,
            vec![
                TreeNode::Directory {
                    name: "sub".to_string(),
                    children: vec![TreeNode::File {
                        name: "nested.md".to_string()
                    }],
                },
                TreeNode::File {
                    name: "top.md".to_string()
                },
            ]
        );
    }

    #[test]
    fn empty_directory_is_empty_tree() {
        let root = PathBuf::from("/root");
        let mut map = HashMap::new();
        map.insert(root.clone(), vec![]);
        let tree = walk(&root, &fake_list_dir(map), 100);
        assert!(tree.children.is_empty());
        assert!(!tree.truncated);
    }

    #[test]
    fn directory_with_only_non_markdown_files_is_empty_tree() {
        let root = PathBuf::from("/root");
        let mut map = HashMap::new();
        map.insert(root.clone(), vec![file("readme.txt"), file("image.png")]);
        let tree = walk(&root, &fake_list_dir(map), 100);
        assert!(tree.children.is_empty());
    }

    #[test]
    fn hidden_files_and_directories_excluded() {
        let root = PathBuf::from("/root");
        let mut map = HashMap::new();
        map.insert(
            root.clone(),
            vec![file(".hidden.md"), dir(".git"), file("visible.md")],
        );
        map.insert(root.join(".git"), vec![file("would-be-visible.md")]);
        let tree = walk(&root, &fake_list_dir(map), 100);
        assert_eq!(
            tree.children,
            vec![TreeNode::File {
                name: "visible.md".to_string()
            }]
        );
    }

    #[test]
    fn symlinked_subdirectory_excluded() {
        let root = PathBuf::from("/root");
        let mut map = HashMap::new();
        map.insert(root.clone(), vec![symlinked_dir("link"), file("real.md")]);
        map.insert(root.join("link"), vec![file("elsewhere.md")]);
        let tree = walk(&root, &fake_list_dir(map), 100);
        assert_eq!(
            tree.children,
            vec![TreeNode::File {
                name: "real.md".to_string()
            }]
        );
    }

    #[test]
    fn exactly_at_limit_is_not_truncated() {
        let root = PathBuf::from("/root");
        let mut map = HashMap::new();
        map.insert(root.clone(), vec![file("a.md"), file("b.md"), file("c.md")]);
        let tree = walk(&root, &fake_list_dir(map), 3);
        assert_eq!(tree.children.len(), 3);
        assert!(!tree.truncated);
    }

    #[test]
    fn one_over_limit_is_truncated() {
        let root = PathBuf::from("/root");
        let mut map = HashMap::new();
        map.insert(
            root.clone(),
            vec![file("a.md"), file("b.md"), file("c.md"), file("d.md")],
        );
        let tree = walk(&root, &fake_list_dir(map), 3);
        assert_eq!(tree.children.len(), 3);
        assert!(tree.truncated);
    }
}

#[cfg(test)]
mod render_index_markdown_tests {
    use super::*;

    #[test]
    fn single_file() {
        let tree = Tree {
            children: vec![TreeNode::File {
                name: "note.md".to_string(),
            }],
            truncated: false,
        };
        assert_eq!(render_index_markdown(&tree), "- [note.md](note.md)\n");
    }

    #[test]
    fn nested_structure_with_correct_indentation() {
        let tree = Tree {
            children: vec![
                TreeNode::Directory {
                    name: "sub".to_string(),
                    children: vec![TreeNode::File {
                        name: "nested.md".to_string(),
                    }],
                },
                TreeNode::File {
                    name: "top.md".to_string(),
                },
            ],
            truncated: false,
        };
        assert_eq!(
            render_index_markdown(&tree),
            "- sub\n  - [nested.md](sub/nested.md)\n- [top.md](top.md)\n"
        );
    }

    #[test]
    fn filename_with_markdown_special_characters_is_escaped_in_display_text() {
        let tree = Tree {
            children: vec![TreeNode::File {
                name: "[note]*.md".to_string(),
            }],
            truncated: false,
        };
        assert_eq!(
            render_index_markdown(&tree),
            "- [\\[note\\]\\*.md](%5Bnote%5D%2A.md)\n"
        );
        // The period isn't Markdown-special in this position, so it's left
        // unescaped in the display text and unencoded in the target.
    }

    #[test]
    fn path_with_spaces_is_percent_encoded_in_link_target() {
        let tree = Tree {
            children: vec![TreeNode::Directory {
                name: "my docs".to_string(),
                children: vec![TreeNode::File {
                    name: "my note.md".to_string(),
                }],
            }],
            truncated: false,
        };
        assert_eq!(
            render_index_markdown(&tree),
            "- my docs\n  - [my note.md](my%20docs/my%20note.md)\n"
        );
    }
}
