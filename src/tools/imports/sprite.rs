//! The sprite side of the image imports: one picture in, one named sprite
//! lump out.
//!
//! [`import_sprite_run`] reuses [`build_patch`] from [`super::image_pipeline`]
//! and adds what is sprite-specific. The lump name comes from the `Doom`
//! sprite grammar of [`crate::gfx::sprite_name::SpriteName::parse`], either
//! as one whole `name` or as `sprite`/`frame`/`rotation` parts (plus an
//! optional `mirror_rotation`) that this module validates through the
//! grammar's own part parsers, assembles with [`SpriteName::format`] and
//! checks as the name they must satisfy. The
//! drawing offsets default to standing the picture on its origin
//! ([`OffsetDefault::CenterBottom`]) unless a `PNG` `grAb` chunk says
//! otherwise. Before the edit, [`warnings_for`] reads the merged sprites
//! region and reports the rotations another lump already serves and the
//! frames that end up mixing the all-angles rotation with per-view
//! rotations: engine-visible ambiguities the caller should know about, none
//! of which refuse the import.

use alloc::collections::BTreeSet;

use rmcp::ErrorData;
use rmcp::model::CallToolResult;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CyberError;
use crate::gfx::sprite_name::{
    self, ALL_ANGLES, MAX_ROTATION, SpriteName, parse_frame_letter, parse_sprite_prefix,
};
use crate::session::{Scope, Session};
use crate::state::AppState;
use crate::tools::imports::image_pipeline::{
    ImageImportParams, ImportReport, OffsetDefault, Patched, build_patch, parse_import,
};
use crate::tools::imports::{put_lump, region_write};
use crate::tools::lumps::rows::build_rows;
use crate::tools::reply;
use crate::wad::classify::{LumpKind, Namespace};
use crate::wad::name::LumpName;

/// The lowest per-view rotation number.
const FIRST_VIEW: u8 = 1_u8;
/// Frame letter a part-named import assumes without one.
const DEFAULT_FRAME: char = 'A';

/// Arguments for [`crate::server::Cyberdemon::import_image_as_sprite`] and
/// its alias: the shared image-import parameters, whose `name` is the whole
/// sprite lump name here, plus the naming parts.
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ImportSpriteParams {
    /// The shared image-import parameters: `source`, `name` (the complete
    /// sprite lump name, e.g. `"TROOA2A8"`; nothing is derived from the
    /// file), `palette`, `transparency`, `offsets` and `replace` (see
    /// `skill/reference/imports.md`).
    #[serde(flatten)]
    pub(super) common: ImageImportParams,
    /// Four-character sprite prefix to build the name from, e.g. `"TROO"`
    /// (case-insensitive `ASCII` letters or digits). Give this with
    /// `frame`/`rotation` instead of `name`, never both.
    #[serde(default)]
    pub(super) sprite: Option<String>,
    /// Frame letter the image serves: one character of `A`-`Z` then `[`,
    /// `\`, `]` (case-insensitive, default `"A"`). With `sprite`.
    #[serde(default)]
    pub(super) frame: Option<String>,
    /// Rotation the image serves, `0`-`8` (default `0`: the all-angles
    /// image). With `sprite`.
    #[serde(default)]
    pub(super) rotation: Option<u8>,
    /// Rotation `1`-`8` the same image serves horizontally mirrored, which
    /// extends the name to the pair form: rotation 2 with mirror 8 gives
    /// `xxxxA2A8`. It must differ from `rotation`, which must then be a
    /// view `1`-`8`. With `sprite`.
    #[serde(default)]
    pub(super) mirror_rotation: Option<u8>,
}

/// The full documentation of the sprite import, shared by the tool and its
/// alias; [`ALIASED_SPRITE_TOOL_DESCRIPTION`] is the alias's variant.
pub(super) const SPRITE_TOOL_DESCRIPTION: &str = "Imports an image file (PNG, JPEG, GIF or BMP) as a Doom sprite lump: quantized with alpha holes and stored as a patch in the working archive's sprites region as one undoable edit, named by the sprite grammar - name \"TROOA1\" or \"TROOA2A8\" (mirrored), or sprite with frame, rotation and mirror_rotation. Prefer create_sprite_from_image, which also removes the background, trims and scales.";

