//! The change plan (`ChangePlan.swift`; plan §7.5).
//!
//! One pass over the block matches makes every decision the consumers —
//! Up rendering, Down rendering, the sidebar, and `mud-changes.js` — must
//! agree on. Change and group IDs are the contract between them, so this
//! mirrors `ChangePlan.swift` exactly:
//!
//! - **Change IDs** (`change-N`) are minted in match order. A code-block
//!   pair's line clusters mint theirs when the pair's gap closes, after
//!   every block-level ID in that gap.
//! - **Code-block pairing**: within a gap, the i-th deleted code block pairs
//!   with the i-th inserted one, skipping Mermaid and math blocks and pairs
//!   whose line diff finds no changed line (those stay block-level).
//! - **Positional pairing**: the remaining deletions pair with the block
//!   insertions in order. Word spans are kept only when they pass the
//!   significance threshold — the threshold gates spans, not pairing.
//! - **Grouping**: one group per gap; a code pair closes the current group
//!   and each of its line clusters becomes a group of its own.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::block::{match_blocks, BlockKind, BlockMatch, DefinitionPolicy, LeafBlock};
use crate::code::{self, CodeLine};
use crate::line::Annotation;
use crate::word::{self, WordSpan};

/// `mud`'s default word-diff significance threshold.
pub const DEFAULT_WORD_DIFF_THRESHOLD: f64 = 0.25;

/// Code-block languages that never pair line-by-line: Up mode shows a
/// diagram or typeset math there, so there are no lines to mark.
const UNPAIRABLE_LANGUAGES: [&str; 2] = ["mermaid", "math"];

/// A changed block with its minted change ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub change_id: String,
    pub block: LeafBlock,
}

/// A deletion and an insertion paired positionally within a gap.
/// `word_spans` is `None` when the word diff fails the threshold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pair {
    pub deletion: Change,
    pub insertion: Change,
    pub word_spans: Option<Vec<WordSpan>>,
}

/// A code-block pair shown as a line diff. The block-level `deletion` and
/// `insertion` IDs render nowhere but keep the ID counter aligned across
/// consumers; the lines carry the cluster IDs that do render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeBlockPair {
    pub deletion: Change,
    pub insertion: Change,
    pub lines: Vec<CodeLine>,
}

/// One insertion position in a gap, in match order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InsertionSlot {
    /// A block-level insertion.
    Block(Change),
    /// A paired code block, sitting at its insertion's position.
    CodeBlockPair(CodeBlockPair),
}

impl InsertionSlot {
    /// The inserted block occupying this slot.
    pub fn insertion_block(&self) -> &LeafBlock {
        match self {
            InsertionSlot::Block(change) => &change.block,
            InsertionSlot::CodeBlockPair(pair) => &pair.insertion.block,
        }
    }
}

/// A run of deletions and insertions between two unchanged anchors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gap {
    /// Block-level deletions in match order (code-pair deletions are
    /// consumed by their pair).
    pub deletions: Vec<Change>,
    /// Insertions in match order, with code pairs in place.
    pub insertion_slots: Vec<InsertionSlot>,
    /// The i-th deletion with the i-th block insertion.
    pub pairs: Vec<Pair>,
    /// The unchanged (new-side) block before the gap; `None` at the start.
    pub preceding_anchor: Option<LeafBlock>,
    /// The unchanged (new-side) block after the gap; `None` at the end.
    pub following_anchor: Option<LeafBlock>,
}

/// Whether a group holds insertions, deletions, or both
/// (`data-group-type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GroupType {
    Ins,
    Del,
    Mix,
}

impl GroupType {
    /// The attribute value: `ins`, `del`, or `mix`.
    pub fn as_str(self) -> &'static str {
        match self {
            GroupType::Ins => "ins",
            GroupType::Del => "del",
            GroupType::Mix => "mix",
        }
    }

    fn of(has_del: bool, has_ins: bool) -> Self {
        match (has_del, has_ins) {
            (true, true) => GroupType::Mix,
            (false, true) => GroupType::Ins,
            _ => GroupType::Del,
        }
    }
}

/// A change's position within its group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GroupPos {
    Sole,
    First,
    Middle,
    Last,
}

