//! Up/Down mode toggle (Phase 10.2 of `docs/IMPLEMENTATION-PLAN.md`): a
//! trivial two-state flip, but still extracted and unit-tested per the
//! plan's explicit instruction, rather than inlined in the GTK
//! key-press-event handler that calls it (`window.rs`).

use mudl_server::routes::Mode;

pub fn next_mode(current: Mode) -> Mode {
    match current {
        Mode::Up => Mode::Down,
        Mode::Down => Mode::Up,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn up_toggles_to_down() {
        assert_eq!(next_mode(Mode::Up), Mode::Down);
    }

    #[test]
    fn down_toggles_to_up() {
        assert_eq!(next_mode(Mode::Down), Mode::Up);
    }
}