/// [`SPRITE_TOOL_DESCRIPTION`] with the alias note on the front.
pub(super) const ALIASED_SPRITE_TOOL_DESCRIPTION: &str = "Alias of import_image_as_sprite for clients that think in PNG: it accepts any supported image and is otherwise identical - same naming forms, sprites-region placement, undoable edit and result. See import_image_as_sprite (prefer create_sprite_from_image for a finished sprite).";

/// The validated naming of one sprite import: the lump name to store under
/// and the frames and rotations its image serves.
#[derive(Debug)]
pub(super) struct Naming {
    /// The whole sprite lump name (`TROOA2A8`).
    pub(super) name: LumpName,
    /// The grammar that name answers with.
    pub(super) parsed: SpriteName,
}

/// One other sprite-region lump's coverage of the rotation space.
#[derive(Debug)]
struct Served {
    /// The lump's name as text.
    lump: String,
    /// The frame letters and rotations its own name serves, the image's
    /// pair first.
    pairs: Vec<(char, u8)>,
}

/// Everything one sprite import commits: the encoded picture with its
/// naming, the warnings read before the edit and the overwrite rule.
struct SpritePlan {
    /// The `Doom` patch bytes and the shared pipeline report.
    patched: Patched,
    /// The validated lump name and grammar.
    naming: Naming,
    /// What the merged sprites region already served.
    warnings: Vec<String>,
    /// Whether a same-name working lump may be overwritten.
    replace: bool,
}

/// Structured result of [`crate::server::Cyberdemon::import_image_as_sprite`]
/// and its alias: the shared pipeline report with the naming facts and the
/// rotation warnings.
#[derive(Serialize)]
pub(super) struct SpriteReport {
    /// The name the sprite lump was stored under (`TROOA2A8`).
    name: LumpName,
    /// Its directory position in the working archive.
    lump_index: usize,
    /// The four-character sprite prefix.
    sprite: String,
    /// The frame letter the image itself serves.
    frame: char,
    /// The rotation the image itself serves; `0` is every angle.
    rotation: u8,
    /// The rotation the horizontally mirrored image serves, `null` for a
    /// six-character name.
    mirror: Option<u8>,
    /// Rotations other sprite lumps of the session already serve, and
    /// frames mixing the all-angles image with per-view rotations.
    warnings: Vec<String>,
    /// The shared `width`, `height`, `left_offset`, `top_offset`,
    /// `transparent_pixels`, `colors_used` and `max_color_error`.
    #[serde(flatten)]
    pipeline: ImportReport,
}

/// Which sprite lump one import names: exactly one of the whole `name` and
/// the `sprite` with its `frame` and `rotation` parts.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for both selectors or neither, and for
/// any text outside the sprite naming grammar.
fn parse_naming(args: &ImportSpriteParams) -> Result<Naming, CyberError> {
    match (args.common.name.as_deref(), args.sprite.as_deref()) {
        (Some(text), None) => whole_name(text),
        (None, Some(prefix)) => part_name(
            prefix,
            args.frame.as_deref(),
            args.rotation,
            args.mirror_rotation,
        ),
        (Some(_), Some(_)) => Err(CyberError::InvalidArgument(
            "import_image_as_sprite takes either name or sprite, not both: name one lump like \
             \"TROOA2A8\", or give the four-character sprite with frame and rotation"
                .to_owned(),
        )),
        (None, None) => Err(CyberError::InvalidArgument(
            "import_image_as_sprite needs a sprite to name: pass the full lump name like \
             \"TROOA2A8\", or the four-character sprite with an optional frame and rotation"
                .to_owned(),
        )),
    }
}

/// The whole-name form: a lump name that must carry the sprite grammar.
///
/// # Errors
///
/// [`LumpName::parse`]' [`CyberError::InvalidArgument`] for text that is no
/// lump name, and [`grammar_error`] for a name outside the sprite grammar.
pub(super) fn whole_name(text: &str) -> Result<Naming, CyberError> {
    let name = LumpName::parse(text)?;
    let parsed = SpriteName::parse(&name.to_string()).ok_or_else(|| grammar_error(text))?;
    Ok(Naming { name, parsed })
}

