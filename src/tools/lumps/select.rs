//! The shared lump selector behind every tool that reads one lump.
//!
//! [`LumpSelector`] is the parameter half: a name plus the optional `scope`,
//! `namespace` and `occurrence` refinements, flattened into each tool's
//! arguments. [`resolve`] is the read half: it parses the selector, lists
//! the scope's [`LumpRow`]s through [`build_rows`], keeps the same-name
//! matches (inside `namespace` when given) and picks one. The default
//! `occurrence` is the **last** match in directory order, the copy an
//! engine's lookup of that name reaches for.

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
    choose(matched, name, scope, selector.occurrence)
}

/// Take `occurrence` out of `matched` (directory order), the last match
/// when it is `None`.
///
/// # Errors
///
/// [`CyberError::NotFound`] naming the scope when nothing matched, or
/// naming how many lumps matched when `occurrence` is past the end.
fn choose(
    matched: Vec<LumpRow>,
    name: LumpName,
    scope: Scope,
    occurrence: Option<usize>,
) -> Result<LumpRow, CyberError> {
    let count = matched.len();
    let Some(last) = count.checked_sub(1) else {
        return Err(
            CyberError::NotFound(format!("lump {name} not found in {scope}"))
                .with_hint("search_lumps finds lumps by pattern"),
        );
    };
    let wanted = occurrence.unwrap_or(last);
    matched.into_iter().nth(wanted).ok_or_else(|| {
        CyberError::NotFound(format!(
            "only {count} lumps named {name} in {scope}: occurrence {wanted} is out of range \
             (0 to {last})"
        ))
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{LumpSelector, resolve};
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