impl GroupPos {
    /// The attribute value: `sole`, `first`, `middle`, or `last`.
    pub fn as_str(self) -> &'static str {
        match self {
            GroupPos::Sole => "sole",
            GroupPos::First => "first",
            GroupPos::Middle => "middle",
            GroupPos::Last => "last",
        }
    }

    /// True for the positions that carry the badge (`data-group-index`).
    pub fn shows_badge(self) -> bool {
        matches!(self, GroupPos::Sole | GroupPos::First)
    }
}

/// A change's membership in a group.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GroupInfo {
    /// `group-N`.
    pub group_id: String,
    /// 1-based group number, shown on the badge.
    pub index: usize,
    pub pos: GroupPos,
    pub ty: GroupType,
}

impl GroupInfo {
    /// True when the group holds both deletions and insertions.
    pub fn is_mixed(&self) -> bool {
        self.ty == GroupType::Mix
    }
}

/// The result of the plan pass. Self-contained (it owns clones of the
/// changed and anchor blocks) so it can be cached and shared.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChangePlan {
    /// Gaps in document order.
    pub gaps: Vec<Gap>,
    /// Group membership per change ID, code-line clusters included. Block
    /// IDs consumed by a code pair have no entry.
    pub groups: HashMap<String, GroupInfo>,
    /// Deletion ↔ insertion change IDs of positional pairs, both ways.
    pub paired_change_id: HashMap<String, String>,
    /// Significant word spans of positional pairs, under both IDs.
    pub word_spans: HashMap<String, Vec<WordSpan>>,
}

/// Sequential `prefix-N` IDs.
struct Minter {
    prefix: &'static str,
    count: usize,
}

impl Minter {
    fn new(prefix: &'static str) -> Self {
        Minter { prefix, count: 0 }
    }

    fn next(&mut self) -> String {
        self.count += 1;
        format!("{}-{}", self.prefix, self.count)
    }
}

impl ChangePlan {
    /// Matches `old` against `new` and runs the plan pass.
    pub fn build(old: &[LeafBlock], new: &[LeafBlock], threshold: f64) -> ChangePlan {
        ChangePlan::from_matches(old, new, &match_blocks(old, new), threshold)
    }

    /// Runs the plan pass over [`match_blocks`] output (indices into `old`
    /// and `new`).
    pub fn from_matches(
        old: &[LeafBlock],
        new: &[LeafBlock],
        matches: &[BlockMatch],
        threshold: f64,
    ) -> ChangePlan {
        let mut ids = Minter::new("change");
        let mut gaps = Vec::new();
        let mut deletions: Vec<Change> = Vec::new();
        let mut insertions: Vec<Change> = Vec::new();
        let mut preceding: Option<LeafBlock> = None;

        for m in matches {
            match *m {
                BlockMatch::Deleted(i) => deletions.push(Change {
                    change_id: ids.next(),
                    block: old[i].clone(),
                }),
                BlockMatch::Inserted(i) => insertions.push(Change {
                    change_id: ids.next(),
                    block: new[i].clone(),
                }),
                BlockMatch::Unchanged(_, n) => {
                    let anchor = new[n].clone();
                    close_gap(
                        &mut gaps,
                        &mut ids,
                        &mut deletions,
                        &mut insertions,
                        &preceding,
                        Some(anchor.clone()),
                        threshold,
                    );
                    preceding = Some(anchor);
                }
            }
        }
        close_gap(
            &mut gaps,
            &mut ids,
            &mut deletions,
            &mut insertions,
            &preceding,
            None,
            threshold,
        );

        let groups = assign_groups(&mut gaps);

        let mut paired_change_id = HashMap::new();
        let mut word_spans = HashMap::new();
        for pair in gaps.iter().flat_map(|g| &g.pairs) {
            let (d, i) = (&pair.deletion.change_id, &pair.insertion.change_id);
            paired_change_id.insert(d.clone(), i.clone());
            paired_change_id.insert(i.clone(), d.clone());
            if let Some(spans) = &pair.word_spans {
                word_spans.insert(d.clone(), spans.clone());
                word_spans.insert(i.clone(), spans.clone());
            }
        }

        ChangePlan {
            gaps,
            groups,
            paired_change_id,
            word_spans,
        }
    }

