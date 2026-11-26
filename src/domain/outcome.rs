//! A player's own result in a match. The team result (`match_result == player_team`) is not it:
//! the game voids a match for the team of a leaver, and a leaver loses rank even when their team
//! wins (deadlock-api/deadlock-api#729).

use schemars::JsonSchema;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlayerResult {
    Win,
    Loss,
    /// The game didn't count the match for this player (usually a teammate left).
    NotScored,
}

/// `player_match_outcome`: 1 win, 2 loss, 3 penalized (left), 4 penalized with the party,
/// 5 not scored. 0 (matches before late July 2026) falls back to the team result.
pub fn player_result(outcome: u64, team: u64, winning_team: u64) -> PlayerResult {
    match outcome {
        1 => PlayerResult::Win,
        2..=4 => PlayerResult::Loss,
        5 => PlayerResult::NotScored,
        _ if team == winning_team => PlayerResult::Win,
        _ => PlayerResult::Loss,
    }
}

/// Why the result differs from a plain win or loss, if it does.
pub fn note(outcome: u64) -> Option<&'static str> {
    match outcome {
        3 => Some("penalized for leaving: counted as a loss"),
        4 => Some("penalized with a party member who left: counted as a loss"),
        5 => Some("not scored: the game voided the match for this player"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_wins_over_team_result() {
        assert_eq!(player_result(5, 1, 0), PlayerResult::NotScored);
        assert_eq!(player_result(3, 0, 0), PlayerResult::Loss);
        assert_eq!(player_result(0, 0, 0), PlayerResult::Win);
        assert_eq!(player_result(0, 1, 0), PlayerResult::Loss);
    }
}
