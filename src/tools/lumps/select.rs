//! The shared lump selector behind every tool that reads one lump.
//!
//! [`LumpSelector`] is the parameter half: a name plus the optional `scope`,
//! `namespace` and `occurrence` refinements, flattened into each tool's
//! arguments. [`resolve`] is the read half: it parses the selector, lists
//! the scope's [`LumpRow`]s through [`build_rows`], keeps the same-name
//! matches (inside `namespace` when given) and picks one. The default
//! `occurrence` is the **last** match in directory order, the copy an
//! engine's lookup of that name reaches for. [`nth_match`] is that pick on
//! its own, shared with the working-archive tools in
//! [`crate::tools::lumps::edit`] so one refusal text names the count
//! everywhere.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::error::CyberError;
use crate::session::{Scope, Session};
use crate::tools::lumps::rows::{LumpRow, build_rows};
use crate::wad::name::LumpName;

/// The fields that pick one lump, flattened into each tool's arguments.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub(in crate::tools) struct LumpSelector {
    /// Lump name to read: 1 to 8 printable `ASCII` characters, matched
    /// case-insensitively (it is uppercased), e.g. `"PLAYPAL"`.
    pub(in crate::tools) name: String,
    /// Which archive to read from: `"merged"` (the default: what an engine
    /// sees), `"working"` or `"layer:N"` (0 is the `IWAD`).
    #[serde(default)]
    pub(in crate::tools) scope: Option<String>,
    /// Keep only matches inside this namespace region (`sprites`, `flats`,
    /// `patches`, `colormaps`, `acs`, `voices`, `hires`, `textures`);
    /// without it any region and the loose lumps match.
    #[serde(default)]
    pub(in crate::tools) namespace: Option<String>,
    /// 0-based index among the matching lumps, in the scope's directory
    /// order; the default is the last one, i.e. the copy that wins.
    #[serde(default)]
    pub(in crate::tools) occurrence: Option<usize>,
}

/// The match `occurrence` numbers out of the directory-ordered `matches`,
/// the last when it is `None`. `matches` must hold at least one row:
/// every caller refuses an empty set in its own words, since which
/// message names what is missing differs per tool.
///
/// # Errors
///
/// [`CyberError::NotFound`] naming how many matched and the valid range
/// when `occurrence` reaches past them; `place` says where the refusal
/// found them.
pub(in crate::tools::lumps) fn nth_match<T>(
    matches: Vec<T>,
    occurrence: Option<usize>,
    name: LumpName,
    place: &str,
) -> Result<T, CyberError> {
    let count = matches.len();
    let last = count.saturating_sub(1);
    let wanted = occurrence.unwrap_or(last);
    matches.into_iter().nth(wanted).ok_or_else(|| {
        CyberError::NotFound(format!(
            "only {count} lumps named {name} in {place}: occurrence {wanted} is out of range \
             (0 to {last})"
        ))
    })
}