    /// True when nothing changed.
    pub fn is_empty(&self) -> bool {
        self.gaps.is_empty()
    }
}

fn is_unpairable(block: &LeafBlock) -> bool {
    UNPAIRABLE_LANGUAGES.iter().any(|l| block.has_language(l))
}

/// Closes the pending gap, if any: pairs code blocks (minting their cluster
/// IDs), pairs the rest positionally, and appends the gap.
fn close_gap(
    gaps: &mut Vec<Gap>,
    ids: &mut Minter,
    deletions: &mut Vec<Change>,
    insertions: &mut Vec<Change>,
    preceding: &Option<LeafBlock>,
    following: Option<LeafBlock>,
    threshold: f64,
) {
    if deletions.is_empty() && insertions.is_empty() {
        return;
    }
    let pending_del = std::mem::take(deletions);
    let pending_ins = std::mem::take(insertions);

    let deleted_code =
        (0..pending_del.len()).filter(|&i| pending_del[i].block.kind == BlockKind::CodeBlock);
    let inserted_code =
        (0..pending_ins.len()).filter(|&i| pending_ins[i].block.kind == BlockKind::CodeBlock);

    // Insertion index → (consumed deletion index, its line diff).
    let mut code_pairs: Vec<Option<(usize, Vec<CodeLine>)>> = vec![None; pending_ins.len()];
    let mut consumed = vec![false; pending_del.len()];
    for (d, i) in deleted_code.zip(inserted_code) {
        let (old_block, new_block) = (&pending_del[d].block, &pending_ins[i].block);
        if is_unpairable(old_block) || is_unpairable(new_block) {
            continue;
        }
        let old_code = old_block.code.as_deref().unwrap_or("");
        let new_code = new_block.code.as_deref().unwrap_or("");
        let Some(mut lines) = code::compute_raw(old_code, new_code, threshold) else {
            continue;
        };
        code::assign_change_ids(&mut lines, || ids.next());
        code_pairs[i] = Some((d, lines));
        consumed[d] = true;
    }

    let mut slots = Vec::with_capacity(pending_ins.len());
    let mut block_insertions = Vec::new();
    for (ins, code_pair) in pending_ins.into_iter().zip(code_pairs) {
        match code_pair {
            Some((d, lines)) => slots.push(InsertionSlot::CodeBlockPair(CodeBlockPair {
                deletion: pending_del[d].clone(),
                insertion: ins,
                lines,
            })),
            None => {
                block_insertions.push(ins.clone());
                slots.push(InsertionSlot::Block(ins));
            }
        }
    }

    let gap_deletions: Vec<Change> = pending_del
        .into_iter()
        .zip(consumed)
        .filter_map(|(change, consumed)| (!consumed).then_some(change))
        .collect();

    let pairs = gap_deletions
        .iter()
        .zip(block_insertions)
        .map(|(del, ins)| {
            let spans = word::diff(del.block.word_text(), ins.block.word_text());
            let significant = word::has_significant_changes(&spans, threshold);
            Pair {
                deletion: del.clone(),
                insertion: ins,
                word_spans: significant.then_some(spans),
            }
        })
        .collect();

    gaps.push(Gap {
        deletions: gap_deletions,
        insertion_slots: slots,
        pairs,
        preceding_anchor: preceding.clone(),
        following_anchor: following,
    });
}

