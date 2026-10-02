//! `ChangeTrackerTests.swift`, minus the external-waypoint cases. Every
//! time is explicit: `t(s)` is `s` seconds after minute 1000 since the
//! epoch, so minute boundaries are known.

use std::time::Duration;

use super::*;
use crate::changes::compute_groups;

fn t(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1000 * 60 + secs)
}

fn md(s: &str) -> Arc<str> {
    Arc::from(s)
}

fn tracker(initial: &str, at: SystemTime) -> ChangeTracker {
    let mut tr = ChangeTracker::new();
    tr.seed(md(initial), at);
    tr
}

fn kinds(tr: &ChangeTracker) -> Vec<WaypointKind> {
    tr.waypoints().iter().map(|w| w.kind).collect()
}

fn texts(tr: &ChangeTracker, kind: WaypointKind) -> Vec<String> {
    tr.waypoints()
        .iter()
        .filter(|w| w.kind == kind)
        .map(|w| w.markdown.to_string())
        .collect()
}

fn id_of(tr: &ChangeTracker, kind: WaypointKind, text: &str) -> u64 {
    tr.waypoints()
        .iter()
        .find(|w| w.kind == kind && &*w.markdown == text)
        .unwrap_or_else(|| panic!("no {kind:?} waypoint with {text:?}"))
        .id
}

fn gap_items(items: &[MenuItem]) -> Vec<&MenuItem> {
    items
        .iter()
        .filter(|i| i.label.contains("minute"))
        .collect()
}

fn groups_between(old: &str, new: &str) -> usize {
    compute_groups(
        old,
        new,
        DefinitionPolicy::DescendPlainFootnotes,
        DEFAULT_WORD_DIFF_THRESHOLD,
    )
    .len()
}

mod lifecycle {
    use super::*;

    #[test]
    fn seed_creates_initial_waypoint() {
        let tr = tracker("Hello.\n", t(0));
        assert_eq!(kinds(&tr), vec![WaypointKind::Initial]);
        assert!(tr.changes().is_empty());
        assert_eq!(tr.current().map(|s| &**s), Some("Hello.\n"));
    }

    #[test]
    fn seed_is_idempotent() {
        let mut tr = tracker("Hello.\n", t(0));
        tr.seed(md("Other.\n"), t(30));
        assert_eq!(kinds(&tr), vec![WaypointKind::Initial]);
        assert_eq!(texts(&tr, WaypointKind::Initial), vec!["Hello.\n"]);
    }

    #[test]
    fn first_update_on_an_unseeded_tracker_seeds_it() {
        let mut tr = ChangeTracker::new();
        assert!(tr.update(md("Hello.\n"), t(0)));
        assert_eq!(kinds(&tr), vec![WaypointKind::Initial]);
    }

    #[test]
    fn subsequent_update_creates_reload_waypoint() {
        let mut tr = tracker("Hello.\n", t(0));
        assert!(tr.update(md("Changed.\n"), t(90)));
        assert_eq!(
            kinds(&tr),
            vec![WaypointKind::Initial, WaypointKind::Reload]
        );
    }

    #[test]
    fn same_minute_reloads_coalesced() {
        let mut tr = tracker("Hello.\n", t(0));
        tr.update(md("V2.\n"), t(10));
        tr.update(md("V3.\n"), t(20));
        tr.update(md("V4.\n"), t(30));
        assert_eq!(tr.waypoints().len(), 2);
        assert_eq!(&*tr.waypoints()[1].markdown, "V4.\n");
    }

    #[test]
    fn different_minute_reloads_survive() {
        let mut tr = tracker("Hello.\n", t(0));
        tr.update(md("V2.\n"), t(15));
        tr.update(md("V3.\n"), t(75));
        assert_eq!(tr.waypoints().len(), 3);
        assert_eq!(&*tr.waypoints()[1].markdown, "V2.\n");
        assert_eq!(&*tr.waypoints()[2].markdown, "V3.\n");
    }

    #[test]
    fn duplicate_content_skipped() {
        let mut tr = tracker("V1.\n", t(0));
        assert!(tr.update(md("V2.\n"), t(90)));
        assert_eq!(tr.waypoints().len(), 2);
        // Same as the initial — skipped.
        assert!(!tr.update(md("V1.\n"), t(180)));
        assert_eq!(tr.waypoints().len(), 2);
        // Same as the reload — skipped.
        assert!(!tr.update(md("V2.\n"), t(270)));
        assert_eq!(tr.waypoints().len(), 2);
        assert!(tr.update(md("V3.\n"), t(360)));
        assert_eq!(tr.waypoints().len(), 3);
    }

