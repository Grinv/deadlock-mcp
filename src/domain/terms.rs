//! Names for Valve's match enums. The matches are exhaustive, so a regenerated contract that adds
//! a variant fails to compile here instead of showing up as "other".

use crate::contracts::valve::c_msg_match_meta_data_contents::EGoldSource;
use crate::contracts::valve::{ECitadelGameMode, ECitadelLobbyTeam, ECitadelMatchMode};

/// The game's localization: `Citadel_Team1_Name` and `Citadel_Team2_Name`.
pub const fn team_name(team: ECitadelLobbyTeam) -> &'static str {
    match team {
        ECitadelLobbyTeam::KECitadelLobbyTeamTeam0 => "The Hidden King",
        ECitadelLobbyTeam::KECitadelLobbyTeamTeam1 => "The Archmother",
        ECitadelLobbyTeam::KECitadelLobbyTeamSpectator => "Spectators",
    }
}

pub const fn match_mode_name(mode: ECitadelMatchMode) -> &'static str {
    match mode {
        ECitadelMatchMode::KECitadelMatchModeInvalid => "Unknown",
        ECitadelMatchMode::KECitadelMatchModeUnranked => "Unranked",
        ECitadelMatchMode::KECitadelMatchModePrivateLobby => "Private lobby",
        ECitadelMatchMode::KECitadelMatchModeCoopBot => "Bot match",
        ECitadelMatchMode::KECitadelMatchModeRanked => "Ranked",
        ECitadelMatchMode::KECitadelMatchModeServerTest => "Server test",
        ECitadelMatchMode::KECitadelMatchModeTutorial => "Tutorial",
        ECitadelMatchMode::KECitadelMatchModeHeroLabs => "Hero Labs",
        ECitadelMatchMode::KECitadelMatchModeNewPlayerPlacement => "New player placement",
    }
}

pub const fn game_mode_name(mode: ECitadelGameMode) -> &'static str {
    match mode {
        ECitadelGameMode::KECitadelGameModeInvalid => "Unknown",
        ECitadelGameMode::KECitadelGameModeNormal => "Normal",
        ECitadelGameMode::KECitadelGameMode1v1Test => "1v1 test",
        ECitadelGameMode::KECitadelGameModeSandbox => "Sandbox",
        ECitadelGameMode::KECitadelGameModeStreetBrawl => "Street Brawl",
        ECitadelGameMode::KECitadelGameModeExploreNyc => "Explore NYC",
        ECitadelGameMode::KECitadelGameModeInternal
        | ECitadelGameMode::KECitadelGameModeInternal02 => "Internal",
    }
}

/// What each `EGoldSource` covers, checked against match demos (deadlock-api/deadlock-api#733).
pub const fn gold_source_name(source: EGoldSource) -> &'static str {
    match source {
        EGoldSource::KEPlayers => "hero kills",
        EGoldSource::KELaneCreeps => "lane troopers",
        EGoldSource::KENeutrals => "jungle (neutrals, incl. Sinner's Sacrifice)",
        EGoldSource::KEBosses => "objectives and bosses",
        EGoldSource::KETreasure => "urn and rift",
        EGoldSource::KEAssists => "assists",
        EGoldSource::KEDenies => "denies",
        EGoldSource::KETeamBonus => "team bonus",
        EGoldSource::KEAbilityAssassinate => "assassinate ability",
        EGoldSource::KEItemTrophyCollector => "Trophy Collector",
        EGoldSource::KEItemCultistSacrifice => "Cultist Sacrifice",
        EGoldSource::KEBreakable => "breakables (crates)",
        EGoldSource::KEItemGooseEgg => "Golden Goose Egg",
    }
}