/// The grouping pass, in document order. Writes cluster group IDs into code
/// pair lines and returns every change ID's group membership.
fn assign_groups(gaps: &mut [Gap]) -> HashMap<String, GroupInfo> {
    let mut groups = HashMap::new();
    let mut counter = 0usize;
    // (change ID, is deletion)
    let mut current: Vec<(String, bool)> = Vec::new();

    let finalize = |current: &mut Vec<(String, bool)>,
                    counter: &mut usize,
                    groups: &mut HashMap<String, GroupInfo>| {
        if current.is_empty() {
            return;
        }
        *counter += 1;
        let group_id = format!("group-{counter}");
        let ty = GroupType::of(
            current.iter().any(|(_, del)| *del),
            current.iter().any(|(_, del)| !*del),
        );
        let count = current.len();
        for (i, (change_id, _)) in current.drain(..).enumerate() {
            let pos = if count == 1 {
                GroupPos::Sole
            } else if i == 0 {
                GroupPos::First
            } else if i == count - 1 {
                GroupPos::Last
            } else {
                GroupPos::Middle
            };
            groups.insert(
                change_id,
                GroupInfo {
                    group_id: group_id.clone(),
                    index: *counter,
                    pos,
                    ty,
                },
            );
        }
    };

    for gap in gaps.iter_mut() {
        finalize(&mut current, &mut counter, &mut groups);
        for del in &gap.deletions {
            current.push((del.change_id.clone(), true));
        }
        for slot in &mut gap.insertion_slots {
            match slot {
                InsertionSlot::Block(change) => current.push((change.change_id.clone(), false)),
                InsertionSlot::CodeBlockPair(pair) => {
                    finalize(&mut current, &mut counter, &mut groups);
                    code::assign_group_ids(&mut pair.lines, || {
                        counter += 1;
                        (format!("group-{counter}"), counter)
                    });
                    register_cluster_groups(&pair.lines, &mut groups);
                }
            }
        }
    }
    finalize(&mut current, &mut counter, &mut groups);
    groups
}

/// Registers one `Sole` group entry per code-line cluster, keyed by the
/// cluster's change ID, so renderers can look up its type.
fn register_cluster_groups(lines: &[CodeLine], groups: &mut HashMap<String, GroupInfo>) {
    for cluster in code::clusters(lines) {
        let cluster = &lines[cluster];
        let (Some(change_id), Some(group_id)) = (&cluster[0].change_id, &cluster[0].group_id)
        else {
            continue;
        };
        groups.insert(
            change_id.clone(),
            GroupInfo {
                group_id: group_id.clone(),
                index: cluster.iter().find_map(|l| l.group_index).unwrap_or(0),
                pos: GroupPos::Sole,
                ty: GroupType::of(
                    cluster.iter().any(|l| l.annotation == Annotation::Deleted),
                    cluster.iter().any(|l| l.annotation == Annotation::Inserted),
                ),
            },
        );
    }
}

/// Cache key for a plan: the two diffed texts (hashed, with lengths as a
/// cheap collision guard), the threshold, and the definition policy — the
/// same text pair yields different leaves per policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlanKey {
    old_hash: u64,
    old_len: usize,
    new_hash: u64,
    new_len: usize,
    threshold_bits: u64,
    policy: DefinitionPolicy,
}

impl PlanKey {
    pub fn new(old: &str, new: &str, threshold: f64, policy: DefinitionPolicy) -> Self {
        let hash = |s: &str| {
            let mut h = DefaultHasher::new();
            s.hash(&mut h);
            h.finish()
        };
        PlanKey {
            old_hash: hash(old),
            old_len: old.len(),
            new_hash: hash(new),
            new_len: new.len(),
            threshold_bits: threshold.to_bits(),
            policy,
        }
    }
}

/// A small LRU of computed plans (`ChangePlan.plan`'s cache), so the Up
/// render, the sidebar list, and the change count for one reload diff the
/// documents once. Not synchronised: the owner wraps it in a `Mutex`.
#[derive(Debug, Default)]
pub struct PlanCache {
    plans: HashMap<PlanKey, Arc<ChangePlan>>,
    /// Most recently used first.
    order: VecDeque<PlanKey>,
}

