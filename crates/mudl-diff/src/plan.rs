//! Assembling `BlockMatch`es into a renderable change plan (ported from
//! `mud`'s `Core/Sources/Diff/ChangePlan.swift`, Phase 13.6).
//!
//! Scope note: the Swift original also ports each code-block pair's
//! per-line diff through `CodeBlockDiff` (its own ~300-line module, minting
//! a change/group ID per changed-line cluster) and caches the *whole
//! parse-to-plan* pipeline keyed on raw old/new source text. Neither is a
//! named deliverable of this plan's step 13.6, whose tests only require
//! ID minting, grouping, and Mermaid/math-excluded positional code-block
//! pairing — so a code-block pair's changed lines are reused from
//! [`crate::line::diff`] rather than a bespoke line-diff module, and
//! [`PlanCache`] is keyed on the already-matched `BlockMatch` slice (the
//! layer this step actually receives), not on raw source text — the
//! source-to-`LeafBlock` walk that would make a text-keyed cache possible
//! is Phase 13.8's concern.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use crate::block::{BlockKind, BlockMatch, LeafBlock};
use crate::line::LineChange;
use crate::word::{self, WordSpan};

/// A changed block with its minted change ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub change_id: String,
    pub block: LeafBlock,
}

/// A deletion and insertion paired within a gap. `word_spans` is `None`
/// when the word-level diff fails the significance threshold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pair {
    pub deletion: Change,
    pub insertion: Change,
    pub word_spans: Option<Vec<WordSpan>>,
}

/// A code-block pair with a line-level diff (`None` when the two blocks'
/// lines are identical).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeBlockPair {
    pub deletion: Change,
    pub insertion: Change,
    pub line_diff: Option<Vec<LineChange>>,
}

/// One insertion position in a gap, in match order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InsertionSlot {
    Block(Change),
    CodeBlockPair(CodeBlockPair),
}

/// A run of deletions and insertions between two unchanged anchors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gap {
    /// Block-level deletions in match order (code-pair deletions are
    /// consumed by their pair and excluded here).
    pub deletions: Vec<Change>,
    /// Insertions in match order, with paired code blocks in place.
    pub insertion_slots: Vec<InsertionSlot>,
    /// Positional pairs among `deletions` and the block-level slots.
    pub pairs: Vec<Pair>,
    /// The unchanged block before this gap; `None` at document start.
    pub preceding_anchor: Option<LeafBlock>,
    /// The unchanged block after this gap; `None` at document end.
    pub following_anchor: Option<LeafBlock>,
}

/// Position of a change within its group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupPos {
    First,
    Middle,
    Last,
    Sole,
}

/// A group's content type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupType {
    Ins,
    Del,
    Mix,
}

/// Describes a change's membership in a consecutive group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupInfo {
    pub group_id: String,
    pub group_pos: GroupPos,
    pub group_index: usize,
    pub group_type: GroupType,
}

/// The single diff pass shared by every change-tracking consumer: mints
/// `change-N` IDs in match order, pairs code blocks positionally
/// (excluding Mermaid/math), pairs remaining deletions/insertions
/// positionally for word-level diffing, and groups adjacent changes into
/// `group-N` badges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangePlan {
    pub gaps: Vec<Gap>,
    pub group_info: HashMap<String, GroupInfo>,
    pub paired_change_id: HashMap<String, String>,
    pub word_spans: HashMap<String, Vec<WordSpan>>,
}

const UNPAIRABLE_LANGUAGES: [&str; 2] = ["mermaid", "math"];

fn is_unpairable(language: &Option<String>) -> bool {
    match language {
        Some(lang) => UNPAIRABLE_LANGUAGES.contains(&lang.to_lowercase().as_str()),
        None => false,
    }
}

struct IdMinter(u64);

impl IdMinter {
    fn next(&mut self) -> String {
        self.0 += 1;
        format!("change-{}", self.0)
    }
}

