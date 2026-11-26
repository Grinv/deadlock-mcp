//! A player's own result in a match. The team result (`match_result == player_team`) is not it:
//! the game voids a match for the team of a leaver, and a leaver loses rank even when their team
//! wins (deadlock-api/deadlock-api#729).

use schemars::JsonSchema;
use serde::Serialize;

use crate::contracts::valve::EPlayerMatchOutcome;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlayerResult {
    Win,
    Loss,
    /// The game didn't count the match for this player (usually a teammate left).
    NotScored,
}

/// `Invalid` (matches before late July 2026 carry no outcome) and unknown values fall back to
/// the team result.
pub fn player_result(outcome: i32, won_as_team: bool) -> PlayerResult {
    use EPlayerMatchOutcome as O;
    match O::try_from(outcome).unwrap_or(O::KEPlayerMatchOutcomeInvalid) {
        O::KEPlayerMatchOutcomeWin => PlayerResult::Win,
        O::KEPlayerMatchOutcomeNotScored => PlayerResult::NotScored,
        O::KEPlayerMatchOutcomeInvalid if won_as_team => PlayerResult::Win,
        O::KEPlayerMatchOutcomeLoss
        | O::KEPlayerMatchOutcomePenalized
        | O::KEPlayerMatchOutcomePenalizedParty
        | O::KEPlayerMatchOutcomeInvalid => PlayerResult::Loss,
    }
}

/// Why the result differs from a plain win or loss, if it does.
pub fn note(outcome: i32) -> Option<&'static str> {
    use EPlayerMatchOutcome as O;
    match O::try_from(outcome).ok()? {
        O::KEPlayerMatchOutcomePenalized => Some("penalized for leaving: counted as a loss"),
        O::KEPlayerMatchOutcomePenalizedParty => {
            Some("penalized with a party member who left: counted as a loss")
        }
        O::KEPlayerMatchOutcomeNotScored => {
            Some("not scored: the game voided the match for this player")
        }
        O::KEPlayerMatchOutcomeInvalid
        | O::KEPlayerMatchOutcomeWin
        | O::KEPlayerMatchOutcomeLoss => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_wins_over_team_result() {
        assert_eq!(player_result(5, true), PlayerResult::NotScored);
        assert_eq!(player_result(3, true), PlayerResult::Loss);
        assert_eq!(player_result(0, true), PlayerResult::Win);
        assert_eq!(player_result(0, false), PlayerResult::Loss);
        assert_eq!(player_result(99, true), PlayerResult::Win);
    }
}
