use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Ranked and HeroLabs reserved for future detection
pub enum MatchMode {
    Unknown,
    Standard,
    Ranked,
    StreetBrawl,
    BotMatch,
    TrainingRange,
    HeroLabs,
}

impl MatchMode {
    pub fn display(self) -> &'static str {
        match self {
            MatchMode::Unknown => "In Match",
            MatchMode::Standard => "Standard Match",
            MatchMode::Ranked => "Ranked Match",
            MatchMode::StreetBrawl => "Street Brawl",
            MatchMode::BotMatch => "Bot Match",
            MatchMode::TrainingRange => "Training Range",
            MatchMode::HeroLabs => "Hero Labs",
        }
    }

    pub fn show_map_location(self) -> bool {
        matches!(self, MatchMode::Standard | MatchMode::Ranked | MatchMode::StreetBrawl | MatchMode::Unknown)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GamePhase {
    NotRunning,
    // The game process is alive but no usable log data exists — e.g. the game was
    // started outside the app without -condebug. Shows a generic "In Game" presence.
    Running,
    MainMenu,
    Hideout,
    InQueue,
    MatchIntro,
    InMatch,
    PostMatch,
    Spectating,
}

impl GamePhase {
    pub fn description(self) -> &'static str {
        match self {
            GamePhase::NotRunning => "Not Running",
            GamePhase::Running => "In Game",
            GamePhase::MainMenu => "Main Menu",
            GamePhase::Hideout => "Hideout",
            GamePhase::InQueue => "Searching for Match",
            GamePhase::MatchIntro => "Match Starting",
            GamePhase::InMatch => "In Match",
            GamePhase::PostMatch => "Post Match",
            GamePhase::Spectating => "Spectating",
        }
    }

    // Whether the hero image and "Playing as" label should be shown for this phase.
    pub fn shows_hero(self) -> bool {
        !matches!(
            self,
            GamePhase::NotRunning
                | GamePhase::Running
                | GamePhase::MainMenu
                | GamePhase::PostMatch
                | GamePhase::Spectating
        )
    }
}

// Renders a match duration as a clock: "07:42", or "1:07:42" once past an hour.
// The value is computed at push time, so it is exact when sent and drifts by at most
// one presence refresh (see MIN_TIMER_REFRESH_SECS in main.rs) before the next push.
pub fn format_match_elapsed(d: std::time::Duration) -> String {
    let total = d.as_secs();
    let (hours, mins, secs) = (total / 3600, (total % 3600) / 60, total % 60);
    if hours > 0 {
        format!("{}:{:02}:{:02}", hours, mins, secs)
    } else {
        format!("{:02}:{:02}", mins, secs)
    }
}

pub struct GameState {
    pub phase: GamePhase,
    pub match_mode: MatchMode,
    pub hero_key: Option<String>,
    pub map_name: Option<String>,
    pub party_size: u8,
    // Set when the match actually starts, cleared whenever we leave the InMatch phase.
    pub match_started_at: Option<std::time::Instant>,

    // internal tracking
    pub(crate) hero_window_open: bool,
    pub(crate) hideout_loaded: bool,
    pub(crate) local_account_id: Option<u64>,
    pub(crate) party_id: Option<u64>,
    pub(crate) party_members: HashSet<u64>,
    pub(crate) pending_player_count: u32,
    // Last observation from the process watcher. Survives reset() so stale log
    // shutdown events can't mark a live game as not running.
    pub(crate) process_alive: bool,
}

impl GameState {
    pub fn new() -> Self {
        Self {
            phase: GamePhase::NotRunning,
            match_mode: MatchMode::Unknown,
            hero_key: None,
            map_name: None,
            party_size: 1,
            match_started_at: None,
            hero_window_open: true,
            hideout_loaded: false,
            local_account_id: None,
            party_id: None,
            party_members: HashSet::new(),
            pending_player_count: 0,
            process_alive: false,
        }
    }

    pub fn reset(&mut self) {
        let process_alive = self.process_alive;
        *self = Self::new();
        self.process_alive = process_alive;
        // A reset normally means the game closed, but if the process is still
        // alive the shutdown signal came from stale log data (previous session).
        // Keep showing the generic In Game presence instead of Not Running.
        if process_alive {
            self.phase = GamePhase::Running;
        }
    }