    #[test]
    fn duplicate_content_skipped_even_when_rapid() {
        let mut tr = tracker("V1.\n", t(0));
        tr.update(md("V2.\n"), t(1));
        tr.update(md("V2.\n"), t(2));
        assert_eq!(tr.waypoints().len(), 2);
    }

    #[test]
    fn reload_count_capped_at_ten() {
        let mut tr = tracker("V0.\n", t(0));
        for i in 1..=12 {
            tr.update(md(&format!("V{i}.\n")), t(i * 60));
        }
        let mut reloads = texts(&tr, WaypointKind::Reload);
        reloads.sort();
        let mut expected: Vec<String> = (3..=12).map(|i| format!("V{i}.\n")).collect();
        expected.sort();
        assert_eq!(reloads, expected);
        assert!(kinds(&tr).contains(&WaypointKind::Initial));
    }

    #[test]
    fn reloads_survive_long_idle_under_cap() {
        let mut tr = tracker("V0.\n", t(0));
        for i in 1..=5 {
            tr.update(md(&format!("V{i}.\n")), t(i * 60));
        }
        tr.update(md("V6.\n"), t(3 * 60 * 60));
        let reloads = texts(&tr, WaypointKind::Reload);
        assert_eq!(reloads.len(), 6);
        assert!(reloads.contains(&"V1.\n".to_string()));
    }

    #[test]
    fn reload_cap_clears_orphaned_selected_waypoint() {
        let mut tr = tracker("V0.\n", t(0));
        for i in 1..=10 {
            tr.update(md(&format!("V{i}.\n")), t(i * 60));
        }
        let oldest = id_of(&tr, WaypointKind::Reload, "V1.\n");
        tr.select_waypoint(Some(oldest));
        assert_eq!(tr.selected_waypoint(), Some(oldest));

        tr.update(md("V11.\n"), t(11 * 60));
        assert!(!tr.waypoints().iter().any(|w| w.id == oldest));
        assert_eq!(tr.selected_waypoint(), None);
    }

    #[test]
    fn pruning_never_removes_initial() {
        let mut tr = tracker("Hello.\n", t(0));
        tr.update(md("V2.\n"), t(60 * 60));
        assert!(kinds(&tr).contains(&WaypointKind::Initial));
    }

    #[test]
    fn pruning_never_removes_accept() {
        let mut tr = tracker("Hello.\n", t(0));
        tr.update(md("V2.\n"), t(90));
        tr.accept(t(90));
        tr.update(md("V3.\n"), t(60 * 60));
        assert!(kinds(&tr).contains(&WaypointKind::Accept));
    }

    #[test]
    fn accept_creates_accept_waypoint() {
        let mut tr = tracker("Hello.\n", t(0));
        tr.update(md("V2.\n"), t(90));
        tr.accept(t(90));
        assert_eq!(texts(&tr, WaypointKind::Accept), vec!["V2.\n"]);
    }

    #[test]
    fn second_accept_replaces_previous_in_place() {
        let mut tr = tracker("Hello.\n", t(0));
        tr.update(md("V2.\n"), t(90));
        tr.accept(t(90));
        let first = id_of(&tr, WaypointKind::Accept, "V2.\n");
        let slot = tr.waypoints().iter().position(|w| w.id == first);

        tr.update(md("V3.\n"), t(180));
        tr.accept(t(180));
        let accepts: Vec<&TrackedWaypoint> = tr
            .waypoints()
            .iter()
            .filter(|w| w.kind == WaypointKind::Accept)
            .collect();
        assert_eq!(accepts.len(), 1);
        assert_ne!(accepts[0].id, first);
        assert_eq!(
            tr.waypoints().iter().position(|w| w.id == accepts[0].id),
            slot
        );
    }

    #[test]
    fn accept_before_any_content_does_nothing() {
        let mut tr = ChangeTracker::new();
        tr.accept(t(0));
        assert!(tr.waypoints().is_empty());
    }

    #[test]
    fn accept_clears_changes_and_selection() {
        let mut tr = tracker("V1.\n", t(0));
        tr.update(md("V2.\n"), t(90));
        let reload = id_of(&tr, WaypointKind::Reload, "V2.\n");
        tr.select_waypoint(Some(reload));
        tr.update(md("V3.\n"), t(180));
        assert!(!tr.changes().is_empty());
        tr.accept(t(180));
        assert!(tr.changes().is_empty());
        assert_eq!(tr.selected_waypoint(), None);
    }