/// The [`LumpRow`] the selector picks.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a name, `scope` or `namespace` that
/// does not parse; [`CyberError::NotFound`] when no lump in the scope
/// matches, or when `occurrence` reaches past the matches (the message
/// names how many exist); [`build_rows`]' errors for the scope itself.
pub(in crate::tools) fn resolve(
    session: &Session,
    selector: &LumpSelector,
) -> Result<LumpRow, CyberError> {
    let name = LumpName::parse(&selector.name)?;
    let scope = Scope::parse_or_merged(selector.scope.as_deref())?;
    let namespace = super::parse_namespace(selector.namespace.as_deref())?;
    let matched: Vec<LumpRow> = build_rows(session, scope)?
        .into_iter()
        .filter(|row| {
            row.name == name && namespace.is_none_or(|wanted| row.namespace == Some(wanted))
        })
        .collect();
    if matched.is_empty() {
        return Err(
            CyberError::NotFound(format!("lump {name} not found in {scope}"))
                .with_hint("search_lumps finds lumps by pattern"),
        );
    }
    nth_match(matched, selector.occurrence, name, &scope.to_string())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{LumpSelector, nth_match, resolve};
    use crate::error::CyberError;
    use crate::session::{Layer, LayerRole, Scope, Session};
    use crate::wad::archive::{Lump, Wad, WadKind};
    use crate::wad::classify::Namespace;
    use crate::wad::name::LumpName;

    /// A valid parsed name for test data.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// A lump named `text` whose payload marks which copy it is.
    fn copy(text: &str, mark: u8) -> Lump {
        Lump::new(name(text), vec![mark])
    }

    /// An empty marker lump named `text`.
    fn marker(text: &str) -> Lump {
        Lump::marker(name(text))
    }

    /// A selector for `text` with no refinements.
    fn selector(text: &str) -> LumpSelector {
        LumpSelector {
            name: text.to_owned(),
            scope: None,
            namespace: None,
            occurrence: None,
        }
    }

    /// A selector for `text` in `scope` with an explicit occurrence.
    fn scoped(text: &str, scope: &str, occurrence: Option<usize>) -> LumpSelector {
        LumpSelector {
            name: text.to_owned(),
            scope: Some(scope.to_owned()),
            namespace: None,
            occurrence,
        }
    }

    /// A session whose base layer holds two `FLOOR1` copies (one loose, one
    /// in the flats region), a `MID`, and a `SPRITE1` in the sprites
    /// region; the working archive is empty.
    fn session() -> Session {
        let mut session = Session::default();
        session
            .set_iwad(Layer::new(
                PathBuf::from("base.wad"),
                LayerRole::Iwad,
                Wad {
                    kind: WadKind::Iwad,
                    lumps: vec![
                        copy("FLOOR1", 1),
                        copy("MID", 9),
                        marker("F_START"),
                        copy("FLOOR1", 2),
                        marker("F_END"),
                        marker("S_START"),
                        copy("SPRITE1", 3),
                        marker("S_END"),
                    ],
                },
            ))
            .expect("no batch is open");
        session
    }

    /// The error code of a [`CyberError`], for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    /// The first payload byte of a resolved row, to tell copies apart.
    fn mark(selector: &LumpSelector) -> u8 {
        let row = resolve(&session(), selector).expect("the fixture lumps exist");
        *row.lump.data.first().expect("one-byte payloads")
    }

    #[test]
    fn nth_match_numbers_the_matches_and_refuses_past_the_end() {
        assert_eq!(
            nth_match(vec![7_usize, 8], None, name("TWIN"), "merged").expect("the default"),
            8,
            "the default is the last match"
        );
        assert_eq!(
            nth_match(vec![7_usize, 8], Some(0), name("TWIN"), "merged").expect("the first"),
            7,
            "an explicit occurrence numbers from the front"
        );
        let error =
            nth_match(vec![7_usize, 8], Some(2), name("TWIN"), "merged").expect_err("past the end");
        assert_eq!(code(&error), "not_found", "past the end code");
        assert_eq!(
            error.to_string(),
            "only 2 lumps named TWIN in merged: occurrence 2 is out of range (0 to 1)",
            "the one refusal names the count, the place and the range"
        );
        let working = nth_match(vec![7_usize], Some(1), name("TWIN"), "the working archive")
            .expect_err("one copy only");
        assert_eq!(
            working.to_string(),
            "only 1 lumps named TWIN in the working archive: occurrence 1 is out of range \
             (0 to 0)",
            "the same text locates the working matches"
        );
    }

    #[test]
    fn the_default_occurrence_is_the_last_match() {
        assert_eq!(mark(&selector("FLOOR1")), 2, "the flats copy is last");
        assert_eq!(mark(&selector("MID")), 9, "a lone name resolves");
        assert_eq!(
            mark(&scoped("FLOOR1", "merged", Some(0))),
            1,
            "directory order numbers the loose copy first"
        );
    }

    #[test]
    fn an_occurrence_past_the_matches_names_how_many_exist() {
        let error = resolve(&session(), &scoped("FLOOR1", "merged", Some(2))).expect_err("only 2");
        assert_eq!(code(&error), "not_found", "past the end code");
        let message = error.to_string();
        assert!(
            message.contains("only 2 lumps named FLOOR1 in merged") && message.contains("0 to 1"),
            "the message counts the matches and the range: {message}"
        );
    }

    #[test]
    fn the_namespace_filter_restricts_the_matches() {
        let flats = LumpSelector {
            namespace: Some("flats".to_owned()),
            ..selector("FLOOR1")
        };
        let row = resolve(&session(), &flats).expect("the region copy");
        assert_eq!(
            row.namespace.map(Namespace::as_str),
            Some("flats"),
            "in flats"
        );
        assert_eq!(row.lump.data.first().copied(), Some(2), "the region copy");
        let sprites = LumpSelector {
            namespace: Some("sprites".to_owned()),
            ..selector("FLOOR1")
        };
        let error = resolve(&session(), &sprites).expect_err("no floor in sprites");
        assert_eq!(code(&error), "not_found", "namespace miss code");
        assert!(
            error.to_string().contains("in merged"),
            "the message names the scope: {error}"
        );
    }

    #[test]
    fn scopes_reach_one_archive_each() {
        let layer = resolve(&session(), &scoped("FLOOR1", "layer:0", None)).expect("layer 0");
        assert_eq!(
            layer.lump.data.first().copied(),
            Some(2),
            "the last base copy wins"
        );
        assert_eq!(layer.index, 3, "a layer index counts inside that archive");
        let error = resolve(&session(), &scoped("FLOOR1", "working", None))
            .expect_err("the working archive is empty");
        assert_eq!(code(&error), "not_found", "empty working scope code");
        assert!(
            error.to_string().contains("not found in working"),
            "the message names the scope: {error}"
        );
    }

    #[test]
    fn selector_parsing_errors_carry_their_own_codes() {
        let bad_name = resolve(&session(), &selector("TOO LONG NAME")).expect_err("invalid name");
        assert_eq!(code(&bad_name), "invalid_argument", "bad name code");
        let bad_scope = resolve(&session(), &scoped("FLOOR1", "nope", None)).expect_err("no scope");
        assert_eq!(code(&bad_scope), "invalid_argument", "bad scope code");
        let bad_ns = LumpSelector {
            namespace: Some("nope".to_owned()),
            ..selector("FLOOR1")
        };
        let error = resolve(&session(), &bad_ns).expect_err("no namespace");
        assert_eq!(code(&error), "invalid_argument", "bad namespace code");
        let missing = resolve(&session(), &selector("NOPE")).expect_err("no such lump");
        assert_eq!(code(&missing), "not_found", "missing lump code");
        assert_eq!(
            missing.to_string(),
            "lump NOPE not found in merged",
            "the documented message"
        );
    }

    #[test]
    fn scope_labels_name_every_archive() {
        assert_eq!(Scope::Merged.to_string(), "merged", "the merged view");
        assert_eq!(
            Scope::Working.to_string(),
            "working",
            "the editable archive"
        );
        assert_eq!(Scope::Layer(2).to_string(), "layer:2", "a layer index");
    }
}
