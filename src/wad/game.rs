//! Game detection from a lump directory: which game an archive belongs to.
//!
//! [`detect_game`] answers one question from lump names alone: which
//! commercial game does this directory look like? The rules are the
//! signature lumps `ZDoom` documents in its `iwadinfo.txt`, applied in the
//! same order so the first matching rule wins. The pass is pure: it reads
//! no files and never second-guesses a name into a guess about content.

use serde::Serialize;

use crate::wad::name::LumpName;

/// The game a lump directory's signatures point to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Game {
    /// `DOOM` or Ultimate `DOOM`: episode maps and no `MAP01`.
    Doom,
    /// `DOOM II`, its `PWAD` replacements (`Final DOOM`, `Plutonia`, `TNT`)
    /// and any `MAP`-format archive: has `MAP01`.
    Doom2,
    /// Heretic: episode maps with its own `TITLE` and `MUS_E1M1` signature.
    Heretic,
    /// Hexen: its `TITLE` plus a `WINNOWR` or `CLUS1MSG` lump.
    Hexen,
    /// Strife: carries the `ENDSTRF` end screen.
    Strife,
    /// No rule matched.
    Unknown,
}

/// Whether the directory carries a lump named exactly `wanted`
/// (case-insensitively, as [`LumpName`] compares).
fn has(names: &[LumpName], wanted: &[u8]) -> bool {
    names.iter().any(|name| name.as_bytes() == wanted)
}

/// Whether the directory carries lumps named by every entry of `wanted`.
fn has_all(names: &[LumpName], wanted: [&[u8]; 3]) -> bool {
    wanted.iter().all(|byte| has(names, byte))
}

/// The game these lump names identify, first matching rule wins:
///
/// 1. `Hexen`: `TITLE` with `WINNOWR` or `CLUS1MSG`;
/// 2. Strife: `ENDSTRF`;
/// 3. Heretic: `E1M1`, `TITLE` and `MUS_E1M1`;
/// 4. `DOOM II`: `MAP01`;
/// 5. `DOOM`: `E1M1`;
/// 6. otherwise [`Game::Unknown`].
#[must_use]
#[inline]
pub fn detect_game(names: &[LumpName]) -> Game {
    let hexen = has(names, b"TITLE") && (has(names, b"WINNOWR") || has(names, b"CLUS1MSG"));
    if hexen {
        return Game::Hexen;
    }
    if has(names, b"ENDSTRF") {
        return Game::Strife;
    }
    if has_all(names, [b"E1M1", b"TITLE", b"MUS_E1M1"]) {
        return Game::Heretic;
    }
    if has(names, b"MAP01") {
        return Game::Doom2;
    }
    if has(names, b"E1M1") {
        return Game::Doom;
    }
    Game::Unknown
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Game, detect_game};
    use crate::wad::name::LumpName;

    /// Parses fixture names; they are all valid.
    fn names(texts: &[&str]) -> Vec<LumpName> {
        texts
            .iter()
            .map(|text| LumpName::parse(text).expect("test names are valid"))
            .collect()
    }

    #[test]
    fn a_hexen_iwad_is_hexen_even_with_map01_and_e1m1() {
        let hexen = detect_game(&names(&["TITLE", "WINNOWR", "MAP01", "E1M1"]));
        assert_eq!(hexen, Game::Hexen, "TITLE plus WINNOWR wins first");
        let clus1 = detect_game(&names(&["TITLE", "CLUS1MSG", "MAP01"]));
        assert_eq!(clus1, Game::Hexen, "TITLE plus CLUS1MSG also matches");
    }

    #[test]
    fn title_alone_is_not_hexen() {
        let only = detect_game(&names(&["TITLE", "MAP01"]));
        assert_eq!(only, Game::Doom2, "WINNOWR or CLUS1MSG is required");
    }

    #[test]
    fn strife_is_detected_by_its_end_screen() {
        let strife = detect_game(&names(&["ENDSTRF", "TITLE", "MAP01"]));
        assert_eq!(strife, Game::Strife, "ENDSTRF outranks the DOOM II rule");
    }

    #[test]
    fn heretic_needs_all_three_signatures() {
        let heretic = detect_game(&names(&["E1M1", "TITLE", "MUS_E1M1"]));
        assert_eq!(heretic, Game::Heretic, "the full signature set");
        let partial = detect_game(&names(&["E1M1", "TITLE", "MAP01"]));
        assert_eq!(partial, Game::Doom2, "without MUS_E1M1 the MAP01 rule hits");
    }

    #[test]
    fn heretic_outranks_the_map01_rule() {
        let both = detect_game(&names(&["E1M1", "TITLE", "MUS_E1M1", "MAP01"]));
        assert_eq!(both, Game::Heretic, "rule 3 comes before rule 4");
    }

    #[test]
    fn map01_means_doom_2_and_e1m1_alone_means_doom() {
        assert_eq!(
            detect_game(&names(&["MAP01", "E1M1"])),
            Game::Doom2,
            "MAP01 wins"
        );
        assert_eq!(
            detect_game(&names(&["E1M1", "E2M7"])),
            Game::Doom,
            "episodes only"
        );
    }

    #[test]
    fn nothing_matching_is_unknown() {
        assert_eq!(detect_game(&[]), Game::Unknown, "an empty directory");
        assert_eq!(
            detect_game(&names(&["PLAYPAL", "FLOOR4_1"])),
            Game::Unknown,
            "a PWAD of flats only"
        );
    }

    #[test]
    fn game_serializes_lowercase() {
        for (game, text) in [
            (Game::Doom, "doom"),
            (Game::Doom2, "doom2"),
            (Game::Heretic, "heretic"),
            (Game::Hexen, "hexen"),
            (Game::Strife, "strife"),
            (Game::Unknown, "unknown"),
        ] {
            assert_eq!(
                serde_json::to_value(game).expect("serializes"),
                json!(text),
                "wire name of {game:?}"
            );
        }
    }
}