    #[test]
    fn waypoint_ids_are_unique_and_increasing() {
        let mut tr = tracker("V1.\n", t(0));
        tr.update(md("V2.\n"), t(60));
        tr.accept(t(60));
        tr.update(md("V3.\n"), t(120));
        tr.accept(t(130));
        let ids: Vec<u64> = tr.waypoints().iter().map(|w| w.id).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len());
    }
}

mod active_waypoint {
    use super::*;

    #[test]
    fn unseeded_tracker_has_no_active_waypoint() {
        assert!(ChangeTracker::new().active().is_none());
    }

    #[test]
    fn defaults_to_initial() {
        let tr = tracker("Hello.\n", t(0));
        assert_eq!(tr.selected_waypoint(), None);
        assert_eq!(tr.active().map(|w| &*w.markdown), Some("Hello.\n"));
    }

    #[test]
    fn defaults_to_accept_when_present() {
        let mut tr = tracker("V1.\n", t(0));
        tr.update(md("V2.\n"), t(90));
        tr.accept(t(90));
        assert_eq!(tr.active().map(|w| &*w.markdown), Some("V2.\n"));
    }

    #[test]
    fn resolves_to_explicit_id() {
        let mut tr = tracker("V1.\n", t(0));
        tr.update(md("V2.\n"), t(90));
        tr.select_waypoint(Some(id_of(&tr, WaypointKind::Reload, "V2.\n")));
        assert_eq!(tr.active().map(|w| &*w.markdown), Some("V2.\n"));
    }

    #[test]
    fn accept_resets_selection() {
        let mut tr = tracker("Hello.\n", t(0));
        tr.update(md("V2.\n"), t(90));
        tr.select_waypoint(Some(id_of(&tr, WaypointKind::Reload, "V2.\n")));
        tr.accept(t(90));
        assert_eq!(tr.selected_waypoint(), None);
    }

    #[test]
    fn selecting_recomputes_the_change_list() {
        let mut tr = tracker("V1.\n", t(0));
        tr.update(md("V2.\n"), t(90));
        assert!(!tr.changes().is_empty());
        tr.select_waypoint(Some(id_of(&tr, WaypointKind::Reload, "V2.\n")));
        assert!(tr.changes().is_empty());
        tr.select_waypoint(None);
        assert!(!tr.changes().is_empty());
    }
}

mod menu {
    use super::*;

    #[test]
    fn empty_before_seeding() {
        assert!(ChangeTracker::new().menu_items(t(0)).is_empty());
    }

    #[test]
    fn shows_document_opened_when_no_accepts() {
        let mut tr = tracker("V1.\n", t(0));
        tr.update(md("V2.\n"), t(90));
        let items = tr.menu_items(t(90));
        assert_eq!(items[0].label, "since document opened");
    }

    #[test]
    fn shows_last_accepted_when_accepted() {
        let mut tr = tracker("V1.\n", t(0));
        tr.update(md("V2.\n"), t(90));
        tr.accept(t(90));
        tr.update(md("V3.\n"), t(180));
        let items = tr.menu_items(t(180));
        assert_eq!(items[0].label, "since last accepted");
    }

    #[test]
    fn shows_document_opened_at_bottom_when_accepted() {
        let mut tr = tracker("V1.\n", t(0));
        tr.update(md("V2.\n"), t(90));
        tr.accept(t(90));
        tr.update(md("V3.\n"), t(180));
        let items = tr.menu_items(t(180));
        assert_eq!(items.last().unwrap().label, "since document opened");
        assert_eq!(
            items.last().unwrap().id,
            id_of(&tr, WaypointKind::Initial, "V1.\n")
        );
    }