    // Feeds process watcher observations into the state machine.
    //
    // The log watcher owns detailed phase tracking; this covers the gap where the
    // game runs without usable log data (started from Steam without -condebug, so
    // no restart is needed just to be detected):
    // - process alive with no known phase → generic In Game presence
    // - process gone → full reset back to Not Running
    //
    // Never overrides a phase the log watcher has already established.
    pub fn apply_process_signal(&mut self, alive: bool) {
        self.process_alive = alive;
        if alive {
            if self.phase == GamePhase::NotRunning {
                self.phase = GamePhase::Running;
            }
        } else if self.phase != GamePhase::NotRunning {
            self.reset();
        }
    }

    pub fn enter_hideout(&mut self) {
        self.phase = GamePhase::Hideout;
        self.match_started_at = None;
        self.hero_key = None;
        self.hero_window_open = true;
        self.hideout_loaded = false;
    }

    pub(crate) fn clear_party(&mut self) {
        self.party_id = None;
        self.party_members.clear();
        self.party_size = 1;
    }

    pub(crate) fn apply_party_event(&mut self, party_id: u64, event_name: &str, account_id: u64) {
        let ev = event_name.to_lowercase();
        if ev.contains("joinedparty") {
            let local = self.local_account_id.unwrap_or(u64::MAX);
            if account_id == local {
                self.party_id = Some(party_id);
                self.party_members = std::iter::once(account_id).collect();
            } else if self.party_id != Some(party_id) {
                self.party_id = Some(party_id);
                self.party_members.clear();
            }
            self.party_members.insert(account_id);
            self.party_size = (self.party_members.len() as u8).max(2);
        } else if ev.contains("leftparty")
            || ev.contains("removedfromparty")
            || ev.contains("kickedfromparty")
        {
            if account_id == self.local_account_id.unwrap_or(u64::MAX) {
                self.clear_party();
            } else {
                self.party_members.remove(&account_id);
                self.party_size = (self.party_members.len() as u8).max(1);
            }
        } else if ev.contains("disband") {
            self.clear_party();
        }
    }

    pub fn enter_main_menu(&mut self) {
        self.phase = GamePhase::MainMenu;
        self.match_started_at = None;
    }

    pub fn enter_queue(&mut self) {
        self.phase = GamePhase::InQueue;
        self.match_started_at = None;
    }

    pub fn leave_queue(&mut self) {
        self.phase = GamePhase::Hideout;
    }

    pub fn enter_match_intro(&mut self) {
        self.phase = GamePhase::MatchIntro;
        self.match_started_at = None;
        self.match_mode = MatchMode::Unknown;
        self.hero_key = None;
        self.hero_window_open = true;
        self.pending_player_count = 0;
    }

    pub fn start_match(&mut self) {
        // Guard against repeated "game in progress" signals restarting the clock.
        if self.phase != GamePhase::InMatch || self.match_started_at.is_none() {
            self.match_started_at = Some(std::time::Instant::now());
        }
        self.phase = GamePhase::InMatch;
    }

    pub fn end_match(&mut self) {
        self.phase = GamePhase::PostMatch;
        self.match_started_at = None;
    }

    pub fn enter_spectating(&mut self) {
        self.phase = GamePhase::Spectating;
        self.match_started_at = None;
    }

    // How long the current match has been running, or None outside of an active match.
    pub fn match_elapsed(&self) -> Option<std::time::Duration> {
        if self.phase != GamePhase::InMatch {
            return None;
        }
        self.match_started_at.map(|t| t.elapsed())
    }

    pub fn prepare_match_hero_tracking(&mut self) {
        self.hero_key = None;
        self.hero_window_open = true;
    }

