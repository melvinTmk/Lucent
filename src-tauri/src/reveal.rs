//! Pure reveal state machine for one dock: the cursor resting on the edge strip shows the
//! dock after `reveal_delay`, leaving it hides the dock after `hide_delay`.
//! `suppressed` is reserved for Game Mode (M3): it forces a hide and blocks any show.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RevealState {
    Hidden,
    Arming { since_ms: u64 },
    Shown,
    Disarming { since_ms: u64 },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RevealAction {
    Show,
    Hide,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct RevealInput {
    pub now_ms: u64,
    pub cursor_in_trigger: bool,
    pub cursor_in_dock: bool,
    pub hotkey_toggle: bool,
    pub suppressed: bool,
    pub peek: bool,
    /// An OS file drag hovers the dock window: counts as the cursor being in the dock.
    pub drag_over: bool,
    /// « Toujours visible »: shown unless suppressed (fullscreen app, focused game).
    pub pinned: bool,
}

pub fn step(
    s: RevealState,
    i: &RevealInput,
    reveal_delay: u64,
    hide_delay: u64,
) -> (RevealState, Option<RevealAction>) {
    use RevealAction::{Hide, Show};
    use RevealState::*;
    let visible = matches!(s, Shown | Disarming { .. });
    if i.suppressed {
        return (Hidden, visible.then_some(Hide));
    }
    if i.pinned || i.peek {
        return (Shown, (!visible).then_some(Show));
    }
    if i.hotkey_toggle {
        return if visible { (Hidden, Some(Hide)) } else { (Shown, Some(Show)) };
    }
    let near = i.cursor_in_trigger || i.cursor_in_dock || i.drag_over;
    let elapsed = |since: u64| i.now_ms.saturating_sub(since);
    match s {
        Hidden if i.cursor_in_trigger => {
            if reveal_delay == 0 { (Shown, Some(Show)) } else { (Arming { since_ms: i.now_ms }, None) }
        }
        Hidden => (Hidden, None),
        Arming { .. } if !near => (Hidden, None),
        Arming { since_ms } if elapsed(since_ms) >= reveal_delay => (Shown, Some(Show)),
        Arming { .. } => (s, None),
        Shown if near => (Shown, None),
        Shown if hide_delay == 0 => (Hidden, Some(Hide)),
        Shown => (Disarming { since_ms: i.now_ms }, None),
        Disarming { .. } if near => (Shown, None),
        Disarming { since_ms } if elapsed(since_ms) >= hide_delay => (Hidden, Some(Hide)),
        Disarming { .. } => (s, None),
    }
}

#[cfg(test)]
mod tests {
    use super::RevealAction::{Hide, Show};
    use super::RevealState::*;
    use super::*;

    #[test]
    fn pinned_stays_shown_until_suppressed() {
        let pinned = |now_ms| RevealInput { now_ms, pinned: true, ..Default::default() };
        assert_eq!(step(Hidden, &pinned(0), 150, 400), (Shown, Some(RevealAction::Show)));
        assert_eq!(step(Shown, &pinned(10_000), 150, 400), (Shown, None));
        let full = RevealInput { now_ms: 20_000, pinned: true, suppressed: true, ..Default::default() };
        assert_eq!(step(Shown, &full, 150, 400), (Hidden, Some(RevealAction::Hide)));
        // The fullscreen app goes away: back at once.
        assert_eq!(step(Hidden, &pinned(30_000), 150, 400), (Shown, Some(RevealAction::Show)));
    }

    const REVEAL: u64 = 150;
    const HIDE: u64 = 400;

    fn at(now_ms: u64) -> RevealInput {
        RevealInput { now_ms, ..Default::default() }
    }
    fn trig(now_ms: u64) -> RevealInput {
        RevealInput { now_ms, cursor_in_trigger: true, ..Default::default() }
    }
    fn dock(now_ms: u64) -> RevealInput {
        RevealInput { now_ms, cursor_in_dock: true, ..Default::default() }
    }
    fn run(s: RevealState, i: RevealInput) -> (RevealState, Option<RevealAction>) {
        step(s, &i, REVEAL, HIDE)
    }

    #[test]
    fn trigger_shorter_than_delay_does_not_show() {
        let (s, a) = run(Hidden, trig(1000));
        assert_eq!((s, a), (Arming { since_ms: 1000 }, None));
        let (s, a) = run(s, trig(1100));
        assert_eq!((s, a), (Arming { since_ms: 1000 }, None));
        // Cursor leaves before 150 ms: back to hidden, nothing shown.
        assert_eq!(run(s, at(1120)), (Hidden, None));
    }

    #[test]
    fn trigger_held_past_delay_shows() {
        let (s, _) = run(Hidden, trig(1000));
        assert_eq!(run(s, trig(1150)), (Shown, Some(Show)));
    }

    #[test]
    fn zero_reveal_delay_shows_at_once() {
        assert_eq!(step(Hidden, &trig(5), 0, HIDE), (Shown, Some(Show)));
    }

    #[test]
    fn drag_over_keeps_the_dock_shown_and_cancels_hiding() {
        let drag = |now_ms| RevealInput { now_ms, drag_over: true, ..Default::default() };
        assert_eq!(run(Shown, drag(2000)), (Shown, None));
        assert_eq!(run(Disarming { since_ms: 2000 }, drag(2100)), (Shown, None));
        // Without the drag, the same state hides once the delay has passed.
        assert_eq!(run(Disarming { since_ms: 2000 }, at(2400)), (Hidden, Some(Hide)));
    }

    #[test]
    fn staying_in_dock_keeps_it_shown() {
        assert_eq!(run(Shown, dock(2000)), (Shown, None));
        assert_eq!(run(Shown, trig(2000)), (Shown, None));
    }

    #[test]
    fn leaving_dock_hides_after_hide_delay() {
        let (s, a) = run(Shown, at(2000));
        assert_eq!((s, a), (Disarming { since_ms: 2000 }, None));
        assert_eq!(run(s, at(2399)), (Disarming { since_ms: 2000 }, None));
        assert_eq!(run(s, at(2400)), (Hidden, Some(Hide)));
    }

    #[test]
    fn reentering_during_disarm_cancels_hide() {
        let (s, _) = run(Shown, at(2000));
        assert_eq!(run(s, dock(2200)), (Shown, None));
    }

    #[test]
    fn hotkey_toggles_immediately() {
        let hk = |now_ms| RevealInput { now_ms, hotkey_toggle: true, ..Default::default() };
        assert_eq!(run(Hidden, hk(10)), (Shown, Some(Show)));
        assert_eq!(run(Arming { since_ms: 5 }, hk(10)), (Shown, Some(Show)));
        assert_eq!(run(Shown, hk(20)), (Hidden, Some(Hide)));
        assert_eq!(run(Disarming { since_ms: 15 }, hk(20)), (Hidden, Some(Hide)));
    }

    #[test]
    fn suppressed_forces_hide_and_blocks_show() {
        let sup = |mut i: RevealInput| {
            i.suppressed = true;
            i
        };
        assert_eq!(run(Shown, sup(dock(10))), (Hidden, Some(Hide)));
        assert_eq!(run(Disarming { since_ms: 1 }, sup(at(10))), (Hidden, Some(Hide)));
        assert_eq!(run(Arming { since_ms: 0 }, sup(trig(1000))), (Hidden, None));
        assert_eq!(run(Hidden, sup(trig(1000))), (Hidden, None));
        let mut hk = sup(at(10));
        hk.hotkey_toggle = true;
        assert_eq!(run(Hidden, hk), (Hidden, None));
    }

    #[test]
    fn peek_forces_show_unless_suppressed() {
        let pk = RevealInput { peek: true, ..Default::default() };
        assert_eq!(run(Hidden, pk), (Shown, Some(Show)));
        assert_eq!(run(Arming { since_ms: 1 }, pk), (Shown, Some(Show)));
        assert_eq!(run(Shown, pk), (Shown, None));
        assert_eq!(run(Disarming { since_ms: 10 }, pk), (Shown, None));

        let sup_pk = RevealInput { peek: true, suppressed: true, ..Default::default() };
        assert_eq!(run(Hidden, sup_pk), (Hidden, None));
        assert_eq!(run(Shown, sup_pk), (Hidden, Some(Hide)));
    }
}