impl ChangePlan {
    /// Runs the pass over [`crate::block::match_blocks`] output.
    pub fn build(matches: &[BlockMatch], word_diff_threshold: f64) -> ChangePlan {
        let mut ids = IdMinter(0);
        let mut gaps: Vec<Gap> = Vec::new();
        let mut pending_deletions: Vec<Change> = Vec::new();
        let mut pending_insertions: Vec<Change> = Vec::new();
        let mut preceding_anchor: Option<LeafBlock> = None;

        for m in matches {
            match m {
                BlockMatch::Deleted { old } => pending_deletions.push(Change {
                    change_id: ids.next(),
                    block: old.clone(),
                }),
                BlockMatch::Inserted { new } => pending_insertions.push(Change {
                    change_id: ids.next(),
                    block: new.clone(),
                }),
                BlockMatch::Unchanged { new, .. } => {
                    close_gap(
                        &mut pending_deletions,
                        &mut pending_insertions,
                        &preceding_anchor,
                        Some(new.clone()),
                        &mut gaps,
                        word_diff_threshold,
                    );
                    preceding_anchor = Some(new.clone());
                }
            }
        }
        close_gap(
            &mut pending_deletions,
            &mut pending_insertions,
            &preceding_anchor,
            None,
            &mut gaps,
            word_diff_threshold,
        );

        let group_info = build_groups(&gaps);

        let mut paired_change_id: HashMap<String, String> = HashMap::new();
        let mut word_spans: HashMap<String, Vec<WordSpan>> = HashMap::new();
        for gap in &gaps {
            for pair in &gap.pairs {
                paired_change_id.insert(
                    pair.deletion.change_id.clone(),
                    pair.insertion.change_id.clone(),
                );
                paired_change_id.insert(
                    pair.insertion.change_id.clone(),
                    pair.deletion.change_id.clone(),
                );
                if let Some(spans) = &pair.word_spans {
                    word_spans.insert(pair.deletion.change_id.clone(), spans.clone());
                    word_spans.insert(pair.insertion.change_id.clone(), spans.clone());
                }
            }
        }

        ChangePlan {
            gaps,
            group_info,
            paired_change_id,
            word_spans,
        }
    }
}

/// Closes the current gap (if it has any deletions/insertions), minting
/// code-block pairs and positional word-diff pairs, then appends it to
/// `gaps` and clears the pending buffers.
#[allow(clippy::too_many_arguments)]
fn close_gap(
    pending_deletions: &mut Vec<Change>,
    pending_insertions: &mut Vec<Change>,
    preceding_anchor: &Option<LeafBlock>,
    following_anchor: Option<LeafBlock>,
    gaps: &mut Vec<Gap>,
    word_diff_threshold: f64,
) {
    if pending_deletions.is_empty() && pending_insertions.is_empty() {
        return;
    }

    let deleted_code: Vec<usize> = pending_deletions
        .iter()
        .enumerate()
        .filter(|(_, c)| c.block.kind == BlockKind::CodeBlock)
        .map(|(i, _)| i)
        .collect();
    let inserted_code: Vec<usize> = pending_insertions
        .iter()
        .enumerate()
        .filter(|(_, c)| c.block.kind == BlockKind::CodeBlock)
        .map(|(i, _)| i)
        .collect();

    let mut consumed_deletions: Vec<usize> = Vec::new();
    let mut code_pairs: HashMap<usize, CodeBlockPair> = HashMap::new(); // insertion index -> pair

    for (&del_idx, &ins_idx) in deleted_code.iter().zip(inserted_code.iter()) {
        let del = &pending_deletions[del_idx];
        let ins = &pending_insertions[ins_idx];
        if is_unpairable(&del.block.language) || is_unpairable(&ins.block.language) {
            continue;
        }
        let old_lines: Vec<&str> = del.block.source_text.lines().collect();
        let new_lines: Vec<&str> = ins.block.source_text.lines().collect();
        let line_diff = crate::line::diff(&old_lines, &new_lines);
        code_pairs.insert(
            ins_idx,
            CodeBlockPair {
                deletion: del.clone(),
                insertion: ins.clone(),
                line_diff,
            },
        );
        consumed_deletions.push(del_idx);
    }

    let deletions: Vec<Change> = pending_deletions
        .iter()
        .enumerate()
        .filter(|(i, _)| !consumed_deletions.contains(i))
        .map(|(_, c)| c.clone())
        .collect();

    let mut insertion_slots: Vec<InsertionSlot> = Vec::new();
    let mut block_insertions: Vec<Change> = Vec::new();
    for (i, ins) in pending_insertions.iter().enumerate() {
        if let Some(pair) = code_pairs.remove(&i) {
            insertion_slots.push(InsertionSlot::CodeBlockPair(pair));
        } else {
            insertion_slots.push(InsertionSlot::Block(ins.clone()));
            block_insertions.push(ins.clone());
        }
    }

    let mut pairs: Vec<Pair> = Vec::new();
    for (del, ins) in deletions.iter().zip(block_insertions.iter()) {
        let spans = word::diff(&del.block.source_text, &ins.block.source_text);
        let significant = word::has_significant_changes(&spans, word_diff_threshold);
        pairs.push(Pair {
            deletion: del.clone(),
            insertion: ins.clone(),
            word_spans: if significant { Some(spans) } else { None },
        });
    }

    gaps.push(Gap {
        deletions,
        insertion_slots,
        pairs,
        preceding_anchor: preceding_anchor.clone(),
        following_anchor,
    });

    pending_deletions.clear();
    pending_insertions.clear();
}