/// The parts form: the four prefix characters, the frame letter and the
/// rotation, with the optional mirrored rotation appended, assembled and
/// validated through [`pair_name`].
///
/// # Errors
///
/// [`parse_prefix`], [`parse_frame`], [`parse_rotation`] and
/// [`parse_mirror`]' [`CyberError::InvalidArgument`] errors, and
/// [`pair_name`]' grammar error.
pub(super) fn part_name(
    sprite: &str,
    frame: Option<&str>,
    rotation: Option<u8>,
    mirror_rotation: Option<u8>,
) -> Result<Naming, CyberError> {
    let prefix = parse_prefix(sprite)?;
    let letter = parse_frame(frame)?;
    let number = parse_rotation(rotation)?;
    let mirror = parse_mirror(mirror_rotation, number)?;
    pair_name(&prefix, letter, number, mirror)
}

/// The [`Naming`] of one validated prefix, frame and rotation, with the
/// view the mirrored image serves appended as the pair form's second
/// frame+rotation when given: the name is assembled by
/// [`SpriteName::format`] and checked by [`whole_name`], so the parts can
/// only name a lump the grammar reads.
///
/// # Errors
///
/// [`whole_name`]' [`CyberError::InvalidArgument`] for a name outside the
/// sprite grammar.
pub(super) fn pair_name(
    prefix: &str,
    frame: char,
    rotation: u8,
    mirror: Option<u8>,
) -> Result<Naming, CyberError> {
    let parsed = SpriteName {
        sprite: prefix.to_owned(),
        frame,
        rotation,
        mirror: mirror.map(|second| (frame, second)),
    };
    whole_name(&parsed.format())
}

/// The `invalid_argument` explaining the sprite lump name grammar.
fn grammar_error(text: &str) -> CyberError {
    CyberError::InvalidArgument(format!(
        "{text:?} is not a sprite lump name: expected PPPPFN or PPPPFNFN - four ASCII letters \
         or digits for the sprite, one frame letter A-Z, [, \\ or ] and one rotation digit 0-8, \
         where a second frame+rotation pair is served by the same image mirrored (e.g. \
         \"TROOA1\", \"TROOA2A8\", \"PLAYA0\"); pass sprite with frame and rotation to have the \
         name built instead"
    ))
}

/// The four-character sprite prefix: `ASCII` letters or digits,
/// uppercased by the grammar's own [`parse_sprite_prefix`].
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the rejected text.
pub(super) fn parse_prefix(text: &str) -> Result<String, CyberError> {
    parse_sprite_prefix(text).map_err(|_error| {
        CyberError::InvalidArgument(format!(
            "sprite {text:?} is not four ASCII letters or digits: pass the four-character sprite \
             prefix, e.g. \"TROO\""
        ))
    })
}

/// The frame letter: one character of `A`-`Z` then `[`, `\`, `]`,
/// uppercased by [`parse_frame_letter`], defaulting to [`DEFAULT_FRAME`].
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for any other text.
pub(super) fn parse_frame(text: Option<&str>) -> Result<char, CyberError> {
    let Some(raw) = text else {
        return Ok(DEFAULT_FRAME);
    };
    parse_frame_letter(raw).map_err(|_error| {
        CyberError::InvalidArgument(format!(
            "frame {text:?} is not one sprite frame letter: pass a single character A-Z, [, \\ \
             or ]"
        ))
    })
}

/// The rotation number: `0`-`8`, checked by
/// [`crate::gfx::sprite_name::parse_rotation`], defaulting to the
/// all-angles image.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the rejected number.
pub(super) fn parse_rotation(value: Option<u8>) -> Result<u8, CyberError> {
    value.map_or(Ok(ALL_ANGLES), |rotation| {
        sprite_name::parse_rotation(rotation).map_err(|_error| {
            CyberError::InvalidArgument(format!(
                "rotation {rotation} is out of range: pass 0 for the all-angles image, or 1 to \
                 {MAX_ROTATION} for one of the eight views"
            ))
        })
    })
}

