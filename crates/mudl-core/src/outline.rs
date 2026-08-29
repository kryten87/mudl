//! Nesting a flat heading list (from [`crate::headings::extract_headings`])
//! into a tree, for the outline sidebar (Phase 9.3).
use crate::headings::OutlineHeading;

/// One node in the nested outline tree: a heading plus every subsequent
/// heading that's deeper than it, up to the next heading at its own level
/// or shallower.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineNode {
    pub heading: OutlineHeading,
    pub children: Vec<OutlineNode>,
}

/// Nests `headings` by strict level comparison: any heading deeper than the
/// current node becomes its child, regardless of how large the level jump
/// is (an `h1` directly followed by an `h3` still nests the `h3` under the
/// `h1` — there's no intermediate `h2` to require).
pub fn build_tree(headings: &[OutlineHeading]) -> Vec<OutlineNode> {
    let mut pos = 0;
    build_siblings(headings, &mut pos, 0)
}

fn build_siblings(headings: &[OutlineHeading], pos: &mut usize, min_level: u8) -> Vec<OutlineNode> {
    let mut nodes = Vec::new();
    while *pos < headings.len() {
        let heading = &headings[*pos];
        if heading.level <= min_level {
            break;
        }
        *pos += 1;
        let children = build_siblings(headings, pos, heading.level);
        nodes.push(OutlineNode {
            heading: heading.clone(),
            children,
        });
    }
    nodes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headings::OutlineTextSegment;

    fn heading(level: u8, text: &str) -> OutlineHeading {
        OutlineHeading {
            level,
            id: text.to_lowercase(),
            segments: vec![OutlineTextSegment::Plain(text.to_string())],
        }
    }

    #[test]
    fn flat_single_level_list() {
        let headings = vec![heading(1, "One"), heading(1, "Two"), heading(1, "Three")];
        let tree = build_tree(&headings);
        assert_eq!(tree.len(), 3);
        assert!(tree.iter().all(|n| n.children.is_empty()));
    }

    #[test]
    fn strictly_increasing_levels() {
        let headings = vec![heading(1, "One"), heading(2, "Two"), heading(3, "Three")];
        let tree = build_tree(&headings);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].heading.id, "one");
        assert_eq!(tree[0].children.len(), 1);
        assert_eq!(tree[0].children[0].heading.id, "two");
        assert_eq!(tree[0].children[0].children.len(), 1);
        assert_eq!(tree[0].children[0].children[0].heading.id, "three");
    }

    #[test]
    fn level_jump_from_h1_to_h3_nests_under_h1() {
        let headings = vec![heading(1, "One"), heading(3, "Three")];
        let tree = build_tree(&headings);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].heading.id, "one");
        assert_eq!(tree[0].children.len(), 1);
        assert_eq!(tree[0].children[0].heading.id, "three");
    }

    #[test]
    fn level_decrease_partway_through() {
        let headings = vec![heading(1, "One"), heading(2, "Two"), heading(1, "Another")];
        let tree = build_tree(&headings);
        assert_eq!(tree.len(), 2);
        assert_eq!(tree[0].heading.id, "one");
        assert_eq!(tree[0].children.len(), 1);
        assert_eq!(tree[0].children[0].heading.id, "two");
        assert_eq!(tree[1].heading.id, "another");
        assert!(tree[1].children.is_empty());
    }

    #[test]
    fn empty_input_is_empty_tree() {
        assert!(build_tree(&[]).is_empty());
    }

    #[test]
    fn single_heading_is_a_single_root_node_with_no_children() {
        let headings = vec![heading(2, "Solo")];
        let tree = build_tree(&headings);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].heading.id, "solo");
        assert!(tree[0].children.is_empty());
    }
}
