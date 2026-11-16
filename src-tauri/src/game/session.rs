//! Game session reducer: pure. Inputs come from the service (foreground changes, the
//! process list, prompt answers); effects tell it what to emit.
use super::library::Class;
use crate::settings::GameModeSettings;
use serde::Serialize;
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    /// The foreground window changed (or was re-polled). `exe` is lowercased, empty when
    /// unknown; `class` was computed by the caller from the libraries and include/exclude.
    Foreground { pid: u32, exe: String, monitor_id: String, fullscreen: bool, class: Class },
    /// Pids alive right now (process snapshot).
    Running(Vec<u32> /*pids alive*/),
    Answer { exe: String, is_game: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// A game session starts.
    Enter { name: String, monitor_id: String },
    /// The active game moved to another monitor.
    Moved { monitor_id: String },
    Exit,
    Focus(bool),
    Ask { exe: String, name: String },
}

/// `game://state` payload.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameStateDto {
    pub active: bool,
    pub name: Option<String>,
    pub monitor_id: Option<String>,
    pub focused: bool,
}

impl GameStateDto {
    /// The game is active, focused and shown on monitor `monitor_id`: that monitor is
    /// the game's (no dock, no search hotkey there).
    pub fn owns(&self, monitor_id: &str) -> bool {
        self.active && self.focused && self.monitor_id.as_deref() == Some(monitor_id)
    }
}

struct Active {
    exe: String,
    /// Every pid of the game's exe seen in the foreground; the session ends when none is alive.
    pids: Vec<u32>,
    name: String,
    monitor_id: String,
}

struct Foreground {
    pid: u32,
    exe: String,
    monitor_id: String,
}

pub struct Session {
    enabled: bool,
    namer: Box<dyn Fn(&str) -> String + Send>,
    active: Option<Active>,
    focused: bool,
    /// Exes already asked about during this run.
    asked: HashSet<String>,
    last_fg: Option<Foreground>,
}

impl Session {
    /// `namer`: display name of a lowercased exe path.
    pub fn new(enabled: bool, namer: impl Fn(&str) -> String + Send + 'static) -> Self {
        Self { enabled, namer: Box::new(namer), active: None, focused: false, asked: HashSet::new(), last_fg: None }
    }

    pub fn step(&mut self, i: Input) -> Vec<Effect> {
        match i {
            Input::Foreground { pid, exe, monitor_id, fullscreen, class } => {
                let out = self.on_foreground(pid, &exe, &monitor_id, fullscreen, class);
                self.last_fg = Some(Foreground { pid, exe, monitor_id });
                out
            }
            Input::Running(alive) => {
                let Some(a) = self.active.as_mut() else { return Vec::new() };
                a.pids.retain(|p| alive.contains(p));
                if a.pids.is_empty() { self.exit() } else { Vec::new() }
            }
            Input::Answer { exe, is_game } => {
                self.asked.insert(exe.clone());
                if !is_game || !self.enabled || self.active.is_some() {
                    return Vec::new();
                }
                match self.last_fg.as_ref().filter(|f| f.exe == exe) {
                    Some(f) => {
                        let (pid, mon) = (f.pid, f.monitor_id.clone());
                        self.enter(pid, &exe, &mon)
                    }
                    None => Vec::new(),
                }
            }
        }
    }

    fn on_foreground(&mut self, pid: u32, exe: &str, monitor_id: &str, fullscreen: bool, class: Class) -> Vec<Effect> {
        let mut out = Vec::new();
        if let Some(a) = self.active.as_mut() {
            let is_game = a.pids.contains(&pid) || (!exe.is_empty() && a.exe == exe);
            if is_game {
                if !a.pids.contains(&pid) {
                    a.pids.push(pid);
                }
                if a.monitor_id != monitor_id {
                    a.monitor_id = monitor_id.to_string();
                    out.push(Effect::Moved { monitor_id: a.monitor_id.clone() });
                }
                if !self.focused {
                    self.focused = true;
                    out.push(Effect::Focus(true));
                }
                return out;
            }
            if !matches!(class, Class::Game(_)) || !self.enabled {
                if self.focused {
                    self.focused = false;
                    out.push(Effect::Focus(false));
                }
                return out;
            }
            // Another game took the foreground: its session replaces this one.
            out.extend(self.exit());
        }
        if !self.enabled || exe.is_empty() {
            return out;
        }
        match class {
            Class::Game(_) => out.extend(self.enter(pid, exe, monitor_id)),
            Class::Unknown if fullscreen && self.asked.insert(exe.to_string()) => {
                out.push(Effect::Ask { exe: exe.to_string(), name: (self.namer)(exe) });
            }
            _ => {}
        }
        out
    }

    fn enter(&mut self, pid: u32, exe: &str, monitor_id: &str) -> Vec<Effect> {
        let name = (self.namer)(exe);
        self.active = Some(Active { exe: exe.to_string(), pids: vec![pid], name: name.clone(), monitor_id: monitor_id.to_string() });
        self.focused = true;
        vec![Effect::Enter { name, monitor_id: monitor_id.to_string() }, Effect::Focus(true)]
    }