/// The rotation the mirrored image serves: one of the eight views, never
/// the rotation the image itself already serves.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a number outside `1..=8`, for one
/// equal to `rotation` and for any mirror on the all-angles rotation.
pub(super) fn parse_mirror(value: Option<u8>, rotation: u8) -> Result<Option<u8>, CyberError> {
    let Some(mirror) = value else {
        return Ok(None);
    };
    if !(FIRST_VIEW..=MAX_ROTATION).contains(&mirror) {
        return Err(CyberError::InvalidArgument(format!(
            "mirror_rotation {mirror} is out of range: pass one of the eight views 1 to \
             {MAX_ROTATION}; the all-angles rotation 0 has no mirror partner"
        )));
    }
    if rotation == ALL_ANGLES {
        return Err(CyberError::InvalidArgument(
            "mirror_rotation needs a rotation of 1 to 8: rotation 0 already serves every view \
             with one image, so it has no mirror partner"
                .to_owned(),
        ));
    }
    if mirror == rotation {
        return Err(CyberError::InvalidArgument(format!(
            "mirror_rotation {mirror} equals rotation: pass another view 1 to {MAX_ROTATION} for \
             the mirrored image to serve, e.g. rotation 2 with mirror_rotation 8"
        )));
    }
    Ok(Some(mirror))
}

/// The frame/rotation pairs a parsed sprite name serves: the image's own
/// pair, then the mirrored one an eight-character name carries.
fn served_pairs(parsed: &SpriteName) -> Vec<(char, u8)> {
    let mut pairs = vec![(parsed.frame, parsed.rotation)];
    if let Some(mirror) = parsed.mirror {
        pairs.push(mirror);
    }
    pairs
}

/// The distinct frame letters a set of pairs touches, in the order the
/// pairs first carry them.
fn served_frames(pairs: &[(char, u8)]) -> Vec<char> {
    let mut frames: Vec<char> = Vec::new();
    for (frame, _rotation) in pairs.iter().copied() {
        if !frames.contains(&frame) {
            frames.push(frame);
        }
    }
    frames
}

/// Whether any of `pairs` serves `frame` as the all-angles image (`angles`)
/// or as one of the eight views.
fn serves(pairs: &[(char, u8)], frame: char, angles: bool) -> bool {
    pairs
        .iter()
        .any(|&(held, rotation)| held == frame && (rotation == ALL_ANGLES) == angles)
}

/// The sprite-region lump's coverage under `sprite`'s prefix, `None` for a
/// name the grammar does not read.
fn served_row(text: &str, sprite: &str) -> Option<Served> {
    let parsed = SpriteName::parse(text)?;
    (parsed.sprite == sprite).then(|| Served {
        lump: text.to_owned(),
        pairs: served_pairs(&parsed),
    })
}

/// The data lumps of the merged view's sprite regions, as their names.
///
/// # Errors
///
/// [`build_rows`]' errors for the merged scope.
fn region_lump_names(session: &Session) -> Result<Vec<String>, CyberError> {
    Ok(build_rows(session, Scope::Merged)?
        .into_iter()
        .filter(|row| row.kind != LumpKind::Marker && row.namespace == Some(Namespace::Sprites))
        .map(|row| row.name.to_string())
        .collect())
}

/// The other sprite lumps an engine sees beside the image being written:
/// the merged region's names under the import's prefix, the import's own
/// name and a repeated name's second copy left out.
///
/// # Errors
///
/// [`region_lump_names`]' errors for the merged scope.
fn covering_lumps(
    parsed: &SpriteName,
    name: LumpName,
    session: &Session,
) -> Result<Vec<Served>, CyberError> {
    let own = name.to_string();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut others: Vec<Served> = Vec::new();
    for text in region_lump_names(session)? {
        if text == own || !seen.insert(text.clone()) {
            continue;
        }
        others.extend(served_row(&text, &parsed.sprite));
    }
    Ok(others)
}

/// The warnings for one sprite import: every rotation of its sprite another
/// lump already serves, and every frame that ends up mixing the all-angles
/// image with per-view rotations.
fn coverage_warnings(parsed: &SpriteName, others: &[Served]) -> Vec<String> {
    let pairs = served_pairs(parsed);
    let mut warnings: Vec<String> = Vec::new();
    for (frame, rotation) in pairs.iter().copied() {
        warnings.extend(
            others
                .iter()
                .filter(|other| other.pairs.contains(&(frame, rotation)))
                .map(|other| overlap_text(parsed, frame, rotation, other)),
        );
    }
    for frame in served_frames(&pairs) {
        warnings.extend(mixing_warning(parsed, &pairs, frame, others));
    }
    warnings
}

