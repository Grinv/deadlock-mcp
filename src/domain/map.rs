//! Places on the map: the objectives of the map asset, named by team and lane, so that a world
//! position (a death in `match_player`, a hotspot of `player_deaths_map`) reads as "The
//! Archmother's York Guardian".
//!
//! The map asset places objectives on the minimap, as fractions of its size; match data is in
//! world units. The asset's entities and neutral camps carry both, and all of them fit
//! `left = (x + r) / 2r`, `top = (r - y) / 2r` (`r` = `radius`) to 0.0001. Checked against matches
//! 143768397 and 143785700 (build 6868, 2026-11-26), with the heroes' paths: every hero spawns
//! within 510 units of its own lobby team's `team<N>_core` (team 0 at y = -10100, team 1 at
//! +10300), 20000 from the other; in 19 of 24 falls of a Guardian or Walker the nearest attacking
//! hero stood within 700 units of it; a killer's position in the fixture is (-7040, 1984), the
//! computed Archmother York Guardian; and 28 of the 29 deaths before 10:00 lie nearest the zip line
//! of the dead hero's own lane.

use crate::contracts::api;
use crate::contracts::valve::ECitadelLobbyTeam;
use crate::domain::assets::{Lane, hex};
use crate::domain::terms::team_name;

/// About 50 m (39.37 units per meter): Guardians of one lane stand about 3800 units apart.
const NEAR: f64 = 2000.0;

pub struct Landmark {
    name: String,
    at: [f64; 2],
}

fn xy(v: &[f64]) -> Option<[f64; 2]> {
    Some([*v.first()?, *v.get(1)?])
}

fn distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// The lane whose zip line passes closest, matched to the game's lanes by color.
fn lane_at<'a>(map: &api::MapData, lanes: &'a [(u32, Lane)], at: [f64; 2]) -> Option<&'a Lane> {
    let zipline = map
        .zipline_paths
        .iter()
        .filter_map(|z| {
            let origin = xy(&z.origin)?;
            // The spline's segment starts lie on it, relative to its origin.
            let d = z
                .p0_points
                .iter()
                .filter_map(|p| xy(p))
                .map(|p| distance([origin[0] + p[0], origin[1] + p[1]], at))
                .min_by(f64::total_cmp)?;
            Some((d, z))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))?
        .1;
    let color = hex(&zipline.color_parsed);
    lanes
        .iter()
        .map(|(_, lane)| lane)
        .find(|lane| lane.color.as_deref() == Some(color.as_str()))
}

/// The objectives with names, at their world positions. Keys are `team<0|1>_<what>`, `<what>`
/// being `core`, `titan` or `tier<1|2>_<lane>`; others are left out.
pub fn landmarks(map: &api::MapData, lanes: &[(u32, Lane)]) -> Vec<Landmark> {
    let Ok(r) = u32::try_from(map.radius).map(f64::from) else {
        return Vec::new();
    };
    map.objective_positions
        .iter()
        .filter_map(|(key, pos)| {
            let (team, what) = key.split_once('_')?;
            let team = match team {
                "team0" => ECitadelLobbyTeam::KECitadelLobbyTeamTeam0,
                "team1" => ECitadelLobbyTeam::KECitadelLobbyTeamTeam1,
                _ => return None,
            };
            // The position is the marker's top-left corner; a core's marker is 30% x 8% of the
            // minimap, the others 10% x 10% (the API's `ObjectivePosition`).
            let (w, h) = if what == "core" {
                (0.30, 0.08)
            } else {
                (0.10, 0.10)
            };
            let at = [
                (pos.left_relative + w / 2.0) * 2.0 * r - r,
                r - (pos.top_relative + h / 2.0) * 2.0 * r,
            ];
            // The game's names for them; the core marker sits on the team's base.
            let kind = match what.split_once('_').map_or(what, |(tier, _)| tier) {
                "core" => "base",
                "titan" => "Patron",
                "tier1" => "Guardian",
                "tier2" => "Walker",
                _ => return None,
            };
            let team = team_name(team);
            let name = match what
                .contains('_')
                .then(|| lane_at(map, lanes, at))
                .flatten()
            {
                Some(lane) => format!("{team}'s {} {kind}", lane.name),
                None => format!("{team}'s {kind}"),
            };
            Some(Landmark { name, at })
        })
        .collect()
}

/// The landmark nearest to a world position, if one is within about 50 m.
pub fn near(landmarks: &[Landmark], x: f64, y: f64) -> Option<String> {
    landmarks
        .iter()
        .map(|l| (distance(l.at, [x, y]), l))
        .filter(|(d, _)| *d <= NEAR)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, l)| l.name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_places_of_match_positions() {
        let map: api::MapData =
            serde_json::from_str(include_str!("../../contracts/fixtures/map.json")).unwrap();
        let lane = |name: &str, color: &str| Lane {
            name: name.to_owned(),
            color: Some(color.to_owned()),
        };
        let lanes = [
            (1, lane("York", "#F1CC30")),
            (4, lane("Broadway", "#29B1CC")),
            (6, lane("Greenwich", "#59B247")),
        ];
        let marks = landmarks(&map, &lanes);
        assert_eq!(marks.len(), 16);
        let at = |x, y| near(&marks, x, y);
        // From contracts/fixtures/match_metadata.json: a death in the York lane at 5:10 and its
        // killer's position, and a spawn point of lobby team 0 (match_paths, not in the fixture).
        let york = Some("The Archmother's York Guardian".to_owned());
        assert_eq!(at(-6346.5, 2160.1), york);
        assert_eq!(at(-7040.0, 1984.0), york);
        assert_eq!(
            at(-1544.0, -10154.0).as_deref(),
            Some("The Hidden King's base")
        );
        assert_eq!(
            at(6988.0, -2007.0).as_deref(),
            Some("The Hidden King's Greenwich Guardian")
        );
        assert_eq!(
            at(0.0, 1800.0).as_deref(),
            Some("The Archmother's Broadway Guardian")
        );
        assert_eq!(
            at(1279.0, 8049.0).as_deref(),
            Some("The Archmother's Patron")
        );
        // Between the lanes, far from any objective.
        assert_eq!(at(-4071.0, 2172.0), None);
    }
}