    #[test]
    fn item_ids_are_unique() {
        let mut tr = tracker("V1.\n", t(0));
        tr.update(md("V2.\n"), t(60));
        let items = tr.menu_items(t(180));
        let mut ids: Vec<u64> = items.iter().map(|i| i.id).collect();
        let n = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), n);
    }

    #[test]
    fn gap_skips_accept_and_initial() {
        let mut tr = tracker("V1.\n", t(0));
        tr.update(md("V2.\n"), t(180));
        tr.accept(t(180));
        tr.update(md("V3.\n"), t(300));
        let items = tr.menu_items(t(300));
        let accept = id_of(&tr, WaypointKind::Accept, "V2.\n");
        let initial = id_of(&tr, WaypointKind::Initial, "V1.\n");
        for item in gap_items(&items) {
            assert_ne!(item.id, accept);
            assert_ne!(item.id, initial);
        }
    }

    #[test]
    fn change_counts_match_diff() {
        let mut tr = tracker("Alpha.\n\nBeta.\n", t(0));
        tr.update(md("Alpha.\n\nBeta.\n\nGamma.\n"), t(90));
        let items = tr.menu_items(t(90));
        let opened = items
            .iter()
            .find(|i| i.label == "since document opened")
            .unwrap();
        assert_eq!(opened.change_count, 1);
        assert!(opened.has_insertions && !opened.has_deletions);
    }

    #[test]
    fn active_flag_defaults_to_initial() {
        let mut tr = tracker("V1.\n", t(0));
        tr.update(md("V2.\n"), t(90));
        let items = tr.menu_items(t(90));
        let opened = items
            .iter()
            .find(|i| i.label == "since document opened")
            .unwrap();
        assert!(opened.is_active);
    }

    #[test]
    fn active_flag_reflects_explicit_waypoint() {
        let mut tr = tracker("V1.\n", t(0));
        tr.update(md("V2.\n"), t(60));
        tr.update(md("V3.\n"), t(180));
        tr.update(md("V4.\n"), t(300));
        let target = id_of(&tr, WaypointKind::Reload, "V2.\n");
        tr.select_waypoint(Some(target));
        let items = tr.menu_items(t(300));
        let active: Vec<u64> = items.iter().filter(|i| i.is_active).map(|i| i.id).collect();
        assert_eq!(active, vec![target]);
    }

    #[test]
    fn gaps_are_listed_newest_first() {
        let mut tr = tracker("V0.\n", t(0));
        tr.update(md("V1.\n"), t(60));
        tr.update(md("V2.\n"), t(180));
        tr.update(md("V3.\n"), t(300));
        let items = tr.menu_items(t(330));
        let gaps: Vec<u64> = gap_items(&items).iter().map(|i| i.id).collect();
        assert_eq!(
            gaps,
            vec![
                id_of(&tr, WaypointKind::Reload, "V2.\n"),
                id_of(&tr, WaypointKind::Reload, "V1.\n"),
            ]
        );
        // The V3 reload is the newest end of the last gap only; the
        // current content diffs to nothing against it, so no row.
        assert!(!items
            .iter()
            .any(|i| i.id == id_of(&tr, WaypointKind::Reload, "V3.\n")));
        assert_eq!(items[0].label, "since document opened");
    }
}

mod gap_entries {
    use super::*;

    #[test]
    fn minutes_ago_label_formula() {
        let base = t(0);
        let at = |s: u64| base + Duration::from_secs(s);
        assert_eq!(minutes_ago(base, at(51)), 1);
        let anchor1 = at(37);
        let now1 = at(3 * 60 + 40);
        assert_eq!(minutes_ago(anchor1, now1), 4);
        let anchor2 = at(3 * 60 + 23);
        assert_eq!(minutes_ago(anchor2, now1), 1);
        let now2 = at(7 * 60 + 14);
        assert_eq!(minutes_ago(anchor2, now2), 5);
        assert_eq!(minutes_ago(base, now2), 8);
        // Exactly on the anchor's minute boundary: floored at 1.
        assert_eq!(minutes_ago(base, base), 1);
    }

    #[test]
    fn time_ago_phrase_tiers() {
        let now = UNIX_EPOCH + Duration::from_secs(10_000 * 60);
        let phrase = |minutes: u64| time_ago_phrase(now - Duration::from_secs(minutes * 60), now);
        assert_eq!(phrase(1), "1 minute");
        assert_eq!(phrase(59), "59 minutes");
        assert_eq!(phrase(60), "1 hour");
        assert_eq!(phrase(119), "1 hour");
        assert_eq!(phrase(120), "2 hours");
        assert_eq!(phrase(1135), "18 hours");
        assert_eq!(phrase(24 * 60), "1 day");
        assert_eq!(phrase(3 * 24 * 60 + 90), "3 days");
        assert_eq!(phrase(7 * 24 * 60), "1 week");
        assert_eq!(phrase(30 * 24 * 60), "4 weeks");
    }