/// The warning naming one rotation two lumps serve.
fn overlap_text(parsed: &SpriteName, frame: char, rotation: u8, other: &Served) -> String {
    format!(
        "{} frame {frame} rotation {rotation} is also served by {}: an engine sees two images \
         for that rotation, so remove one of the lumps",
        parsed.sprite, other.lump
    )
}

/// The warning for a frame carrying both the all-angles image and per-view
/// rotations.
fn mixing_text(parsed: &SpriteName, frame: char, other: &str) -> String {
    format!(
        "{} frame {frame} mixes the all-angles rotation 0 with per-view rotations ({other}): \
         engines disagree on which image serves rotations 1 to {MAX_ROTATION}",
        parsed.sprite
    )
}

/// The warning for `frame`: set when the merged region holds the other kind
/// of image for that frame than the import brings.
fn mixing_warning(
    parsed: &SpriteName,
    pairs: &[(char, u8)],
    frame: char,
    others: &[Served],
) -> Option<String> {
    let brings_angles = serves(pairs, frame, true);
    let brings_views = serves(pairs, frame, false);
    others.iter().find_map(|other| {
        let clashes = (brings_angles && serves(&other.pairs, frame, false))
            || (brings_views && serves(&other.pairs, frame, true));
        clashes.then(|| mixing_text(parsed, frame, &other.lump))
    })
}

/// The rotation-coverage warnings one import answers with, read from the
/// merged sprites region.
///
/// # Errors
///
/// [`covering_lumps`]' errors for the merged scope.
pub(super) fn warnings_for(session: &Session, naming: &Naming) -> Result<Vec<String>, CyberError> {
    let others = covering_lumps(&naming.parsed, naming.name, session)?;
    Ok(coverage_warnings(&naming.parsed, &others))
}

/// `import_image_as_sprite`' work: validate the naming, run the shared
/// picture pipeline, read what the sprites region already serves, then
/// commit the lump as one undoable edit.
///
/// The `source` is fetched before any lock is taken and the warnings are
/// read in their own session access, so the edit itself stays synchronous.
///
/// # Errors
///
/// [`parse_naming`]' and [`parse_import`]' argument errors,
/// [`build_patch`]' source, picture and palette errors,
/// [`warnings_for`]' read errors and [`put_lump`]' placement and
/// `already_exists` errors; a refusal leaves the working archive untouched.
pub(super) async fn import_sprite_run(
    state: &AppState,
    args: &ImportSpriteParams,
) -> Result<SpriteReport, CyberError> {
    let naming = parse_naming(args)?;
    let mut import = parse_import(&args.common)?;
    import.name = Some(naming.name);
    let patched = build_patch(state, &import, OffsetDefault::CenterBottom).await?;
    let warnings = state.with_session(|session| warnings_for(session, &naming))?;
    let plan = SpritePlan {
        patched,
        naming,
        warnings,
        replace: import.replace,
    };
    state.with_session(move |session| commit_sprite(session, plan))
}

/// The synchronous half of a sprite import: one undoable edit placing the
/// patch in the working archive's sprites region.
///
/// # Errors
///
/// [`put_lump`]' placement and `already_exists` errors.
fn commit_sprite(session: &mut Session, plan: SpritePlan) -> Result<SpriteReport, CyberError> {
    let SpritePlan {
        patched,
        naming,
        warnings,
        replace,
    } = plan;
    let Naming { name, parsed } = naming;
    let label = format!("import_sprite {name}");
    let Patched { data, report, .. } = patched;
    session.edit(&label, move |wad| {
        let write = region_write(name, &data, Namespace::Sprites, replace);
        let lump_index = put_lump(wad, &write)?;
        Ok(SpriteReport {
            name,
            lump_index,
            sprite: parsed.sprite,
            frame: parsed.frame,
            rotation: parsed.rotation,
            mirror: parsed.mirror.map(|(_frame, rotation)| rotation),
            warnings,
            pipeline: report,
        })
    })
}

/// The shared body of [`crate::server::Cyberdemon::import_image_as_sprite`]
/// and its alias.
pub(super) async fn sprite_imported(
    state: &AppState,
    args: &ImportSpriteParams,
) -> Result<CallToolResult, ErrorData> {
    reply(import_sprite_run(state, args).await)
}