    // Returns the Discord presence status line for the current phase.
    //
    // - `hideout_text`: hero-specific text from the API (takes priority in Hideout phase).
    // - `hero_name`: display name of the current hero (used as `{hero}` variable).
    // - `cfg`: per-phase string templates from the loaded config.
    // - `timer`: formatted match duration (used as `{timer}`, appended if the template omits it).
    pub fn presence_status(
        &self,
        hideout_text: Option<&str>,
        hero_name: Option<&str>,
        timer: Option<&str>,
        cfg: &crate::config::StatusStrings,
    ) -> String {
        use crate::config::apply_vars;
        match self.phase {
            GamePhase::NotRunning => cfg.game_not_running.clone(),
            GamePhase::Running => cfg.in_game.clone(),
            GamePhase::MainMenu => cfg.in_main_menu.clone(),
            GamePhase::Hideout => {
                if let Some(text) = hideout_text.filter(|t| !t.is_empty()) {
                    text.to_string()
                } else {
                    apply_vars(&cfg.in_hideout, &[("hero", hero_name.unwrap_or(""))])
                }
            }
            GamePhase::InQueue => cfg.in_matchmaking.clone(),
            GamePhase::MatchIntro => {
                apply_vars(&cfg.loading_into_match, &[("mode", self.match_mode.display())])
            }
            GamePhase::InMatch => {
                let mode = self.match_mode.display();
                let uses_template = self.match_mode.show_map_location();
                let mut status = if uses_template {
                    apply_vars(
                        &cfg.in_match,
                        &[
                            ("mode", mode),
                            ("location", &cfg.match_location_label),
                            ("timer", timer.unwrap_or("")),
                        ],
                    )
                } else {
                    mode.to_string()
                };
                // Templates written before {timer} existed still get the timer, appended.
                let placed = uses_template && cfg.in_match.contains("{timer}");
                if let Some(t) = timer.filter(|_| !placed) {
                    status = format!("{status} - {t}");
                }
                status
            }
            GamePhase::PostMatch => cfg.post_match.clone(),
            GamePhase::Spectating => cfg.spectating.clone(),
        }
    }

    pub fn apply_hero_signal(&mut self, hero_key: &str) {
        if self.phase == GamePhase::Spectating {
            return;
        }

        match self.phase {
            GamePhase::MatchIntro | GamePhase::InMatch => {
                let free_swap = matches!(self.match_mode, MatchMode::TrainingRange | MatchMode::HeroLabs);
                if !free_swap {
                    if let Some(ref current) = self.hero_key {
                        if hero_key != current.as_str() {
                            return; // hero locked in, ignore a different hero
                        }
                    } else if !self.hero_window_open {
                        return;
                    }
                }
                self.hero_key = Some(hero_key.to_string());
                self.hero_window_open = false;
            }
            GamePhase::Hideout => {
                self.hero_key = Some(hero_key.to_string());
            }
            GamePhase::MainMenu | GamePhase::PostMatch => {}
            _ => {
                self.hero_key = Some(hero_key.to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_signal_promotes_not_running_to_running() {
        let mut gs = GameState::new();
        gs.apply_process_signal(true);
        assert_eq!(gs.phase, GamePhase::Running);
    }

    #[test]
    fn process_signal_never_overrides_log_derived_phase() {
        let mut gs = GameState::new();
        gs.start_match();
        gs.apply_process_signal(true);
        assert_eq!(gs.phase, GamePhase::InMatch);
    }

    #[test]
    fn process_gone_resets_to_not_running() {
        let mut gs = GameState::new();
        gs.apply_process_signal(true);
        gs.apply_process_signal(false);
        assert_eq!(gs.phase, GamePhase::NotRunning);
    }

    #[test]
    fn process_gone_clears_stale_log_phase() {
        // A crashed session can leave the log resync stuck on an in-game phase.
        let mut gs = GameState::new();
        gs.start_match();
        gs.hero_key = Some("hero_dynamo".to_string());
        gs.apply_process_signal(false);
        assert_eq!(gs.phase, GamePhase::NotRunning);
        assert_eq!(gs.hero_key, None);
    }

    #[test]
    fn process_signal_is_noop_when_nothing_runs() {
        let mut gs = GameState::new();
        gs.apply_process_signal(false);
        assert_eq!(gs.phase, GamePhase::NotRunning);
    }

    #[test]
    fn reset_keeps_running_phase_while_process_alive() {
        // Stale log data replaying a shutdown must not mark a live game as closed.
        let mut gs = GameState::new();
        gs.apply_process_signal(true);
        gs.start_match();
        gs.reset();
        assert_eq!(gs.phase, GamePhase::Running);
    }

    #[test]
    fn reset_goes_to_not_running_without_process() {
        let mut gs = GameState::new();
        gs.start_match();
        gs.reset();
        assert_eq!(gs.phase, GamePhase::NotRunning);
    }

    #[test]
    fn running_phase_hides_hero_and_uses_in_game_status() {
        let mut gs = GameState::new();
        gs.apply_process_signal(true);
        assert!(!gs.phase.shows_hero());
        let cfg = crate::config::StatusStrings::default();
        assert_eq!(gs.presence_status(None, None, &cfg), "In Game");
    }
}