    #[test]
    fn absolute_minute_matches_mud_reference_minutes() {
        // Swift counts from 2001-01-01T00:00:00Z = Unix 978_307_200, a
        // whole number of minutes, so boundaries coincide.
        assert_eq!(978_307_200 % 60, 0);
        assert_eq!(absolute_minute(t(59)), 1000);
        assert_eq!(absolute_minute(t(60)), 1001);
    }

    #[test]
    fn gap_label_uses_newer_waypoint_minute() {
        let mut tr = tracker("V0.\n", t(0));
        tr.update(md("V1.\n"), t(60));
        tr.update(md("V2.\n"), t(180));
        let items = tr.menu_items(t(5 * 60 + 30));
        let gaps = gap_items(&items);
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].label, "since 3 minutes ago");
        assert_eq!(gaps[0].at, t(180));
    }

    #[test]
    fn gap_diffs_against_older_endpoint() {
        let (v0, v1, v2) = (
            "# Title\n\nP1.\n",
            "# Modified Title\n\nP1.\n",
            "# Modified Title\n\nP1.\n\nP2.\n",
        );
        let mut tr = tracker(v0, t(0));
        tr.update(md(v1), t(60));
        tr.update(md(v2), t(180));
        let items = tr.menu_items(t(5 * 60));
        let v1_id = id_of(&tr, WaypointKind::Reload, v1);
        let gap = items.iter().find(|i| i.id == v1_id).unwrap();
        let v1_to_v2 = groups_between(v1, v2);
        assert_eq!(gap.change_count, v1_to_v2);
        assert_ne!(v1_to_v2, groups_between(v0, v2));
    }

    #[test]
    fn ast_equivalent_save_skipped() {
        let v0 = "# Title\n\nP1.\n";
        let v1 = "# Title\n\nP1.\n\nP2.\n";
        let v2 = "# Title\n\nP1.\n\n\nP2.\n";
        let v3 = "# Title\n\nP1.\n\n\nP2.\n\nP3.\n";
        assert_eq!(groups_between(v1, v2), 0);

        let mut tr = tracker(v0, t(0));
        assert!(tr.update(md(v1), t(60)));
        assert!(!tr.update(md(v2), t(120)));
        assert!(tr.update(md(v3), t(180)));
        assert!(!tr.waypoints().iter().any(|w| &*w.markdown == v2));

        let items = tr.menu_items(t(240));
        let gaps = gap_items(&items);
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].id, id_of(&tr, WaypointKind::Reload, v1));
    }

    #[test]
    fn menu_hides_zero_change_gaps() {
        let mut tr = tracker("V1.\n", t(0));
        tr.update(md("V2.\n"), t(60));
        tr.update(md("V3.\n"), t(120));
        // Revert to V2: a duplicate, so no waypoint, but current is V2.
        assert!(!tr.update(md("V2.\n"), t(180)));
        let items = tr.menu_items(t(240));
        assert!(gap_items(&items).is_empty());

        assert!(texts(&tr, WaypointKind::Reload).contains(&"V3.\n".to_string()));
        tr.update(md("V4.\n"), t(300));
        let items = tr.menu_items(t(360));
        assert!(!gap_items(&items).is_empty());
    }

    #[test]
    fn gap_entry_selects_older_endpoint_as_active_waypoint() {
        let mut tr = tracker("V0.\n", t(0));
        tr.update(md("V1.\n"), t(60));
        tr.update(md("V2.\n"), t(180));
        let now = t(5 * 60);
        let v1_id = id_of(&tr, WaypointKind::Reload, "V1.\n");
        let items = tr.menu_items(now);
        let gap = items.iter().find(|i| i.id == v1_id).unwrap();
        tr.select_waypoint(Some(gap.id));
        assert_eq!(tr.selected_waypoint(), Some(v1_id));
        let active: Vec<u64> = tr
            .menu_items(now)
            .iter()
            .filter(|i| i.is_active)
            .map(|i| i.id)
            .collect();
        assert_eq!(active, vec![v1_id]);
    }
}

mod mode {
    use super::*;

    #[test]
    fn sidebar_changes_follow_mode() {
        let v1 = "Intro with a note.[^a]\n\n[^a]: Original.\n";
        let v2 = "Intro with a note.[^a]\n\n[^a]: Edited.\n";
        let up = crate::changes::compute_changes(v1, v2, DefinitionPolicy::SkipAll, 0.25);
        let down =
            crate::changes::compute_changes(v1, v2, DefinitionPolicy::DescendPlainFootnotes, 0.25);
        assert!(up.is_empty() && !down.is_empty());

        let mut tr = tracker(v1, t(0));
        // A definition-only edit is a real save (content policy descends).
        assert!(tr.update(md(v2), t(90)));
        assert_eq!(tr.mode(), TrackerMode::Up);
        assert!(tr.changes().is_empty());
        tr.set_mode(TrackerMode::Down);
        assert!(!tr.changes().is_empty());
        tr.set_mode(TrackerMode::Up);
        assert!(tr.changes().is_empty());
    }