#[cfg(test)]
mod tests {
    use super::{
        ImportSpriteParams, Served, coverage_warnings, covering_lumps, grammar_error, pair_name,
        parse_frame, parse_mirror, parse_naming, parse_prefix, parse_rotation, served_frames,
        served_pairs,
    };
    use crate::error::CyberError;
    use crate::gfx::sprite_name::SpriteName;
    use crate::session::Session;
    use crate::tools::imports::image_pipeline::ImageImportParams;
    use crate::wad::archive::Lump;
    use crate::wad::name::LumpName;

    /// The error code of a [`CyberError`], for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    /// A parsed test lump name.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// The grammar of one test sprite name.
    fn parsed(text: &str) -> SpriteName {
        SpriteName::parse(text).expect("test names are sprite names")
    }

    /// Sprite-import arguments over one source with nothing else set.
    fn bare() -> ImportSpriteParams {
        ImportSpriteParams {
            common: ImageImportParams {
                source: "/x/y.png".to_owned(),
                name: None,
                palette: None,
                transparency: None,
                offsets: None,
                replace: false,
            },
            sprite: None,
            frame: None,
            rotation: None,
            mirror_rotation: None,
        }
    }

    /// Arguments naming the sprite with one whole lump name.
    fn by_name(text: &str) -> ImportSpriteParams {
        let mut args = bare();
        args.common.name = Some(text.to_owned());
        args
    }

    /// Arguments naming the sprite by its four-character prefix.
    fn by_sprite(text: &str) -> ImportSpriteParams {
        let mut args = bare();
        args.sprite = Some(text.to_owned());
        args
    }

    /// The rotation parts a part-named call asks for.
    fn with_views(
        args: ImportSpriteParams,
        frame: Option<&str>,
        rotation: Option<u8>,
        mirror: Option<u8>,
    ) -> ImportSpriteParams {
        let mut filled = args;
        filled.frame = frame.map(str::to_owned);
        filled.rotation = rotation;
        filled.mirror_rotation = mirror;
        filled
    }

    /// One coverage fixture: a lump name and the pairs it serves.
    fn served(lump: &str, pairs: &[(char, u8)]) -> Served {
        Served {
            lump: lump.to_owned(),
            pairs: pairs.to_vec(),
        }
    }

    /// The seed lump for one `text`: a marker for the region markers, a
    /// short patch for everything else.
    fn seed_lump(text: &str) -> Lump {
        if text.starts_with("S_") {
            return Lump::marker(name(text));
        }
        Lump::new(name(text), vec![0_u8; 8])
    }

    /// A session whose working archive holds a sprites region with `names`
    /// and one loose lump of the same grammar outside it.
    fn session_with(names: &[&str]) -> Session {
        let mut session = Session::default();
        let lumps: Vec<Lump> = names.iter().copied().map(seed_lump).collect();
        session
            .edit("seed", move |wad| {
                wad.lumps.extend(lumps);
                wad.lumps.push(Lump::new(name("TROOA4"), vec![0_u8; 8]));
                Ok(())
            })
            .expect("plain append");
        session
    }

    #[test]
    fn naming_reads_a_whole_lump_name() {
        let pair = parse_naming(&by_name("TROOA2A8")).expect("the pair form");
        assert_eq!(pair.name, name("TROOA2A8"), "the name is stored as given");
        assert_eq!(pair.parsed.sprite, "TROO", "the prefix");
        assert_eq!((pair.parsed.frame, pair.parsed.rotation), ('A', 2));
        assert_eq!(pair.parsed.mirror, Some(('A', 8)), "the mirrored pair");
        let loose = parse_naming(&by_name("troo]8")).expect("lowercase is a lump name too");
        assert_eq!(loose.name, name("TROO]8"), "uppercased like any lump name");
        assert_eq!((loose.parsed.frame, loose.parsed.rotation), (']', 8));
    }

