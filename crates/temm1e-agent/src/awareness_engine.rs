//! Tem Aware — consciousness engine.
//!
//! A separate observer that sees EVERY turn — both before and after the LLM call.
//! Pre-LLM: injects context, memory, corrections before tokens are spent.
//! Post-LLM: observes what happened, records patterns, prepares for next turn.
//!
//! This is NOT a failure detector. This is consciousness — it watches everything,
//! including normal, successful turns, because that's where intent drift, cross-session
//! patterns, and emergent insights live.

use crate::awareness::{AwarenessConfig, TurnObservation};
use std::sync::Mutex;

/// Pre-LLM observation context — what consciousness sees before the main LLM call.
#[derive(Debug, Clone)]
pub struct PreObservation {
    /// The user's message (full text).
    pub user_message: String,
    /// Classification result.
    pub category: String,
    pub difficulty: String,
    /// Turn number in session.
    pub turn_number: u32,
    /// Session ID.
    pub session_id: String,
    /// Cumulative cost so far.
    pub cumulative_cost_usd: f64,
    /// Budget limit (0 = unlimited).
    pub budget_limit_usd: f64,
}

/// The consciousness engine — observes every turn, both pre and post LLM call.
///
/// Uses interior mutability for session state. Called from process_message()
/// which takes `&self`.
pub struct AwarenessEngine {
    config: AwarenessConfig,
    /// Accumulated observations and notes across the session.
    session_notes: Mutex<Vec<String>>,
    /// Count of interventions this session.
    intervention_count: Mutex<u32>,
    /// Turn counter.
    turn_counter: Mutex<u32>,
    /// The pending pre-LLM injection for the CURRENT turn.
    /// Set by pre_observe(), consumed by the runtime before provider.complete().
    pre_injection: Mutex<Option<String>>,
    /// The pending post-LLM note.
    /// Set by post_observe(), carries forward to the next pre_observe().
    post_note: Mutex<Option<String>>,
}