    #[test]
    fn comment_only_edit_is_not_a_new_waypoint() {
        let plain = "The quick fox.\n";
        let commented = "The quick fox[^comment-a].\n\n[^comment-a]: A note.\n";
        let mut tr = tracker(plain, t(0));
        assert!(!tr.update(md(commented), t(90)));
        assert_eq!(tr.waypoints().len(), 1);
        for mode in [TrackerMode::Up, TrackerMode::Down] {
            tr.set_mode(mode);
            assert!(tr.changes().is_empty());
        }
    }
}

mod render_waypoint {
    use super::*;

    #[test]
    fn none_when_disabled() {
        let mut tr = tracker("V1.\n", t(0));
        tr.update(md("V2.\n"), t(90));
        assert!(tr.render_waypoint(false, false, 25).is_none());
    }

    #[test]
    fn none_when_there_are_no_changes() {
        let tr = tracker("V1.\n", t(0));
        assert!(tr.render_waypoint(true, false, 25).is_none());
        assert!(ChangeTracker::new()
            .render_waypoint(true, false, 25)
            .is_none());
    }

    #[test]
    fn some_against_the_active_waypoint() {
        let mut tr = tracker("V1.\n", t(0));
        tr.update(md("V2.\n"), t(90));
        let w = tr.render_waypoint(true, true, 40).unwrap();
        assert_eq!(&*w.old_markdown, "V1.\n");
        assert!(w.show_inline_deletions);
        assert_eq!(w.word_diff_threshold_pct, 40);
        // After accept there is nothing to show.
        tr.accept(t(90));
        assert!(tr.render_waypoint(true, true, 40).is_none());
    }

    #[test]
    fn follows_the_mode_policy() {
        let v1 = "Intro.[^a]\n\n[^a]: Original.\n";
        let v2 = "Intro.[^a]\n\n[^a]: Edited.\n";
        let mut tr = tracker(v1, t(0));
        tr.update(md(v2), t(90));
        assert!(tr.render_waypoint(true, false, 25).is_none());
        tr.set_mode(TrackerMode::Down);
        assert!(tr.render_waypoint(true, false, 25).is_some());
    }
}

mod cache {
    use super::*;

    #[test]
    fn menu_items_are_stable_across_calls() {
        let mut tr = tracker("V1.\n", t(0));
        tr.update(md("V2.\n"), t(90));
        let a = tr.menu_items(t(90));
        assert!(tr.cached_summary_count() > 0);
        let b = tr.menu_items(t(90));
        assert_eq!(a, b);
    }

    #[test]
    fn update_invalidates_cache() {
        let mut tr = tracker("V1.\n", t(0));
        tr.update(md("V2.\n"), t(90));
        let before = tr.menu_items(t(90));
        assert_eq!(before[0].change_count, 1);
        tr.update(md("V2.\n\nMore.\n"), t(180));
        assert_eq!(tr.cached_summary_count(), 0);
        let after = tr.menu_items(t(180));
        assert!(!after.is_empty());
        assert_eq!(after[0].label, "since document opened");
        assert_eq!(
            after[0].change_count,
            groups_between("V1.\n", "V2.\n\nMore.\n")
        );
    }

    #[test]
    fn redundant_update_still_invalidates_cache() {
        let mut tr = tracker("V1.\n", t(0));
        tr.update(md("V2.\n"), t(90));
        tr.menu_items(t(90));
        tr.update(md("V1.\n"), t(100));
        assert_eq!(tr.cached_summary_count(), 0);
        // Reverted to the initial: nothing since document opened.
        assert_eq!(tr.menu_items(t(100))[0].change_count, 0);
    }

    #[test]
    fn accept_invalidates_cache() {
        let mut tr = tracker("V1.\n", t(0));
        tr.update(md("V2.\n"), t(90));
        tr.menu_items(t(90));
        tr.accept(t(90));
        assert_eq!(tr.cached_summary_count(), 0);
        tr.update(md("V3.\n"), t(180));
        let items = tr.menu_items(t(180));
        assert_eq!(items[0].label, "since last accepted");
    }
}