    #[test]
    fn naming_builds_the_name_from_its_parts() {
        let plain = parse_naming(&by_sprite("boba")).expect("the prefix alone names a lump");
        assert_eq!(plain.name, name("BOBAA0"), "frame A, rotation 0");
        assert_eq!(plain.parsed.mirror, None, "nothing mirrored");
        let pair = with_views(by_sprite("TROO"), Some("a"), Some(2_u8), Some(8_u8));
        let named = parse_naming(&pair).expect("the pair form");
        assert_eq!(named.name, name("TROOA2A8"), "the assembled name");
        assert_eq!(named.parsed.mirror, Some(('A', 8)), "the mirrored pair");
    }

    #[test]
    fn pair_name_formats_the_validated_parts() {
        let plain = pair_name("TROO", 'A', 0, None).expect("the plain form");
        assert_eq!(plain.name, name("TROOA0"), "the all-angles name");
        assert_eq!(plain.parsed.mirror, None, "nothing mirrored");
        let pair = pair_name("TROO", '[', 1, Some(5_u8)).expect("the pair form");
        assert_eq!(pair.name, name("TROO[1[5"), "the past-Z pair");
        assert_eq!(pair.parsed.frame, '[', "the direct pair");
        assert_eq!(pair.parsed.mirror, Some(('[', 5)), "the mirrored pair");
    }

    #[test]
    fn naming_takes_exactly_one_of_its_two_forms() {
        let mut both = by_name("TROOA1");
        both.sprite = Some("TROO".to_owned());
        let clash = parse_naming(&both).expect_err("two selectors");
        assert_eq!(code(&clash), "invalid_argument");
        assert!(
            clash.to_string().contains("not both"),
            "the message says which: {clash}"
        );
        let none = parse_naming(&bare()).expect_err("no selector");
        assert_eq!(code(&none), "invalid_argument");
        assert!(
            none.to_string().contains("needs a sprite"),
            "the message asks for one: {none}"
        );
    }

    #[test]
    fn naming_refuses_text_outside_the_grammar() {
        for text in ["TOOLONG1", "TROOA9", "TROOAA", "TROOA0A8", "TROO_A1"] {
            let error =
                parse_naming(&by_name(text)).expect_err(&format!("{text} is no sprite name"));
            assert_eq!(code(&error), "invalid_argument", "{text} code");
            assert!(
                error.to_string().contains("sprite lump name"),
                "the message explains the grammar for {text}: {error}"
            );
        }
        assert!(
            grammar_error("X").to_string().contains("TROOA2A8"),
            "the grammar message carries an example"
        );
    }

    #[test]
    fn the_prefix_and_frame_parts_are_validated() {
        for text in ["TRO", "TROOO", "TRO!", ""] {
            assert_eq!(
                code(&parse_prefix(text).expect_err(&format!("{text} is no prefix"))),
                "invalid_argument",
                "{text} code"
            );
        }
        assert_eq!(parse_prefix("b4r1").expect("digits are allowed"), "B4R1");
        assert_eq!(parse_frame(None).expect("the default"), 'A');
        assert_eq!(parse_frame(Some("]")).expect("the last frame"), ']');
        for text in ["AA", "9", " "] {
            assert_eq!(
                code(&parse_frame(Some(text)).expect_err(&format!("{text} is no frame"))),
                "invalid_argument",
                "{text} code"
            );
        }
    }

    #[test]
    fn the_rotation_part_takes_the_nine_rotations() {
        assert_eq!(parse_rotation(None).expect("the default"), 0_u8);
        for rotation in 0_u8..=8_u8 {
            assert_eq!(
                parse_rotation(Some(rotation)).expect("a rotation"),
                rotation
            );
        }
        let error = parse_rotation(Some(9_u8)).expect_err("no ninth view");
        assert_eq!(code(&error), "invalid_argument");
        assert!(
            error.to_string().contains("out of range"),
            "the message names the rule: {error}"
        );
    }

    #[test]
    fn the_mirror_part_needs_a_differing_view() {
        assert_eq!(parse_mirror(None, 3_u8).expect("absent"), None);
        assert_eq!(parse_mirror(Some(7_u8), 3_u8).expect("a pair"), Some(7_u8));
        let cases = [
            (0_u8, 3_u8, "out of range"),
            (9_u8, 3_u8, "out of range"),
            (8_u8, 0_u8, "needs a rotation"),
            (3_u8, 3_u8, "equals rotation"),
        ];
        for (mirror, rotation, expected) in cases {
            let error = parse_mirror(Some(mirror), rotation).expect_err("a refused pair");
            assert_eq!(code(&error), "invalid_argument", "{mirror} over {rotation}");
            assert!(
                error.to_string().contains(expected),
                "the message says {expected:?}: {error}"
            );
        }
    }