/// The grouping pass, in document order: every gap starts a new group; a
/// code-block pair closes the current group and forms its own mixed group
/// from its (block-level) deletion/insertion change IDs.
fn build_groups(gaps: &[Gap]) -> HashMap<String, GroupInfo> {
    let mut group_info: HashMap<String, GroupInfo> = HashMap::new();
    let mut group_counter = 0usize;
    let mut current_group: Vec<(String, bool)> = Vec::new();

    fn finalize(
        current_group: &mut Vec<(String, bool)>,
        group_counter: &mut usize,
        group_info: &mut HashMap<String, GroupInfo>,
    ) {
        if current_group.is_empty() {
            return;
        }
        *group_counter += 1;
        let group_id = format!("group-{}", group_counter);
        let has_del = current_group.iter().any(|(_, is_del)| *is_del);
        let has_ins = current_group.iter().any(|(_, is_del)| !*is_del);
        let group_type = match (has_del, has_ins) {
            (true, true) => GroupType::Mix,
            (false, true) => GroupType::Ins,
            _ => GroupType::Del,
        };
        let count = current_group.len();
        for (i, (change_id, _)) in current_group.iter().enumerate() {
            let pos = if count == 1 {
                GroupPos::Sole
            } else if i == 0 {
                GroupPos::First
            } else if i == count - 1 {
                GroupPos::Last
            } else {
                GroupPos::Middle
            };
            group_info.insert(
                change_id.clone(),
                GroupInfo {
                    group_id: group_id.clone(),
                    group_pos: pos,
                    group_index: *group_counter,
                    group_type,
                },
            );
        }
        current_group.clear();
    }

    for gap in gaps {
        finalize(&mut current_group, &mut group_counter, &mut group_info);
        for del in &gap.deletions {
            current_group.push((del.change_id.clone(), true));
        }
        for slot in &gap.insertion_slots {
            match slot {
                InsertionSlot::Block(change) => {
                    current_group.push((change.change_id.clone(), false));
                }
                InsertionSlot::CodeBlockPair(pair) => {
                    finalize(&mut current_group, &mut group_counter, &mut group_info);
                    group_counter += 1;
                    let group_id = format!("group-{}", group_counter);
                    group_info.insert(
                        pair.deletion.change_id.clone(),
                        GroupInfo {
                            group_id: group_id.clone(),
                            group_pos: GroupPos::First,
                            group_index: group_counter,
                            group_type: GroupType::Mix,
                        },
                    );
                    group_info.insert(
                        pair.insertion.change_id.clone(),
                        GroupInfo {
                            group_id,
                            group_pos: GroupPos::Last,
                            group_index: group_counter,
                            group_type: GroupType::Mix,
                        },
                    );
                }
            }
        }
    }
    finalize(&mut current_group, &mut group_counter, &mut group_info);

    group_info
}

/// A small fixed-size LRU cache over [`ChangePlan::build`], keyed by the
/// matched blocks and threshold it was built from (see the module-level
/// scope note for why this is keyed on `&[BlockMatch]` rather than raw
/// source text). A `HashMap` + manual eviction is enough for an 8-entry
/// cache — no `lru` crate needed.
pub struct PlanCache {
    entries: Vec<(u64, ChangePlan)>,
    capacity: usize,
}

impl PlanCache {
    pub fn new(capacity: usize) -> Self {
        PlanCache {
            entries: Vec::new(),
            capacity,
        }
    }

    /// Returns the plan for `(matches, threshold)`, computing it only on a
    /// cache miss. A hit moves the entry to the front (most-recently-used).
    pub fn get_or_build(&mut self, matches: &[BlockMatch], threshold: f64) -> ChangePlan {
        let key = Self::cache_key(matches, threshold);
        if let Some(pos) = self.entries.iter().position(|(k, _)| *k == key) {
            let entry = self.entries.remove(pos);
            let plan = entry.1.clone();
            self.entries.insert(0, (key, plan.clone()));
            return plan;
        }

        let plan = ChangePlan::build(matches, threshold);
        self.entries.insert(0, (key, plan.clone()));
        if self.entries.len() > self.capacity {
            self.entries.truncate(self.capacity);
        }
        plan
    }

