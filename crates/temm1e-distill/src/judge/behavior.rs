//! Behavior Judge — user behavior signals for SPRT/CUSUM.
//! Default shadow/monitor method. Zero LLM cost.

use crate::collector::{is_likely_retry, is_rejection};
use crate::types::QualitySignal;

/// Determine the SPRT observation from user behavior.
/// Returns (observation, signal_type) where observation is 1 (agree) or 0 (disagree).
pub fn behavior_observation(
    current_message: &str,
    previous_message: Option<&str>,
    elapsed_secs: u64,
    tool_failed: bool,
) -> (bool, &'static str) {
    // Priority 1: Tool failure
    if tool_failed {
        return (false, "tool_failure");
    }

    // Priority 2: Explicit rejection
    if is_rejection(current_message) {
        return (false, "explicit_rejection");
    }

    // Priority 3: Retry/rephrase
    if let Some(prev) = previous_message {
        if is_likely_retry(current_message, prev, elapsed_secs) {
            return (false, "retry_rephrase");
        }
    }

    // Default: user continued normally (implicit agreement)
    (true, "continued_normally")
}

/// Map QualitySignal to SPRT observation.
pub fn signal_to_observation(signal: QualitySignal) -> bool {
    signal.is_positive()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_failure_disagrees() {
        let (agree, signal) = behavior_observation("ok", None, 0, true);
        assert!(!agree);
        assert_eq!(signal, "tool_failure");
    }

    #[test]
    fn test_rejection_disagrees() {
        let (agree, signal) = behavior_observation("That's wrong", None, 0, false);
        assert!(!agree);
        assert_eq!(signal, "explicit_rejection");
    }

    #[test]
    fn test_retry_disagrees() {
        let (agree, signal) = behavior_observation(
            "What is the weather today",
            Some("What is the weather"),
            30,
            false,
        );
        assert!(!agree);
        assert_eq!(signal, "retry_rephrase");
    }

    #[test]
    fn test_normal_continuation_agrees() {
        let (agree, signal) = behavior_observation(
            "Thanks, now tell me about Rust",
            Some("What is Python?"),
            45,
            false,
        );
        assert!(agree);
        assert_eq!(signal, "continued_normally");
    }

    #[test]
    fn test_signal_to_observation_positive() {
        assert!(signal_to_observation(QualitySignal::UserContinued));
        assert!(signal_to_observation(QualitySignal::ToolCallSucceeded));
    }

    #[test]
    fn test_signal_to_observation_negative() {
        assert!(!signal_to_observation(QualitySignal::UserRetried));
        assert!(!signal_to_observation(QualitySignal::ResponseError));
    }
}