impl PlanCache {
    /// How many plans are kept (as in `mud`).
    pub const CAPACITY: usize = 8;

    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.plans.len()
    }

    pub fn is_empty(&self) -> bool {
        self.plans.is_empty()
    }

    /// Returns the cached plan for `key`, marking it most recently used.
    pub fn get(&mut self, key: &PlanKey) -> Option<Arc<ChangePlan>> {
        let plan = self.plans.get(key)?.clone();
        self.touch(*key);
        Some(plan)
    }

    /// Stores `plan` under `key` as most recently used, evicting the least
    /// recently used entry beyond [`Self::CAPACITY`].
    pub fn insert(&mut self, key: PlanKey, plan: ChangePlan) -> Arc<ChangePlan> {
        let plan = Arc::new(plan);
        self.plans.insert(key, plan.clone());
        self.touch(key);
        while self.order.len() > Self::CAPACITY {
            if let Some(evicted) = self.order.pop_back() {
                self.plans.remove(&evicted);
            }
        }
        plan
    }

    /// Returns the plan for `key`, running `build` only on a miss.
    pub fn get_or_insert_with(
        &mut self,
        key: PlanKey,
        build: impl FnOnce() -> ChangePlan,
    ) -> Arc<ChangePlan> {
        match self.get(&key) {
            Some(plan) => plan,
            None => self.insert(key, build()),
        }
    }

    fn touch(&mut self, key: PlanKey) {
        if let Some(pos) = self.order.iter().position(|k| *k == key) {
            self.order.remove(pos);
        }
        self.order.push_front(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::doc;

    fn plan(old: &str, new: &str) -> ChangePlan {
        ChangePlan::build(&doc(old), &doc(new), DEFAULT_WORD_DIFF_THRESHOLD)
    }

    fn group(p: &ChangePlan, id: &str) -> (String, usize, GroupPos, GroupType) {
        let g = p
            .groups
            .get(id)
            .unwrap_or_else(|| panic!("no group for {id}"));
        (g.group_id.clone(), g.index, g.pos, g.ty)
    }

    fn g(
        id: &str,
        index: usize,
        pos: GroupPos,
        ty: GroupType,
    ) -> (String, usize, GroupPos, GroupType) {
        (id.to_string(), index, pos, ty)
    }

    fn ids(changes: &[Change]) -> Vec<&str> {
        changes.iter().map(|c| c.change_id.as_str()).collect()
    }

    fn cluster_ids(pair: &CodeBlockPair) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for l in &pair.lines {
            if let Some(id) = l.change_id.as_deref() {
                if out.last() != Some(&id) {
                    out.push(id);
                }
            }
        }
        out
    }

    fn code_pair(slot: &InsertionSlot) -> &CodeBlockPair {
        match slot {
            InsertionSlot::CodeBlockPair(p) => p,
            InsertionSlot::Block(c) => panic!("expected a code pair, got block {}", c.change_id),
        }
    }

    fn block(slot: &InsertionSlot) -> &str {
        match slot {
            InsertionSlot::Block(c) => &c.change_id,
            InsertionSlot::CodeBlockPair(_) => panic!("expected a block slot"),
        }
    }

    use GroupPos::{First, Last, Middle, Sole};
    use GroupType::{Del, Ins, Mix};

    /// `ChangeIDParityTests.corpus`, with the IDs and groups `mud`'s
    /// `ChangePlan` assigns pinned per case.
    mod corpus_tests {
        use super::*;

        #[test]
        fn paragraph_edit() {
            let p = plan("Alpha one.\n\nBeta.\n", "Alpha two.\n\nBeta.\n");
            assert_eq!(p.gaps.len(), 1);
            assert_eq!(ids(&p.gaps[0].deletions), ["change-1"]);
            assert_eq!(block(&p.gaps[0].insertion_slots[0]), "change-2");
            assert_eq!(group(&p, "change-1"), g("group-1", 1, First, Mix));
            assert_eq!(group(&p, "change-2"), g("group-1", 1, Last, Mix));
            assert!(p.word_spans.contains_key("change-1"));
            assert_eq!(
                p.gaps[0].following_anchor.as_ref().unwrap().source_text,
                "Beta."
            );
        }

        #[test]
        fn insertion_only() {
            let p = plan("Alpha.\n", "Alpha.\n\nAdded.\n");
            assert_eq!(group(&p, "change-1"), g("group-1", 1, Sole, Ins));
            assert!(p.gaps[0].following_anchor.is_none());
            assert_eq!(
                p.gaps[0].preceding_anchor.as_ref().unwrap().source_text,
                "Alpha."
            );
        }

        #[test]
        fn trailing_deletion_only() {
            let p = plan("Alpha.\n\nGone.\n", "Alpha.\n");
            assert_eq!(group(&p, "change-1"), g("group-1", 1, Sole, Del));
        }

        #[test]
        fn code_block_edit_alone() {
            let p = plan("```\nkeep\nold\n```\n", "```\nkeep\nnew\n```\n");
            let gap = &p.gaps[0];
            assert!(gap.deletions.is_empty());
            let pair = code_pair(&gap.insertion_slots[0]);
            assert_eq!(
                (
                    pair.deletion.change_id.as_str(),
                    pair.insertion.change_id.as_str()
                ),
                ("change-1", "change-2")
            );
            assert_eq!(cluster_ids(pair), ["change-3"]);
            assert_eq!(group(&p, "change-3"), g("group-1", 1, Sole, Mix));
            assert!(!p.groups.contains_key("change-1") && !p.groups.contains_key("change-2"));
        }

        #[test]
        fn code_block_edit_then_paragraph_edit() {
            let p = plan(
                "```\nkeep\nold\n```\n\nMiddle.\n\nTail one.\n",
                "```\nkeep\nnew\n```\n\nMiddle.\n\nTail two.\n",
            );
            assert_eq!(
                cluster_ids(code_pair(&p.gaps[0].insertion_slots[0])),
                ["change-3"]
            );
            assert_eq!(group(&p, "change-3"), g("group-1", 1, Sole, Mix));
            assert_eq!(group(&p, "change-4"), g("group-2", 2, First, Mix));
            assert_eq!(group(&p, "change-5"), g("group-2", 2, Last, Mix));
        }

        #[test]
        fn paragraph_edit_then_code_block_edit() {
            let p = plan(
                "Head one.\n\nMiddle.\n\n```\nkeep\nold\n```\n",
                "Head two.\n\nMiddle.\n\n```\nkeep\nnew\n```\n",
            );
            assert_eq!(group(&p, "change-1"), g("group-1", 1, First, Mix));
            assert_eq!(group(&p, "change-2"), g("group-1", 1, Last, Mix));
            assert_eq!(
                cluster_ids(code_pair(&p.gaps[1].insertion_slots[0])),
                ["change-5"]
            );
            assert_eq!(group(&p, "change-5"), g("group-2", 2, Sole, Mix));
        }

        #[test]
        fn two_code_block_edits() {
            let p = plan(
                "```\na\nold1\n```\n\nMid.\n\n```\nb\nold2\n```\n",
                "```\na\nnew1\n```\n\nMid.\n\n```\nb\nnew2\n```\n",
            );
            assert_eq!(
                cluster_ids(code_pair(&p.gaps[0].insertion_slots[0])),
                ["change-3"]
            );
            assert_eq!(
                cluster_ids(code_pair(&p.gaps[1].insertion_slots[0])),
                ["change-6"]
            );
            assert_eq!(group(&p, "change-6"), g("group-2", 2, Sole, Mix));
        }

        #[test]
        fn code_block_edit_and_insertion_in_the_same_gap() {
            let p = plan(
                "```\nkeep\nold\n```\n\nTail.\n",
                "```\nkeep\nnew\n```\n\nAdded.\n\nTail.\n",
            );
            let gap = &p.gaps[0];
            assert_eq!(
                cluster_ids(code_pair(&gap.insertion_slots[0])),
                ["change-4"]
            );
            assert_eq!(block(&gap.insertion_slots[1]), "change-3");
            assert!(gap.pairs.is_empty());
            assert_eq!(group(&p, "change-4"), g("group-1", 1, Sole, Mix));
            assert_eq!(group(&p, "change-3"), g("group-2", 2, Sole, Ins));
        }

        #[test]
        fn multi_cluster_code_block_edit_then_paragraph_edit() {
            let p = plan(
                "```\na\nb\nc\nd\ne\n```\n\nTail one.\n",
                "```\na\nB\nc\nD\ne\n```\n\nTail two.\n",
            );
            assert_eq!(p.gaps.len(), 1);
            let gap = &p.gaps[0];
            assert_eq!(ids(&gap.deletions), ["change-2"]);
            assert_eq!(
                cluster_ids(code_pair(&gap.insertion_slots[0])),
                ["change-5", "change-6"]
            );
            assert_eq!(block(&gap.insertion_slots[1]), "change-4");
            // The positional pair spans the code pair, so its two halves
            // land in different groups (as in mud).
            assert_eq!(p.paired_change_id["change-2"], "change-4");
            assert_eq!(group(&p, "change-2"), g("group-1", 1, Sole, Del));
            assert_eq!(group(&p, "change-5"), g("group-2", 2, Sole, Mix));
            assert_eq!(group(&p, "change-6"), g("group-3", 3, Sole, Mix));
            assert_eq!(group(&p, "change-4"), g("group-4", 4, Sole, Ins));
        }

        #[test]
        fn paragraph_and_code_block_swap_positions_in_one_gap() {
            let p = plan(
                "Intro.\n\nAlpha beta gamma.\n\n```\nkeep\nold\n```\n\nTail.\n",
                "Intro.\n\n```\nkeep\nnew\n```\n\nDelta epsilon zeta.\n\nTail.\n",
            );
            let gap = &p.gaps[0];
            assert_eq!(ids(&gap.deletions), ["change-1"]);
            assert_eq!(
                cluster_ids(code_pair(&gap.insertion_slots[0])),
                ["change-5"]
            );
            assert_eq!(block(&gap.insertion_slots[1]), "change-4");
            // Paired, but too dissimilar for word spans.
            assert_eq!(p.paired_change_id["change-4"], "change-1");
            assert!(gap.pairs[0].word_spans.is_none());
            assert!(!p.word_spans.contains_key("change-1"));
            assert_eq!(group(&p, "change-1"), g("group-1", 1, Sole, Del));
            assert_eq!(group(&p, "change-5"), g("group-2", 2, Sole, Mix));
            assert_eq!(group(&p, "change-4"), g("group-3", 3, Sole, Ins));
        }

        #[test]
        fn paragraph_deleted_and_code_block_edited_in_one_gap() {
            let p = plan(
                "Gone paragraph.\n\n```\nkeep\nold\n```\n\nTail.\n",
                "```\nkeep\nnew\n```\n\nTail.\n",
            );
            let gap = &p.gaps[0];
            assert_eq!(ids(&gap.deletions), ["change-1"]);
            assert_eq!(
                cluster_ids(code_pair(&gap.insertion_slots[0])),
                ["change-4"]
            );
            assert_eq!(group(&p, "change-1"), g("group-1", 1, Sole, Del));
            assert_eq!(group(&p, "change-4"), g("group-2", 2, Sole, Mix));
        }
    }

    mod code_pairing_tests {
        use super::*;

        #[test]
        fn mermaid_blocks_stay_block_level() {
            let p = plan("```mermaid\ngraph A\n```\n", "```Mermaid\ngraph B\n```\n");
            let gap = &p.gaps[0];
            assert_eq!(ids(&gap.deletions), ["change-1"]);
            assert_eq!(block(&gap.insertion_slots[0]), "change-2");
            assert_eq!(group(&p, "change-1"), g("group-1", 1, First, Mix));
            // Code blocks have no inline text, so the pair has no spans.
            assert_eq!(p.paired_change_id["change-1"], "change-2");
            assert!(p.word_spans.is_empty());
        }

        #[test]
        fn math_blocks_stay_block_level_on_either_side() {
            let p = plan("```math\nx\n```\n", "```\ny\n```\n");
            assert_eq!(ids(&p.gaps[0].deletions), ["change-1"]);
        }

        #[test]
        fn language_only_change_stays_block_level() {
            let p = plan("```rust\nx\n```\n", "```python\nx\n```\n");
            assert_eq!(ids(&p.gaps[0].deletions), ["change-1"]);
            assert_eq!(block(&p.gaps[0].insertion_slots[0]), "change-2");
        }

        #[test]
        fn code_blocks_pair_by_type_wherever_they_sit() {
            // Deleted: P, C. Inserted: C', P'. Code pairs with code.
            let p = plan(
                "Top.\n\nOld words here.\n\n```\nx\n```\n\nEnd.\n",
                "Top.\n\n```\ny\n```\n\nNew words here.\n\nEnd.\n",
            );
            let gap = &p.gaps[0];
            assert_eq!(ids(&gap.deletions), ["change-1"]);
            assert_eq!(
                code_pair(&gap.insertion_slots[0]).deletion.change_id,
                "change-2"
            );
            assert_eq!(gap.pairs[0].insertion.change_id, "change-4");
            assert!(gap.pairs[0].word_spans.is_some());
        }
    }

    mod pairing_and_grouping_tests {
        use super::*;

        #[test]
        fn threshold_gates_spans_not_pairing() {
            let old = doc("The quick fox.\n");
            let new = doc("The slow fox.\n");
            let lenient = ChangePlan::build(&old, &new, 0.25);
            let strict = ChangePlan::build(&old, &new, 0.99);
            assert!(lenient.word_spans.contains_key("change-2"));
            assert!(strict.word_spans.is_empty());
            assert_eq!(strict.paired_change_id["change-1"], "change-2");
            assert_eq!(strict.paired_change_id["change-2"], "change-1");
        }

        #[test]
        fn extra_deletions_stay_unpaired() {
            let p = plan("One.\n\nTwo.\n\nThree.\n", "Uno.\n");
            assert_eq!(p.gaps[0].pairs.len(), 1);
            assert_eq!(p.paired_change_id.len(), 2);
            assert_eq!(group(&p, "change-1"), g("group-1", 1, First, Mix));
            assert_eq!(group(&p, "change-2"), g("group-1", 1, Middle, Mix));
            assert_eq!(group(&p, "change-3"), g("group-1", 1, Middle, Mix));
            assert_eq!(group(&p, "change-4"), g("group-1", 1, Last, Mix));
        }

        #[test]
        fn each_gap_is_its_own_group() {
            let p = plan("A.\n\nB.\n\nC.\n", "A2.\n\nB.\n\nC2.\n");
            assert_eq!(group(&p, "change-1").0, "group-1");
            assert_eq!(group(&p, "change-3").0, "group-2");
            assert_eq!(group(&p, "change-4"), g("group-2", 2, Last, Mix));
        }

        #[test]
        fn identical_documents_have_an_empty_plan() {
            let p = plan("A.\n\nB.\n", "A.\n\nB.\n");
            assert!(p.is_empty());
            assert!(p.groups.is_empty());
        }

        #[test]
        fn renders_attribute_values() {
            assert_eq!(GroupType::Mix.as_str(), "mix");
            assert_eq!(GroupPos::Middle.as_str(), "middle");
            assert!(GroupPos::First.shows_badge() && !GroupPos::Last.shows_badge());
        }
    }

    mod plan_cache_tests {
        use super::*;

        fn key(n: usize) -> PlanKey {
            PlanKey::new(&format!("old {n}"), "new", 0.25, DefinitionPolicy::SkipAll)
        }

        #[test]
        fn hit_returns_the_same_plan_without_rebuilding() {
            let mut cache = PlanCache::new();
            let a = cache.get_or_insert_with(key(1), ChangePlan::default);
            let b = cache.get_or_insert_with(key(1), || panic!("rebuilt on a hit"));
            assert!(Arc::ptr_eq(&a, &b));
        }

        #[test]
        fn policy_and_threshold_join_the_key() {
            let up = PlanKey::new("a", "b", 0.25, DefinitionPolicy::SkipAll);
            let down = PlanKey::new("a", "b", 0.25, DefinitionPolicy::DescendPlainFootnotes);
            let strict = PlanKey::new("a", "b", 0.5, DefinitionPolicy::SkipAll);
            assert_ne!(up, down);
            assert_ne!(up, strict);
            assert_eq!(up, PlanKey::new("a", "b", 0.25, DefinitionPolicy::SkipAll));
            assert_ne!(up, PlanKey::new("b", "a", 0.25, DefinitionPolicy::SkipAll));
        }

        #[test]
        fn evicts_the_least_recently_used_beyond_capacity() {
            let mut cache = PlanCache::new();
            for n in 0..PlanCache::CAPACITY {
                cache.insert(key(n), ChangePlan::default());
            }
            // Touch 0 so 1 becomes the oldest.
            assert!(cache.get(&key(0)).is_some());
            cache.insert(key(100), ChangePlan::default());
            assert_eq!(cache.len(), PlanCache::CAPACITY);
            assert!(cache.get(&key(0)).is_some());
            assert!(cache.get(&key(1)).is_none());
            assert!(cache.get(&key(2)).is_some());
        }

        #[test]
        fn reinserting_a_key_does_not_grow_the_cache() {
            let mut cache = PlanCache::new();
            cache.insert(key(1), ChangePlan::default());
            cache.insert(key(1), ChangePlan::default());
            assert_eq!(cache.len(), 1);
        }
    }
}