    fn cache_key(matches: &[BlockMatch], threshold: f64) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        matches.hash(&mut hasher);
        threshold.to_bits().hash(&mut hasher);
        hasher.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prose(text: &str) -> LeafBlock {
        LeafBlock {
            kind: BlockKind::Prose,
            source_line: 1,
            source_text: text.to_string(),
            language: None,
            range: None,
        }
    }

    fn code(text: &str, language: Option<&str>) -> LeafBlock {
        LeafBlock {
            kind: BlockKind::CodeBlock,
            source_line: 1,
            source_text: text.to_string(),
            language: language.map(str::to_string),
            range: None,
        }
    }

    fn deleted(block: LeafBlock) -> BlockMatch {
        BlockMatch::Deleted { old: block }
    }

    fn inserted(block: LeafBlock) -> BlockMatch {
        BlockMatch::Inserted { new: block }
    }

    fn unchanged(text: &str) -> BlockMatch {
        BlockMatch::Unchanged {
            old: prose(text),
            new: prose(text),
        }
    }

    // --- ID minting / stability ---

    #[test]
    fn change_ids_mint_in_match_order() {
        let matches = vec![
            deleted(prose("old paragraph one")),
            inserted(prose("new paragraph one")),
        ];
        let plan = ChangePlan::build(&matches, 0.25);
        assert_eq!(plan.gaps[0].deletions[0].change_id, "change-1");
    }

    #[test]
    fn ids_are_stable_across_repeated_calls_with_the_same_input() {
        let matches = vec![
            unchanged("alpha"),
            deleted(prose("old middle")),
            inserted(prose("new middle")),
            unchanged("gamma"),
        ];
        let first = ChangePlan::build(&matches, 0.25);
        let second = ChangePlan::build(&matches, 0.25);
        assert_eq!(first, second);
    }

    // --- grouping ---

    #[test]
    fn adjacent_changes_in_one_gap_form_a_single_mixed_group() {
        let matches = vec![
            unchanged("alpha"),
            deleted(prose("old one")),
            deleted(prose("old two")),
            inserted(prose("new one")),
            inserted(prose("new two")),
            unchanged("omega"),
        ];
        let plan = ChangePlan::build(&matches, 0.25);
        let ids: Vec<&str> = plan.gaps[0]
            .deletions
            .iter()
            .map(|c| c.change_id.as_str())
            .chain(plan.gaps[0].insertion_slots.iter().map(|s| match s {
                InsertionSlot::Block(c) => c.change_id.as_str(),
                InsertionSlot::CodeBlockPair(_) => unreachable!(),
            }))
            .collect();
        assert_eq!(ids.len(), 4);
        let group_ids: Vec<&str> = ids
            .iter()
            .map(|id| plan.group_info[*id].group_id.as_str())
            .collect();
        assert!(group_ids.iter().all(|g| *g == group_ids[0]));
        assert_eq!(plan.group_info[ids[0]].group_type, GroupType::Mix);
        assert_eq!(plan.group_info[ids[0]].group_pos, GroupPos::First);
        assert_eq!(plan.group_info[ids[3]].group_pos, GroupPos::Last);
    }

    #[test]
    fn separate_gaps_form_separate_groups() {
        let matches = vec![
            deleted(prose("old one")),
            inserted(prose("new one")),
            unchanged("anchor"),
            deleted(prose("old two")),
            inserted(prose("new two")),
        ];
        let plan = ChangePlan::build(&matches, 0.25);
        let first_group = &plan.group_info[&plan.gaps[0].deletions[0].change_id].group_id;
        let second_group = &plan.group_info[&plan.gaps[1].deletions[0].change_id].group_id;
        assert_ne!(first_group, second_group);
    }

    #[test]
    fn a_lone_deletion_group_is_del_typed_and_sole_positioned() {
        let matches = vec![deleted(prose("old only"))];
        let plan = ChangePlan::build(&matches, 0.25);
        let id = &plan.gaps[0].deletions[0].change_id;
        assert_eq!(plan.group_info[id].group_type, GroupType::Del);
        assert_eq!(plan.group_info[id].group_pos, GroupPos::Sole);
    }

    // --- code-block pairing ---