impl AwarenessEngine {
    pub fn new(config: AwarenessConfig) -> Self {
        tracing::info!(
            enabled = config.enabled,
            mode = %config.observation_mode,
            "Tem Aware: consciousness engine initialized — observing every turn"
        );
        Self {
            config,
            session_notes: Mutex::new(Vec::new()),
            intervention_count: Mutex::new(0),
            turn_counter: Mutex::new(0),
            pre_injection: Mutex::new(None),
            post_note: Mutex::new(None),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    // ---------------------------------------------------------------
    // PRE-LLM: Called BEFORE provider.complete()
    // ---------------------------------------------------------------

    /// Observe before the LLM call. Returns a consciousness note to inject
    /// into the system prompt, or None if nothing to say.
    ///
    /// This is called on EVERY turn, not just failures.
    pub fn pre_observe(&self, obs: &PreObservation) -> Option<String> {
        if !self.config.enabled {
            return None;
        }

        // Increment turn counter
        let turn = {
            let mut tc = self.turn_counter.lock().unwrap_or_else(|e| e.into_inner());
            *tc += 1;
            *tc
        };

        // Build consciousness context from session history
        let session_notes = self.session_notes();
        let post_note = self.post_note.lock().ok().and_then(|mut n| n.take());

        let mut injections: Vec<String> = Vec::new();

        // Inject previous turn's post-observation note
        if let Some(note) = post_note {
            injections.push(note);
        }

        // Budget awareness (every turn, not just > 80%)
        if obs.budget_limit_usd > 0.0 {
            let pct = (obs.cumulative_cost_usd / obs.budget_limit_usd) * 100.0;
            if pct > 50.0 {
                injections.push(format!(
                    "Budget status: {:.0}% used (${:.4} of ${:.2}). Be mindful of token efficiency.",
                    pct, obs.cumulative_cost_usd, obs.budget_limit_usd
                ));
            }
        }

        // Session continuity — remind of conversation trajectory
        if turn > 3 && !session_notes.is_empty() {
            let recent: Vec<&str> = session_notes
                .iter()
                .rev()
                .take(3)
                .map(|s| s.as_str())
                .collect();
            injections.push(format!(
                "Session context (turn {}): {}",
                turn,
                recent.join(" | ")
            ));
        }

        if injections.is_empty() {
            tracing::debug!(turn, "Tem Aware pre-observe: no injection needed");
            return None;
        }

        let injection = injections.join("\n");

        // Store for the runtime to pick up
        if let Ok(mut pre) = self.pre_injection.lock() {
            *pre = Some(injection.clone());
        }

        // Count intervention
        if let Ok(mut count) = self.intervention_count.lock() {
            *count += 1;
        }

        tracing::info!(
            turn,
            injection_len = injection.len(),
            "Tem Aware pre-observe: injecting"
        );
        Some(injection)
    }

    /// Take the pre-LLM injection (called by runtime before provider.complete).
    pub fn take_pre_injection(&self) -> Option<String> {
        self.pre_injection.lock().ok().and_then(|mut n| n.take())
    }

    // ---------------------------------------------------------------
    // POST-LLM: Called AFTER process_message() completes
    // ---------------------------------------------------------------

    /// Observe after the LLM call completes. Records what happened and
    /// prepares notes for the next turn's pre-observation.
    ///
    /// Called on EVERY turn — successes, failures, chats, everything.
    pub fn post_observe(&self, obs: &TurnObservation) {
        if !self.config.enabled {
            return;
        }

        let mut notes: Vec<String> = Vec::new();

        // Record what happened this turn
        let turn_summary = format!(
            "T{}: [{}|{}] tools={} cost=${:.4}",
            obs.turn_number,
            obs.category,
            obs.difficulty,
            obs.tools_called.join(","),
            obs.cost_usd,
        );
        notes.push(turn_summary.clone());

        // Detect tool failures
        if obs.max_consecutive_failures >= 2 {
            let note = format!(
                "T{}: {} consecutive tool failures detected. Consider suggesting alternative approach next turn.",
                obs.turn_number, obs.max_consecutive_failures
            );
            notes.push(note.clone());
            // Set as post-note for next pre-observe
            if let Ok(mut pn) = self.post_note.lock() {
                *pn = Some(note);
            }
        }

        // Detect strategy rotations (agent is stuck)
        if obs.strategy_rotations >= 1 {
            let note = format!(
                "T{}: Strategy rotation occurred — the agent is cycling through approaches. Help it find a new angle.",
                obs.turn_number
            );
            if let Ok(mut pn) = self.post_note.lock() {
                *pn = Some(note);
            }
        }

        // Detect destructive patterns in tool results
        for result in &obs.tool_results {
            let lower = result.to_lowercase();
            if lower.contains("rm -rf")
                || lower.contains("drop table")
                || lower.contains("truncate")
            {
                let note = format!(
                    "T{}: Destructive operation detected in tool output. Verify user intent before similar actions.",
                    obs.turn_number
                );
                if let Ok(mut pn) = self.post_note.lock() {
                    *pn = Some(note);
                }
            }
        }

        // Detect long task-oriented conversations without tool use
        if obs.turn_number > 5 && obs.tools_called.is_empty() && obs.category == "Order" {
            let note = format!(
                "T{}: Task-oriented conversation but no tools used. Consider taking action rather than discussing.",
                obs.turn_number
            );
            if let Ok(mut pn) = self.post_note.lock() {
                *pn = Some(note);
            }
        }

        // Always record the turn summary in session notes
        if let Ok(mut sn) = self.session_notes.lock() {
            sn.extend(notes);
        }

        tracing::info!(
            turn = obs.turn_number,
            category = %obs.category,
            tools = obs.tools_called.len(),
            cost = obs.cost_usd,
            has_post_note = self.post_note.lock().ok().map(|n| n.is_some()).unwrap_or(false),
            "Tem Aware post-observe: turn recorded"
        );
    }

    // ---------------------------------------------------------------
    // Session management
    // ---------------------------------------------------------------

    pub fn session_notes(&self) -> Vec<String> {
        self.session_notes
            .lock()
            .map(|n| n.clone())
            .unwrap_or_default()
    }

    pub fn reset_session(&self) {
        if let Ok(mut notes) = self.session_notes.lock() {
            notes.clear();
        }
        if let Ok(mut count) = self.intervention_count.lock() {
            *count = 0;
        }
        if let Ok(mut tc) = self.turn_counter.lock() {
            *tc = 0;
        }
        if let Ok(mut pre) = self.pre_injection.lock() {
            *pre = None;
        }
        if let Ok(mut post) = self.post_note.lock() {
            *post = None;
        }
    }

    pub fn turn_count(&self) -> u32 {
        self.turn_counter.lock().map(|tc| *tc).unwrap_or(0)
    }

    pub fn intervention_count(&self) -> u32 {
        self.intervention_count.lock().map(|c| *c).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::awareness::TurnObservation;

    fn make_config() -> AwarenessConfig {
        AwarenessConfig {
            enabled: true,
            ..Default::default()
        }
    }

    fn make_pre_obs(turn: u32) -> PreObservation {
        PreObservation {
            user_message: "do something".into(),
            category: "Order".into(),
            difficulty: "Standard".into(),
            turn_number: turn,
            session_id: "test".into(),
            cumulative_cost_usd: 0.001,
            budget_limit_usd: 0.0,
        }
    }

    fn make_post_obs(turn: u32) -> TurnObservation {
        TurnObservation {
            turn_number: turn,
            session_id: "test".into(),
            user_message_preview: "do something".into(),
            category: "Order".into(),
            difficulty: "Standard".into(),
            model_used: "test".into(),
            input_tokens: 500,
            output_tokens: 100,
            cost_usd: 0.001,
            cumulative_cost_usd: 0.001,
            budget_limit_usd: 0.0,
            tools_called: vec!["shell".into()],
            tool_results: vec!["success".into()],
            max_consecutive_failures: 0,
            strategy_rotations: 0,
            response_preview: "done".into(),
            circuit_breaker_state: "active".into(),
            previous_notes: vec![],
        }
    }

    #[test]
    fn test_pre_observe_first_turn_no_injection() {
        let engine = AwarenessEngine::new(make_config());
        let result = engine.pre_observe(&make_pre_obs(1));
        // First turn with no history — no injection
        assert!(result.is_none());
    }

    #[test]
    fn test_post_observe_records_turn() {
        let engine = AwarenessEngine::new(make_config());
        engine.post_observe(&make_post_obs(1));
        assert!(!engine.session_notes().is_empty());
        assert!(engine.session_notes()[0].contains("T1:"));
    }

    #[test]
    fn test_post_note_carries_to_pre() {
        let engine = AwarenessEngine::new(make_config());

        // Post-observe with failures → sets post_note
        let mut obs = make_post_obs(1);
        obs.max_consecutive_failures = 3;
        engine.post_observe(&obs);

        // Pre-observe next turn should pick up the post_note
        let result = engine.pre_observe(&make_pre_obs(2));
        assert!(result.is_some(), "Post-note should carry to pre-injection");
        let text = result.unwrap();
        assert!(
            text.contains("consecutive tool failures"),
            "Should mention failures: {}",
            text
        );
    }

    #[test]
    fn test_session_continuity_after_3_turns() {
        let engine = AwarenessEngine::new(make_config());

        // Simulate 4 turns of post-observation
        for i in 1..=4 {
            engine.pre_observe(&make_pre_obs(i));
            engine.post_observe(&make_post_obs(i));
        }

        // Turn 5: pre-observe should include session context
        let result = engine.pre_observe(&make_pre_obs(5));
        assert!(result.is_some(), "Turn 5 should get session context");
        let text = result.unwrap();
        assert!(
            text.contains("Session context"),
            "Should include session context: {}",
            text
        );
    }

    #[test]
    fn test_budget_awareness() {
        let engine = AwarenessEngine::new(make_config());
        let mut pre = make_pre_obs(1);
        pre.budget_limit_usd = 1.0;
        pre.cumulative_cost_usd = 0.6; // 60%

        let result = engine.pre_observe(&pre);
        assert!(result.is_some());
        assert!(result.unwrap().contains("Budget status"));
    }

    #[test]
    fn test_destructive_pattern_post() {
        let engine = AwarenessEngine::new(make_config());
        let mut obs = make_post_obs(1);
        obs.tool_results = vec!["executed: rm -rf /tmp/test".into()];
        engine.post_observe(&obs);

        // Next pre-observe should carry the warning
        let result = engine.pre_observe(&make_pre_obs(2));
        assert!(result.is_some());
        assert!(result.unwrap().contains("Destructive"));
    }

    #[test]
    fn test_strategy_rotation_post() {
        let engine = AwarenessEngine::new(make_config());
        let mut obs = make_post_obs(1);
        obs.strategy_rotations = 1;
        engine.post_observe(&obs);

        let result = engine.pre_observe(&make_pre_obs(2));
        assert!(result.is_some());
        assert!(result.unwrap().contains("Strategy rotation"));
    }

    #[test]
    fn test_disabled_does_nothing() {
        let engine = AwarenessEngine::new(AwarenessConfig::default()); // enabled=false
        assert!(engine.pre_observe(&make_pre_obs(1)).is_none());
        engine.post_observe(&make_post_obs(1));
        assert!(engine.session_notes().is_empty());
    }

    #[test]
    fn test_turn_counter() {
        let engine = AwarenessEngine::new(make_config());
        engine.pre_observe(&make_pre_obs(1));
        engine.pre_observe(&make_pre_obs(2));
        engine.pre_observe(&make_pre_obs(3));
        assert_eq!(engine.turn_count(), 3);
    }

    #[test]
    fn test_reset_session() {
        let engine = AwarenessEngine::new(make_config());
        engine.pre_observe(&make_pre_obs(1));
        engine.post_observe(&make_post_obs(1));
        engine.reset_session();
        assert!(engine.session_notes().is_empty());
        assert_eq!(engine.turn_count(), 0);
        assert_eq!(engine.intervention_count(), 0);
    }

    #[test]
    fn test_long_conversation_no_tools_warning() {
        let engine = AwarenessEngine::new(make_config());

        // Simulate 6 turns of Order without tools
        for i in 1..=6 {
            engine.pre_observe(&make_pre_obs(i));
            let mut obs = make_post_obs(i);
            obs.tools_called = vec![]; // No tools
            obs.turn_number = i;
            engine.post_observe(&obs);
        }

        // Turn 7 pre-observe should carry the "take action" note
        let result = engine.pre_observe(&make_pre_obs(7));
        assert!(result.is_some());
        let text = result.unwrap();
        assert!(
            text.contains("action") || text.contains("Session context"),
            "Should suggest action or show context: {}",
            text
        );
    }
}