    fn exit(&mut self) -> Vec<Effect> {
        if self.active.take().is_none() {
            return Vec::new();
        }
        self.focused = false;
        vec![Effect::Exit]
    }

    /// Settings toggle. Turning game mode off ends the running session.
    pub fn set_enabled(&mut self, enabled: bool) -> Vec<Effect> {
        self.enabled = enabled;
        if enabled { Vec::new() } else { self.exit() }
    }

    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    pub fn active_exe(&self) -> Option<&str> {
        self.active.as_ref().map(|a| a.exe.as_str())
    }

    pub fn state(&self) -> GameStateDto {
        match &self.active {
            Some(a) => GameStateDto {
                active: true,
                name: Some(a.name.clone()),
                monitor_id: Some(a.monitor_id.clone()),
                focused: self.focused,
            },
            None => GameStateDto::default(),
        }
    }
}

/// Records a prompt answer in the settings: a game goes to `include` (and leaves
/// `exclude`), a non-game the other way round.
pub fn apply_answer(gm: &mut GameModeSettings, exe: &str, is_game: bool) {
    let exe = exe.trim().to_lowercase();
    if exe.is_empty() {
        return;
    }
    let (add, remove) = if is_game { (&mut gm.include, &mut gm.exclude) } else { (&mut gm.exclude, &mut gm.include) };
    remove.retain(|e| *e != exe);
    if !add.contains(&exe) {
        add.push(exe);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::library::Source;

    const GAME: &str = r"c:\riot games\valorant\live\valorant.exe";
    const BRAVE: &str = r"c:\program files\brave\brave.exe";
    const ODD: &str = r"c:\tools\odd.exe";

    fn session() -> Session {
        Session::new(true, |exe: &str| format!("name:{exe}"))
    }
    fn fg(pid: u32, exe: &str, mon: &str, fullscreen: bool, class: Class) -> Input {
        Input::Foreground { pid, exe: exe.into(), monitor_id: mon.into(), fullscreen, class }
    }
    fn game(mon: &str) -> Input {
        fg(10, GAME, mon, true, Class::Game(Source::Riot))
    }
    fn brave() -> Input {
        fg(20, BRAVE, "M1", false, Class::NotGame)
    }
    fn enter(mon: &str) -> Effect {
        Effect::Enter { name: format!("name:{GAME}"), monitor_id: mon.into() }
    }

    #[test]
    fn enters_on_game_foreground() {
        let mut s = session();
        assert_eq!(s.step(game("M1")), vec![enter("M1"), Effect::Focus(true)]);
        assert_eq!(
            s.state(),
            GameStateDto { active: true, name: Some(format!("name:{GAME}")), monitor_id: Some("M1".into()), focused: true }
        );
        // Same foreground again: nothing new.
        assert_eq!(s.step(game("M1")), vec![]);
    }

    #[test]
    fn alt_tab_unfocuses_but_stays_active_then_refocuses() {
        let mut s = session();
        s.step(game("M1"));
        assert_eq!(s.step(brave()), vec![Effect::Focus(false)]);
        assert!(s.state().active);
        assert!(!s.state().focused);
        assert_eq!(s.step(brave()), vec![]);
        assert_eq!(s.step(game("M1")), vec![Effect::Focus(true)]);
        assert!(s.state().focused);
    }

    #[test]
    fn exits_when_game_process_is_gone() {
        let mut s = session();
        s.step(game("M1"));
        assert_eq!(s.step(Input::Running(vec![1, 10, 20])), vec![]);
        assert_eq!(s.step(Input::Running(vec![1, 20])), vec![Effect::Exit]);
        assert_eq!(s.state(), GameStateDto::default());
        assert_eq!(s.step(Input::Running(vec![1])), vec![]);
    }

    #[test]
    fn unknown_fullscreen_asks_once_per_exe() {
        let mut s = session();
        let odd = || fg(30, ODD, "M1", true, Class::Unknown);
        assert_eq!(s.step(odd()), vec![Effect::Ask { exe: ODD.into(), name: format!("name:{ODD}") }]);
        s.step(brave());
        assert_eq!(s.step(odd()), vec![]);
        assert!(!s.state().active);
    }

    #[test]
    fn unknown_windowed_or_excluded_never_asks() {
        let mut s = session();
        assert_eq!(s.step(fg(30, ODD, "M1", false, Class::Unknown)), vec![]);
        // Excluded exes classify as NotGame.
        assert_eq!(s.step(fg(30, ODD, "M1", true, Class::NotGame)), vec![]);
        // Unknown identity (elevated process): never asks.
        assert_eq!(s.step(fg(31, "", "M1", true, Class::Unknown)), vec![]);
    }

    #[test]
    fn answer_yes_enters_if_foreground() {
        let mut s = session();
        s.step(fg(30, ODD, "M2", true, Class::Unknown));
        let e = s.step(Input::Answer { exe: ODD.into(), is_game: true });
        assert_eq!(e, vec![Effect::Enter { name: format!("name:{ODD}"), monitor_id: "M2".into() }, Effect::Focus(true)]);
        assert_eq!(s.step(Input::Running(vec![30])), vec![]);
        assert_eq!(s.step(Input::Running(vec![])), vec![Effect::Exit]);
    }

    #[test]
    fn answer_for_background_exe_or_no_does_not_enter() {
        let mut s = session();
        s.step(fg(30, ODD, "M1", true, Class::Unknown));
        s.step(brave());
        assert_eq!(s.step(Input::Answer { exe: ODD.into(), is_game: true }), vec![]);
        s.step(fg(30, ODD, "M1", true, Class::Unknown));
        assert_eq!(s.step(Input::Answer { exe: ODD.into(), is_game: false }), vec![]);
        assert!(!s.state().active);
    }

    #[test]
    fn disabled_does_nothing() {
        let mut s = Session::new(false, |e: &str| e.to_string());
        assert_eq!(s.step(game("M1")), vec![]);
        assert_eq!(s.step(fg(30, ODD, "M1", true, Class::Unknown)), vec![]);
        assert_eq!(s.step(Input::Answer { exe: ODD.into(), is_game: true }), vec![]);
        assert!(!s.state().active);
    }

    #[test]
    fn disabling_while_active_exits() {
        let mut s = session();
        s.step(game("M1"));
        assert_eq!(s.set_enabled(false), vec![Effect::Exit]);
        assert!(!s.state().active);
        assert_eq!(s.set_enabled(true), vec![]);
    }

    #[test]
    fn monitor_change_re_emits() {
        let mut s = session();
        s.step(game("M1"));
        assert_eq!(s.step(game("M2")), vec![Effect::Moved { monitor_id: "M2".into() }]);
        assert_eq!(s.state().monitor_id.as_deref(), Some("M2"));
        // Moved while unfocused, then focused again on the new monitor.
        s.step(brave());
        assert_eq!(s.step(game("M1")), vec![Effect::Moved { monitor_id: "M1".into() }, Effect::Focus(true)]);
    }

    #[test]
    fn another_process_of_the_same_game_counts_as_the_game() {
        let mut s = session();
        s.step(game("M1"));
        s.step(brave());
        assert_eq!(s.step(fg(11, GAME, "M1", true, Class::Game(Source::Riot))), vec![Effect::Focus(true)]);
        // Exit only once every game pid is gone.
        assert_eq!(s.step(Input::Running(vec![11])), vec![]);
        assert_eq!(s.step(Input::Running(vec![])), vec![Effect::Exit]);
    }

    #[test]
    fn switching_to_another_game_restarts_the_session() {
        let mut s = session();
        s.step(game("M1"));
        let other = r"c:\games\other\other.exe";
        let e = s.step(fg(40, other, "M1", true, Class::Game(Source::Steam)));
        assert_eq!(e, vec![Effect::Exit, Effect::Enter { name: format!("name:{other}"), monitor_id: "M1".into() }, Effect::Focus(true)]);
    }

    #[test]
    fn apply_answer_moves_exe_between_lists() {
        let mut gm = GameModeSettings::default();
        apply_answer(&mut gm, r"C:\Tools\Odd.exe ", true);
        assert_eq!(gm.include, vec![ODD.to_string()]);
        apply_answer(&mut gm, ODD, true);
        assert_eq!(gm.include.len(), 1);
        apply_answer(&mut gm, ODD, false);
        assert!(gm.include.is_empty());
        assert_eq!(gm.exclude, vec![ODD.to_string()]);
    }

    #[test]
    fn refed_foreground_enters_once_reclassified_as_game() {
        // Startup: libraries not discovered yet, the game already in the foreground looks
        // unknown (and the service never asks before discovery). Libraries arrive, the
        // service re-feeds the same foreground: the session starts.
        let mut s = session();
        assert_eq!(s.step(fg(10, GAME, "M1", false, Class::Unknown)), vec![]);
        assert_eq!(s.step(game("M1")), vec![enter("M1"), Effect::Focus(true)]);
    }

    #[test]
    fn refed_foreground_after_reenable_enters_and_never_reasks() {
        let mut s = session();
        s.set_enabled(false);
        assert_eq!(s.step(game("M1")), vec![]);
        s.set_enabled(true);
        assert_eq!(s.step(game("M1")), vec![enter("M1"), Effect::Focus(true)]);
        // A re-check of an already-asked unknown fullscreen exe stays silent.
        let mut s = session();
        let odd = || fg(30, ODD, "M1", true, Class::Unknown);
        assert_eq!(s.step(odd()).len(), 1);
        assert_eq!(s.step(odd()), vec![]);
    }
}