    #[test]
    fn code_blocks_pair_positionally() {
        let matches = vec![
            deleted(code("fn old() {}", Some("rust"))),
            inserted(code("fn new() {}", Some("rust"))),
        ];
        let plan = ChangePlan::build(&matches, 0.25);
        assert!(plan.gaps[0].deletions.is_empty());
        assert_eq!(plan.gaps[0].insertion_slots.len(), 1);
        assert!(matches!(
            plan.gaps[0].insertion_slots[0],
            InsertionSlot::CodeBlockPair(_)
        ));
    }

    #[test]
    fn mermaid_code_blocks_are_excluded_from_pairing() {
        let matches = vec![
            deleted(code("graph TD; A-->B;", Some("mermaid"))),
            inserted(code("graph TD; A-->C;", Some("mermaid"))),
        ];
        let plan = ChangePlan::build(&matches, 0.25);
        assert_eq!(plan.gaps[0].deletions.len(), 1);
        assert_eq!(plan.gaps[0].insertion_slots.len(), 1);
        assert!(matches!(
            plan.gaps[0].insertion_slots[0],
            InsertionSlot::Block(_)
        ));
    }

    #[test]
    fn math_code_blocks_are_excluded_from_pairing() {
        let matches = vec![
            deleted(code("x = 1", Some("math"))),
            inserted(code("x = 2", Some("Math"))),
        ];
        let plan = ChangePlan::build(&matches, 0.25);
        assert!(matches!(
            plan.gaps[0].insertion_slots[0],
            InsertionSlot::Block(_)
        ));
    }

    #[test]
    fn code_block_pair_carries_line_diff() {
        let matches = vec![
            deleted(code("line one\nline two", Some("rust"))),
            inserted(code("line one\nline changed", Some("rust"))),
        ];
        let plan = ChangePlan::build(&matches, 0.25);
        let InsertionSlot::CodeBlockPair(pair) = &plan.gaps[0].insertion_slots[0] else {
            panic!("expected a code block pair");
        };
        assert!(pair.line_diff.is_some());
    }

    // --- word-diff pairing ---

    #[test]
    fn significant_word_changes_are_recorded_both_directions() {
        let matches = vec![
            deleted(prose("the quick fox")),
            inserted(prose("the slow fox")),
        ];
        let plan = ChangePlan::build(&matches, 0.25);
        let del_id = &plan.gaps[0].pairs[0].deletion.change_id;
        let ins_id = &plan.gaps[0].pairs[0].insertion.change_id;
        assert_eq!(plan.paired_change_id[del_id], *ins_id);
        assert_eq!(plan.paired_change_id[ins_id], *del_id);
        assert!(plan.word_spans.contains_key(del_id));
        assert!(plan.word_spans.contains_key(ins_id));
    }

    #[test]
    fn insignificant_word_changes_are_not_recorded() {
        let matches = vec![
            deleted(prose("alpha beta gamma delta epsilon")),
            inserted(prose("zeta eta theta iota kappa")),
        ];
        let plan = ChangePlan::build(&matches, 0.25);
        let del_id = &plan.gaps[0].pairs[0].deletion.change_id;
        assert!(!plan.word_spans.contains_key(del_id));
    }

    // --- cache ---

    #[test]
    fn cache_hit_returns_equal_plan_without_growing() {
        let matches = vec![deleted(prose("only"))];
        let mut cache = PlanCache::new(8);
        let first = cache.get_or_build(&matches, 0.25);
        let second = cache.get_or_build(&matches, 0.25);
        assert_eq!(first, second);
        assert_eq!(cache.entries.len(), 1);
    }

    #[test]
    fn cache_evicts_least_recently_used_beyond_capacity() {
        let mut cache = PlanCache::new(2);
        let key_a = PlanCache::cache_key(&[], 0.1);
        let key_b = PlanCache::cache_key(&[], 0.2);
        let key_c = PlanCache::cache_key(&[], 0.3);

        cache.get_or_build(&[], 0.1); // entries: [a]
        cache.get_or_build(&[], 0.2); // entries: [b, a]
        cache.get_or_build(&[], 0.1); // hit a, moves to front: [a, b]
        cache.get_or_build(&[], 0.3); // insert c, evict b: [c, a]

        assert_eq!(cache.entries.len(), 2);
        let keys: Vec<u64> = cache.entries.iter().map(|(k, _)| *k).collect();
        assert_eq!(keys, vec![key_c, key_a]);
        assert!(!keys.contains(&key_b));
    }
}