    #[test]
    fn pairs_and_frames_follow_the_name() {
        assert_eq!(served_pairs(&parsed("TROOA5")), vec![('A', 5)]);
        assert_eq!(
            served_pairs(&parsed("TROOA1B5")),
            vec![('A', 1), ('B', 5)],
            "the mirrored pair comes second"
        );
        assert_eq!(served_frames(&served_pairs(&parsed("TROOA2A8"))), vec!['A']);
        assert_eq!(
            served_frames(&served_pairs(&parsed("TROOA1B5"))),
            vec!['A', 'B'],
            "each touched frame once"
        );
    }

    #[test]
    fn warnings_name_a_rotation_another_lump_already_serves() {
        let others = [served("TROOA2A8", &[('A', 2), ('A', 8)])];
        let warnings = coverage_warnings(&parsed("TROOA2"), &others);
        assert_eq!(warnings.len(), 1, "one overlap: {warnings:?}");
        assert!(warnings[0].contains("rotation 2"), "{}", warnings[0]);
        assert!(warnings[0].contains("TROOA2A8"), "{}", warnings[0]);
        assert!(
            coverage_warnings(&parsed("TROOA7"), &others).is_empty(),
            "rotation 7 is free"
        );
    }

    #[test]
    fn warnings_name_a_frame_mixing_angles_with_views() {
        let views = [served("TROOA1", &[('A', 1)])];
        let angles = coverage_warnings(&parsed("TROOA0"), &views);
        assert_eq!(angles.len(), 1, "one mixed frame: {angles:?}");
        assert!(angles[0].contains("mixes"), "{}", angles[0]);
        assert!(angles[0].contains("TROOA1"), "{}", angles[0]);
        let per_view = coverage_warnings(&parsed("PLAYA1"), &[served("PLAYA0", &[('A', 0)])]);
        assert_eq!(per_view.len(), 1, "the other direction warns too");
        assert!(
            coverage_warnings(&parsed("TROOA3"), &views).is_empty(),
            "two views do not mix"
        );
    }

    #[test]
    fn a_mixed_frame_warns_once_whatever_it_carries() {
        let others = [served("TROOA0", &[('A', 0)])];
        let warnings = coverage_warnings(&parsed("TROOA2A8"), &others);
        assert_eq!(warnings.len(), 1, "one warning: {warnings:?}");
        assert!(
            warnings[0].contains("mixes"),
            "the pair's two rotations share one mixing warning: {}",
            warnings[0]
        );
    }

    #[test]
    fn covering_lumps_reads_the_sprites_region_of_the_merged_view() {
        let session = session_with(&[
            "S_START", "TROOA1", "TROOA2A8", "TROOA2A8", "PLAYA0", "S_END",
        ]);
        let others =
            covering_lumps(&parsed("TROOA2"), name("TROOA2"), &session).expect("the region");
        let lumps: Vec<&str> = others.iter().map(|other| other.lump.as_str()).collect();
        assert_eq!(
            lumps,
            ["TROOA1", "TROOA2A8"],
            "same prefix only, the own name and the repeated copy left out"
        );
    }

    #[test]
    fn the_wire_form_deserializes_the_naming_parts() {
        let args: ImportSpriteParams = serde_json::from_value(serde_json::json!({
            "source": "/x/y.png",
            "sprite": "TROO",
            "frame": "B",
            "rotation": 2_u64,
            "mirror_rotation": 8_u64,
            "offsets": "center",
            "replace": true,
        }))
        .expect("the wire form");
        assert_eq!(args.sprite.as_deref(), Some("TROO"));
        assert_eq!(args.frame.as_deref(), Some("B"));
        assert_eq!(args.rotation, Some(2_u8));
        assert_eq!(args.mirror_rotation, Some(8_u8));
        assert!(args.common.replace, "the shared flag flattens in");
        let named = parse_naming(&args).expect("the parts name a lump");
        assert_eq!(named.name, name("TROOB2B8"), "the frame carries the mirror");
    }
}
