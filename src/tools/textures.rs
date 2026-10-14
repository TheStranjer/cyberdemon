//! Texture tools: the composite-texture definitions of the merged view and
//! the copy-on-write `PNAMES` editing behind `edit_pnames`.
//!
//! [`TextureSet`] loads the merged `PNAMES`, `TEXTURE1` and `TEXTURE2`
//! lumps (the last copy of each name, decoded with the codecs in
//! [`crate::wad::textures`]); `list_textures` and `list_pnames` page
//! through it and `edit_pnames` rewrites it, renumbering every texture
//! patch reference so the indices stay truthful. Every write goes through
//! exactly one [`crate::session::Session::edit`], bringing the lumps into
//! the working archive as overrides.

use alloc::collections::BTreeSet;
use alloc::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CyberError;
use crate::server::Cyberdemon;
use crate::session::{MergedLump, Scope, Session, Source};
use crate::tools::lumps::rows::build_rows;
use crate::tools::{fail, ok};
use crate::wad::archive::{Lump, Wad};
use crate::wad::classify::Namespace;
use crate::wad::name::{LumpName, glob_match};
use crate::wad::textures::{
    PatchMapping, TextureDef, TextureLump, dropped_references, parse_pnames, parse_texture_lump,
    remap_patches, write_pnames, write_texture_lump,
};

/// Default page size for the listing tools.
const DEFAULT_LIMIT: usize = 200;
/// Largest page either listing tool will return.
const MAX_LIMIT: usize = 2000;
/// Raw directory bytes of the patch-name lump.
const PNAMES_RAW: [u8; 8] = *b"PNAMES\0\0";
/// Raw directory bytes of the first texture lump.
const TEXTURE1_RAW: [u8; 8] = *b"TEXTURE1";
/// Raw directory bytes of the second texture lump.
const TEXTURE2_RAW: [u8; 8] = *b"TEXTURE2";

/// Arguments for [`Cyberdemon::list_textures`].
#[derive(Debug, Deserialize, JsonSchema)]
struct ListTexturesParams {
    /// Case-insensitive texture-name glob: `*` matches any run of
    /// characters, `?` exactly one, e.g. `"WALL*"`.
    #[serde(default)]
    pattern: Option<String>,
    /// Matching textures to skip before the page (default 0).
    #[serde(default)]
    offset: Option<usize>,
    /// Maximum rows in the result (default 200, at most 2000).
    #[serde(default)]
    limit: Option<usize>,
}

/// Arguments for [`Cyberdemon::list_pnames`].
#[derive(Debug, Deserialize, JsonSchema)]
struct ListPnamesParams {
    /// Case-insensitive patch-name glob: `*` matches any run of characters,
    /// `?` exactly one, e.g. `"WALL*"`.
    #[serde(default)]
    pattern: Option<String>,
    /// Matching entries to skip before the page (default 0).
    #[serde(default)]
    offset: Option<usize>,
    /// Maximum rows in the result (default 200, at most 2000).
    #[serde(default)]
    limit: Option<usize>,
}

/// One `from`/`to` pair of [`EditPnamesParams::rename`].
#[derive(Debug, Deserialize, JsonSchema)]
struct RenamePair {
    /// The current `PNAMES` entry to change: 1 to 8 printable `ASCII`
    /// characters, matched case-insensitively.
    from: String,
    /// Its new name; must not already be in `PNAMES` (indices stay put, so
    /// texture references are untouched).
    to: String,
}

/// Arguments for [`Cyberdemon::edit_pnames`]: exactly one operation.
#[derive(Debug, Deserialize, JsonSchema)]
struct EditPnamesParams {
    /// Patch names to append. Names already present are skipped and
    /// reported in `skipped`.
    #[serde(default)]
    add: Option<Vec<String>>,
    /// Patch names to remove. Without `force`, removing a name a texture
    /// still refers to fails with `conflict`; `force: true` drops those
    /// patch references from the textures.
    #[serde(default)]
    remove: Option<Vec<String>>,
    /// Entries to rename in place: `[{from, to}]`; indices stay put.
    #[serde(default)]
    rename: Option<Vec<RenamePair>>,
    /// The complete new list, replacing every entry. Indexing is by name,
    /// so surviving entries are renumbered in the textures and missing
    /// ones follow the same `conflict`/`force` rule as `remove`; an empty
    /// list clears `PNAMES`.
    #[serde(default)]
    replace_all: Option<Vec<String>>,
    /// Proceed despite dropped references (default `false`). Applies to
    /// `remove` and `replace_all`.
    #[serde(default)]
    force: Option<bool>,
}

/// The one operation an `edit_pnames` call carries.
#[derive(Clone, Copy, Debug)]
enum Operation<'op> {
    /// Append names.
    Add(&'op [String]),
    /// Remove names.
    Remove(&'op [String]),
    /// Rename entries in place.
    Rename(&'op [RenamePair]),
    /// Replace the whole list.
    ReplaceAll(&'op [String]),
}

impl Operation<'_> {
    /// The operation's name, used in the undo label.
    const fn name(self) -> &'static str {
        match self {
            Self::Add(_) => "add",
            Self::Remove(_) => "remove",
            Self::Rename(_) => "rename",
            Self::ReplaceAll(_) => "replace_all",
        }
    }
}

/// The lump a listed texture came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WhichLump {
    /// The merged view's `TEXTURE1` lump.
    Texture1,
    /// The merged view's `TEXTURE2` lump.
    Texture2,
}

impl WhichLump {
    /// The lump name clients see in the `lump` field.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Texture1 => "TEXTURE1",
            Self::Texture2 => "TEXTURE2",
        }
    }
}

/// The merged view's texture definitions, loaded once per tool call.
///
/// `TEXTURE2` entries override `TEXTURE1` ones of the same name in
/// [`TextureSet::find`]; the listings show both lumps, each row tagged
/// with its origin. This is the session-level loader the texture tools
/// read and write through; it is public so embedding callers can reuse
/// the lookup (an unused crate-private item would not survive the
/// `dead_code` check).
#[derive(Debug)]
pub struct TextureSet {
    /// The merged `PNAMES` list; empty when no `PNAMES` lump exists.
    pnames: Vec<String>,
    /// The decoded `TEXTURE1` lump and where it came from.
    texture1: Option<(TextureLump, Source)>,
    /// The decoded `TEXTURE2` lump and where it came from.
    texture2: Option<(TextureLump, Source)>,
}

impl TextureSet {
    /// Reads the last (winning) `PNAMES`, `TEXTURE1` and `TEXTURE2` of the
    /// merged view. Without a `PNAMES` lump the patch indices inside the
    /// texture lumps stay unbounded.
    ///
    /// # Errors
    ///
    /// [`CyberError::Format`] when `PNAMES` is malformed or a texture lump
    /// decodes neither as the `Doom` nor as the `Strife` layout.
    #[inline]
    pub fn load(session: &Session) -> Result<Self, CyberError> {
        let pnames_lump = find_global(session, PNAMES_RAW);
        let pnames = match pnames_lump.as_ref() {
            Some(entry) => parse_pnames(&entry.lump.data)?,
            None => Vec::new(),
        };
        let bound = if pnames_lump.is_some() {
            pnames.len()
        } else {
            usize::MAX
        };
        Ok(Self {
            texture1: decode_texture_lump(find_global(session, TEXTURE1_RAW), bound)?,
            texture2: decode_texture_lump(find_global(session, TEXTURE2_RAW), bound)?,
            pnames,
        })
    }

    /// The texture named `name`, case-insensitively; a `TEXTURE2` entry
    /// wins over a `TEXTURE1` one of the same name.
    #[must_use]
    #[inline]
    pub fn find(&self, name: &str) -> Option<(&TextureDef, WhichLump)> {
        first_match(self.texture2.as_ref(), name)
            .map(|def| (def, WhichLump::Texture2))
            .or_else(|| {
                first_match(self.texture1.as_ref(), name).map(|def| (def, WhichLump::Texture1))
            })
    }

    /// Every texture of both lumps, in lump order, as listing rows.
    fn texture_rows(&self) -> Vec<TextureRow> {
        let mut rows = Vec::new();
        push_rows(&mut rows, self.texture1.as_ref(), WhichLump::Texture1);
        push_rows(&mut rows, self.texture2.as_ref(), WhichLump::Texture2);
        rows
    }

    /// The texture lumps a renumbering would touch, as
    /// `(lump, lump name)` pairs for conflict reporting.
    fn textures(&self) -> Vec<(&TextureLump, &'static str)> {
        let mut list = Vec::new();
        if let Some(pair) = self.texture1.as_ref() {
            list.push((&pair.0, WhichLump::Texture1.as_str()));
        }
        if let Some(pair) = self.texture2.as_ref() {
            list.push((&pair.0, WhichLump::Texture2.as_str()));
        }
        list
    }
}

/// The first definition named `name` (case-insensitive) in one lump entry.
fn first_match<'lump>(
    entry: Option<&'lump (TextureLump, Source)>,
    name: &str,
) -> Option<&'lump TextureDef> {
    let pair = entry?;
    pair.0
        .textures
        .iter()
        .find(|def| def.name.eq_ignore_ascii_case(name))
}

/// The merged view's last lump with the raw directory name `raw`.
fn find_global(session: &Session, raw: [u8; 8]) -> Option<MergedLump> {
    session.find(LumpName::from_raw(raw), None)
}

/// One texture lump decoded together with the archive it came from.
type DecodedLump = Option<(TextureLump, Source)>;

/// Decodes one found texture lump; `None` stays `None`.
///
/// # Errors
///
/// [`crate::wad::textures::parse_texture_lump`]'s [`CyberError::Format`].
fn decode_texture_lump(
    found: Option<MergedLump>,
    pnames_len: usize,
) -> Result<DecodedLump, CyberError> {
    let Some(entry) = found else {
        return Ok(None);
    };
    let lump = parse_texture_lump(&entry.lump.data, pnames_len)?;
    Ok(Some((lump, entry.source)))
}

/// Appends `entry`'s textures to `rows`, tagged with `which` and its source.
fn push_rows(rows: &mut Vec<TextureRow>, entry: Option<&(TextureLump, Source)>, which: WhichLump) {
    let Some(pair) = entry else {
        return;
    };
    for def in &pair.0.textures {
        rows.push(TextureRow {
            name: def.name.clone(),
            width: def.width,
            height: def.height,
            patches: def.patches.len(),
            lump: which.as_str(),
            source: pair.1,
        });
    }
}

/// One `list_textures` row.
#[derive(Debug, Serialize)]
struct TextureRow {
    /// The texture name.
    name: String,
    /// Width in pixels.
    width: u16,
    /// Height in pixels.
    height: u16,
    /// Patch count.
    patches: usize,
    /// Which texture lump defines it: `"TEXTURE1"` or `"TEXTURE2"`.
    lump: &'static str,
    /// The archive that lump was read from.
    source: Source,
}

/// Structured result of [`Cyberdemon::list_textures`].
#[derive(Debug, Serialize)]
struct TexturesReport {
    /// Textures matching `pattern` (before paging).
    total: usize,
    /// The matching textures, in lump order, on this page.
    textures: Vec<TextureRow>,
}

/// One `list_pnames` row.
#[derive(Debug, Serialize)]
struct PnameRow {
    /// Index in `PNAMES` (the number textures refer to).
    index: usize,
    /// The patch name.
    name: String,
    /// Whether a lump with this name exists in the merged view outside the
    /// flats and sprites regions.
    lump_found: bool,
}

/// Structured result of [`Cyberdemon::list_pnames`].
#[derive(Debug, Serialize)]
struct PnamesReport {
    /// Entries matching `pattern` (before paging).
    total: usize,
    /// The matching entries, in `PNAMES` order, on this page.
    pnames: Vec<PnameRow>,
}

/// One renamed pair in [`EditReport::renamed`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct RenameRow {
    /// The old entry name.
    from: String,
    /// The new entry name.
    to: String,
}

/// Structured result of [`Cyberdemon::edit_pnames`].
#[derive(Debug, Serialize)]
struct EditReport {
    /// `PNAMES` entries after the edit.
    pnames: usize,
    /// Names appended by `add` or brought in by `replace_all`.
    added: Vec<String>,
    /// Requested names that needed no change (already present for `add`,
    /// absent for `remove`).
    skipped: Vec<String>,
    /// Names `remove` deleted or `replace_all` dropped.
    removed: Vec<String>,
    /// Pairs `rename` applied.
    renamed: Vec<RenameRow>,
    /// Textures whose patch references were renumbered or pruned by the
    /// edit (always 0 for `add` and `rename`).
    textures_changed: usize,
}

/// The validated page request of the listing tools.
#[derive(Debug)]
struct Page {
    /// Rows to skip before the page.
    offset: usize,
    /// Maximum rows on the page.
    limit: usize,
}

/// The `{offset, limit}` pair a listing tool was given.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a `limit` outside `1..=2000`.
fn parse_page(offset: Option<usize>, limit: Option<usize>) -> Result<Page, CyberError> {
    let value = match limit {
        None => DEFAULT_LIMIT,
        Some(value) if value == 0 || value > MAX_LIMIT => {
            return Err(CyberError::InvalidArgument(format!(
                "limit {value} is out of range: pass a value between 1 and {MAX_LIMIT}"
            )));
        }
        Some(value) => value,
    };
    Ok(Page {
        offset: offset.unwrap_or(0_usize),
        limit: value,
    })
}

/// Whether `name` passes the optional glob filter.
fn passes(name: &str, pattern: Option<&str>) -> bool {
    pattern.is_none_or(|glob| glob_match(name.as_bytes(), glob))
}

/// Keeps the matching rows of `matched` on `page` and counts all matches.
fn page_rows<T>(matched: Vec<T>, page: &Page) -> (usize, Vec<T>) {
    let total = matched.len();
    (
        total,
        matched
            .into_iter()
            .skip(page.offset)
            .take(page.limit)
            .collect(),
    )
}

/// `list_textures`' work over the session.
///
/// # Errors
///
/// [`TextureSet::load`]'s [`CyberError::Format`] for a malformed lump.
fn list_textures_run(
    session: &Session,
    page: &Page,
    pattern: Option<&str>,
) -> Result<TexturesReport, CyberError> {
    let set = TextureSet::load(session)?;
    let matched: Vec<TextureRow> = set
        .texture_rows()
        .into_iter()
        .filter(|row| passes(&row.name, pattern))
        .collect();
    let (total, textures) = page_rows(matched, page);
    Ok(TexturesReport { total, textures })
}

/// `list_pnames`' work over the session.
///
/// # Errors
///
/// [`TextureSet::load`]'s errors and [`build_rows`]' for the merged scope.
fn list_pnames_run(
    session: &Session,
    page: &Page,
    pattern: Option<&str>,
) -> Result<PnamesReport, CyberError> {
    let set = TextureSet::load(session)?;
    let present = present_lump_names(session)?;
    let matched: Vec<PnameRow> = set
        .pnames
        .iter()
        .enumerate()
        .filter(|&(_index, entry)| passes(entry, pattern))
        .map(|(index, entry)| PnameRow {
            lump_found: LumpName::parse(entry).is_ok_and(|parsed| present.contains(&parsed)),
            index,
            name: entry.clone(),
        })
        .collect();
    let (total, pnames) = page_rows(matched, page);
    Ok(PnamesReport { total, pnames })
}

/// The merged-view names that exist as lumps outside the flats and sprites
/// regions: the candidates a `PNAMES` entry can point at.
fn present_lump_names(session: &Session) -> Result<BTreeSet<LumpName>, CyberError> {
    Ok(build_rows(session, Scope::Merged)?
        .into_iter()
        .filter(|row| !matches!(row.namespace, Some(Namespace::Flats | Namespace::Sprites)))
        .map(|row| row.name)
        .collect())
}

/// The whole change one `edit_pnames` call wants, before anything is
/// written.
#[derive(Default)]
struct Plan {
    /// The new complete `PNAMES` list.
    names: Vec<String>,
    /// `mapping[old]` renumbers texture references; `None` when no index
    /// moves.
    mapping: Option<PatchMapping>,
    /// Names appended.
    added: Vec<String>,
    /// Requested names that needed no change.
    skipped: Vec<String>,
    /// Names deleted.
    removed: Vec<String>,
    /// Pairs applied by `rename`.
    renamed: Vec<RenameRow>,
}

/// Computes the plan for one operation: the new list, the renumbering
/// mapping and the report's name lists.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a bad name, an empty `add`/`remove`/
/// `rename` list or a `replace_all` list with duplicates;
/// [`CyberError::NotFound`] for a `rename` whose `from` is absent;
/// [`CyberError::Conflict`] when entries a texture still refers to would
/// vanish without `force`; [`CyberError::LimitExceeded`] over the `u16`
/// index range.
fn plan_edit(
    current: &[String],
    operation: Operation<'_>,
    force: bool,
    textures: &[(&TextureLump, &'static str)],
) -> Result<Plan, CyberError> {
    let plan = match operation {
        Operation::Add(list) => plan_add(current, list)?,
        Operation::Remove(list) => plan_remove(current, list)?,
        Operation::Rename(pairs) => plan_rename(current, pairs)?,
        Operation::ReplaceAll(list) => plan_replace_all(current, list)?,
    };
    if let Some(mapping) = plan.mapping.as_ref() {
        check_dropped(mapping, force, textures)?;
    }
    Ok(plan)
}

/// Parses `list` into uppercased names for `operation`, refusing empties.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an empty list or an unparsable name.
fn name_list(operation: &str, list: &[String]) -> Result<Vec<String>, CyberError> {
    if list.is_empty() {
        return Err(CyberError::InvalidArgument(format!(
            "{operation} needs at least one name: pass a non-empty list"
        )));
    }
    list.iter().map(|text| parsed_name(text)).collect()
}

/// Uppercases one name through [`LumpName::parse`], echoing it in errors.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the rejected text.
fn parsed_name(text: &str) -> Result<String, CyberError> {
    LumpName::parse(text)
        .map(|name| name.to_string())
        .map_err(|error| {
            CyberError::InvalidArgument(format!("PNAMES name {text:?} is not valid: {error}"))
        })
}

/// The position of `name` in `list`, matched case-insensitively.
fn position_of(list: &[String], name: &str) -> Option<usize> {
    list.iter()
        .position(|entry| entry.eq_ignore_ascii_case(name))
}

/// `add`: append the names not present yet.
fn plan_add(current: &[String], list: &[String]) -> Result<Plan, CyberError> {
    let wanted = name_list("add", list)?;
    let mut names = current.to_vec();
    let mut added = Vec::new();
    let mut skipped = Vec::new();
    for name in wanted {
        if position_of(&names, &name).is_some() {
            skipped.push(name);
        } else {
            names.push(name.clone());
            added.push(name);
        }
    }
    Ok(Plan {
        names,
        added,
        skipped,
        ..Plan::default()
    })
}

/// `remove`: drop the named entries; the mapping renumbers what is left.
fn plan_remove(current: &[String], list: &[String]) -> Result<Plan, CyberError> {
    let wanted = name_list("remove", list)?;
    let names: Vec<String> = current
        .iter()
        .filter(|entry| !wanted.iter().any(|name| name == *entry))
        .cloned()
        .collect();
    let mut removed = Vec::new();
    let mut skipped = Vec::new();
    for name in wanted {
        let present = position_of(current, &name).is_some();
        if present && !removed.contains(&name) {
            removed.push(name);
        } else if !present && !skipped.contains(&name) {
            skipped.push(name);
        } else {
            // A repeated request for the same outcome: report it once.
        }
    }
    Ok(Plan {
        mapping: name_mapping(current, &names)?,
        names,
        removed,
        skipped,
        ..Plan::default()
    })
}

/// `rename`: change entries in place; indices (and textures) do not move.
fn plan_rename(current: &[String], pairs: &[RenamePair]) -> Result<Plan, CyberError> {
    if pairs.is_empty() {
        return Err(CyberError::InvalidArgument(
            "rename needs at least one {from, to} pair".to_owned(),
        ));
    }
    let mut names = current.to_vec();
    let mut renamed = Vec::new();
    for pair in pairs {
        let from = parsed_name(&pair.from)?;
        let to = parsed_name(&pair.to)?;
        let at = position_of(&names, &from).ok_or_else(|| {
            CyberError::NotFound(format!(
                "PNAMES holds no entry {from}: list_pnames shows the current names"
            ))
        })?;
        if to != from && position_of(&names, &to).is_some() {
            return Err(CyberError::Conflict(format!(
                "PNAMES already holds {to}: rename to a free name (a later pair may free one)"
            )));
        }
        if let Some(slot) = names.get_mut(at) {
            slot.clone_from(&to);
        }
        renamed.push(RenameRow { from, to });
    }
    Ok(Plan {
        names,
        renamed,
        ..Plan::default()
    })
}

/// `replace_all`: the new complete list; indexing is by name, so the
/// mapping renumbers every surviving reference.
fn plan_replace_all(current: &[String], list: &[String]) -> Result<Plan, CyberError> {
    let names: Vec<String> = list
        .iter()
        .map(|text| parsed_name(text))
        .collect::<Result<_, _>>()?;
    let mut sorted = names.clone();
    sorted.sort_unstable();
    let duplicated = sorted
        .windows(2)
        .find(|pair| {
            pair.first()
                .zip(pair.get(1))
                .is_some_and(|(left, right)| left == right)
        })
        .and_then(|pair| pair.first().cloned());
    if let Some(dup) = duplicated {
        return Err(CyberError::InvalidArgument(format!(
            "replace_all lists {dup} twice: every PNAMES name must be unique"
        )));
    }
    let mut removed = Vec::new();
    let mut added = Vec::new();
    for entry in current {
        if position_of(&names, entry).is_none() {
            removed.push(entry.clone());
        }
    }
    for name in &names {
        if position_of(current, name).is_none() {
            added.push(name.clone());
        }
    }
    Ok(Plan {
        mapping: name_mapping(current, &names)?,
        names,
        added,
        removed,
        ..Plan::default()
    })
}

/// The name-based `old -> new` mapping of two lists, `None` when no index
/// moves.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] when a surviving name lands past the
/// `u16` range a patch reference can store.
fn name_mapping(current: &[String], names: &[String]) -> Result<Option<PatchMapping>, CyberError> {
    let mut mapping = Vec::with_capacity(current.len());
    let mut identity = true;
    for (old, entry) in current.iter().enumerate() {
        let new = position_of(names, entry).map(usize_to_u16).transpose()?;
        let old_field = u16::try_from(old).ok();
        if new != old_field {
            identity = false;
        }
        mapping.push(new);
    }
    Ok((!identity).then_some(mapping))
}

/// The `u16` form of a new index position.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] above `u16::MAX`.
fn usize_to_u16(value: usize) -> Result<u16, CyberError> {
    u16::try_from(value).map_err(|error| {
        CyberError::LimitExceeded(format!(
            "the new PNAMES puts an entry at index {value}, past the u16 range textures store: \
             {error}"
        ))
    })
}

/// Refuses a plan whose mapping drops texture patch references, unless
/// `force` was given.
///
/// # Errors
///
/// [`CyberError::Conflict`] listing the affected textures.
fn check_dropped(
    mapping: &PatchMapping,
    force: bool,
    textures: &[(&TextureLump, &'static str)],
) -> Result<(), CyberError> {
    if force {
        return Ok(());
    }
    let offenders: Vec<String> = textures
        .iter()
        .flat_map(|&(lump, label)| {
            dropped_references(lump, mapping)
                .into_iter()
                .map(move |name| format!("{name} ({label})"))
        })
        .collect();
    if offenders.is_empty() {
        return Ok(());
    }
    Err(CyberError::Conflict(format!(
        "textures still refer to PNAMES names this edit removes: {}; pass force=true to drop \
         those patch references",
        offenders.join(", ")
    )))
}

/// Writes `data` as the working archive's global lump `name`: over the
/// last working copy in place, or appended as a new override. Shared with
/// the palette tools, which copy-on-write `PLAYPAL` and `COLORMAP` the
/// same way.
pub(crate) fn put_global(wad: &mut Wad, name: LumpName, data: Vec<u8>) -> Result<(), CyberError> {
    if let Some(index) = wad.find_last(name) {
        let Some(lump) = wad.lumps.get_mut(index) else {
            return Err(CyberError::Internal(format!(
                "no lump at index {index} of the working archive: {name} was expected there"
            )));
        };
        lump.data = Arc::from(data);
        return Ok(());
    }
    wad.lumps.push(Lump::new(name, data));
    Ok(())
}

/// The re-encoded texture lumps of one edit.
#[derive(Debug, Default)]
struct PlannedTextures {
    /// New `TEXTURE1` bytes, when it changed.
    texture1: Option<Vec<u8>>,
    /// New `TEXTURE2` bytes, when it changed.
    texture2: Option<Vec<u8>>,
}

/// Re-encodes the set's texture lumps through `mapping`, counting the
/// changed textures.
///
/// # Errors
///
/// [`crate::wad::textures::write_texture_lump`]'s errors (a parsed lump
/// always re-encodes, so these are [`CyberError::Internal`]-class only).
fn rewrite_textures(
    set: &TextureSet,
    mapping: &PatchMapping,
) -> Result<(PlannedTextures, usize), CyberError> {
    let mut out = PlannedTextures::default();
    let mut changed = 0_usize;
    for (entry, first) in [
        (set.texture1.as_ref(), true),
        (set.texture2.as_ref(), false),
    ] {
        let Some(pair) = entry else {
            continue;
        };
        let mut edited = pair.0.clone();
        let count = remap_patches(&mut edited, mapping);
        if count == 0 {
            continue;
        }
        changed = changed.saturating_add(count);
        let bytes = write_texture_lump(&edited)?;
        if first {
            out.texture1 = Some(bytes);
        } else {
            out.texture2 = Some(bytes);
        }
    }
    Ok((out, changed))
}

/// `edit_pnames`' work: plan the change, then commit it as one edit.
///
/// A plan that changed nothing (all names skipped, no texture rewritten)
/// records no history and reports the counters as they stand.
///
/// # Errors
///
/// [`plan_edit`]'s errors, [`TextureSet::load`]'s [`CyberError::Format`],
/// the codecs' [`CyberError::InvalidArgument`] and
/// [`CyberError::LimitExceeded`].
fn edit_pnames_run(
    session: &mut Session,
    operation: Operation<'_>,
    force: bool,
) -> Result<EditReport, CyberError> {
    let set = TextureSet::load(session)?;
    let textures = set.textures();
    let plan = plan_edit(&set.pnames, operation, force, &textures)?;
    let pnames_bytes = write_pnames(&plan.names)?;
    let (rewritten, changed) = match plan.mapping.as_ref() {
        Some(mapping) => rewrite_textures(&set, mapping)?,
        None => (PlannedTextures::default(), 0_usize),
    };
    let unchanged =
        plan.names == set.pnames && rewritten.texture1.is_none() && rewritten.texture2.is_none();
    if unchanged {
        return Ok(report_of(&plan, changed));
    }
    let label = format!("edit_pnames {}", operation.name());
    session.edit(&label, |wad| {
        put_global(wad, LumpName::from_raw(PNAMES_RAW), pnames_bytes.clone())?;
        if let Some(bytes) = rewritten.texture1.as_ref() {
            put_global(wad, LumpName::from_raw(TEXTURE1_RAW), bytes.clone())?;
        }
        if let Some(bytes) = rewritten.texture2.as_ref() {
            put_global(wad, LumpName::from_raw(TEXTURE2_RAW), bytes.clone())?;
        }
        Ok(())
    })?;
    Ok(report_of(&plan, changed))
}

/// The report for a finished plan.
fn report_of(plan: &Plan, changed: usize) -> EditReport {
    EditReport {
        pnames: plan.names.len(),
        added: plan.added.clone(),
        skipped: plan.skipped.clone(),
        removed: plan.removed.clone(),
        renamed: plan.renamed.clone(),
        textures_changed: changed,
    }
}

/// The single operation the arguments carry, refusing none and many.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] unless exactly one of `add`, `remove`,
/// `rename` and `replace_all` is present.
fn pick_operation(args: &EditPnamesParams) -> Result<Operation<'_>, CyberError> {
    let mut given = [
        args.add.as_deref().map(Operation::Add),
        args.remove.as_deref().map(Operation::Remove),
        args.rename.as_deref().map(Operation::Rename),
        args.replace_all.as_deref().map(Operation::ReplaceAll),
    ]
    .into_iter()
    .flatten();
    let Some(operation) = given.next() else {
        return Err(no_operation());
    };
    if given.next().is_some() {
        return Err(no_operation());
    }
    Ok(operation)
}

/// The refusal naming the accepted operations.
fn no_operation() -> CyberError {
    CyberError::InvalidArgument(
        "edit_pnames takes exactly one operation: pass one of add, remove, rename or \
         replace_all"
            .to_owned(),
    )
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = textures_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Lists the composite texture definitions of the session.
    #[tool(
        description = "Lists the composite textures defined by the merged view's `TEXTURE1` and `TEXTURE2` lumps, in lump order (all TEXTURE1 entries first, then TEXTURE2), each row {name, width, height, patches, lump, source}: the mappatch count and the archive the defining lump came from (list_lumps' source shape). A session without either lump answers {total: 0, textures: []}, not an error; a lump that decodes as neither the Doom nor the Strife texture layout fails with format_error. Optional pattern is a case-insensitive texture-name glob (* and ?); paging is offset (default 0) and limit (default 200, max 2000) over the matched list. Returns {total, textures} where total counts pattern matches before paging. Textures are built from patch lumps named by their PNAMES index: see list_pnames and edit_pnames."
    )]
    fn list_textures(
        &self,
        Parameters(args): Parameters<ListTexturesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let page = match parse_page(args.offset, args.limit) {
            Ok(page) => page,
            Err(error) => return fail(&error),
        };
        let report = self
            .state()
            .with_session(|session| list_textures_run(session, &page, args.pattern.as_deref()));
        match report {
            Ok(success) => ok(&success),
            Err(error) => fail(&error),
        }
    }

    /// Lists the merged `PNAMES` patch-name index.
    #[tool(
        description = "Lists the merged view's `PNAMES` entries in index order: {index, name, lump_found}. The index is the number TEXTURE1/TEXTURE2 patch references carry; lump_found says whether a lump with that name exists in the merged view outside the flats and sprites namespaces (a missing lump renders as nothing wherever a texture uses it). No PNAMES lump answers {total: 0, pnames: []}, not an error; a malformed one fails with format_error. Optional pattern is a case-insensitive name glob (* and ?); paging is offset (default 0) and limit (default 200, max 2000). Returns {total, pnames} where total counts pattern matches before paging. Change the list with edit_pnames; create the patch lumps themselves with the lump tools."
    )]
    fn list_pnames(
        &self,
        Parameters(args): Parameters<ListPnamesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let page = match parse_page(args.offset, args.limit) {
            Ok(page) => page,
            Err(error) => return fail(&error),
        };
        let report = self
            .state()
            .with_session(|session| list_pnames_run(session, &page, args.pattern.as_deref()));
        match report {
            Ok(success) => ok(&success),
            Err(error) => fail(&error),
        }
    }

    /// Edits `PNAMES` and renumbers every texture reference.
    #[tool(
        description = "Edits the merged view's `PNAMES` patch-name list and renumbers the patch references of TEXTURE1/TEXTURE2 so the indices stay truthful. Pass exactly one operation: add (append names; entries already present are skipped), remove (delete names), rename (a list of {from, to} pairs; indices stay put so textures are untouched) or replace_all (the new complete list; an empty list clears PNAMES; duplicate names in the list are invalid_argument). Every name must be 1 to 8 printable ASCII characters, stored uppercased; invalid names fail with invalid_argument. Removing or replacing shifts indices, so changed textures are rewritten into the SAME lump layout they were read in (Doom or Strife; column directory 0, stepdir 1, colormap 0). A name a texture still refers to fails with conflict listing the affected textures unless force=true, which drops those patch references. Only the working archive changes: PNAMES and each changed TEXTUREx lump is copied in as an override and written as ONE undoable edit labelled `edit_pnames <operation>` (save_wad writes the result); a no-op reports the counters and records nothing. A session without PNAMES can be created by add or replace_all; texture lumps without a PNAMES are listed unindexed. Returns {pnames: new entry count, added, skipped, removed, renamed, textures_changed}."
    )]
    fn edit_pnames(
        &self,
        Parameters(args): Parameters<EditPnamesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let operation = match pick_operation(&args) {
            Ok(operation) => operation,
            Err(error) => return fail(&error),
        };
        let report = self.state().with_session(|session| {
            edit_pnames_run(session, operation, args.force.unwrap_or(false))
        });
        match report {
            Ok(success) => ok(&success),
            Err(error) => fail(&error),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        EditPnamesParams, EditReport, Operation, Page, PnamesReport, RenamePair, TextureSet,
        WhichLump, edit_pnames_run, find_global, list_pnames_run, list_textures_run, name_mapping,
        parse_page, pick_operation, plan_edit, put_global,
    };
    use crate::error::CyberError;
    use crate::session::{Layer, LayerRole, Scope, Session, Source};
    use crate::wad::archive::{Lump, Wad, WadKind};
    use crate::wad::name::LumpName;
    use crate::wad::textures::{
        PatchRef, TextureDef, TextureLayout, TextureLump, write_pnames, write_texture_lump,
    };

    /// A valid parsed name for test data.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// A `PNAMES` lump carrying `names`.
    fn pnames_lump(names: &[&str]) -> Lump {
        let owned: Vec<String> = names.iter().map(|text| (*text).to_owned()).collect();
        Lump::new(
            name("PNAMES"),
            write_pnames(&owned).expect("test names are valid"),
        )
    }

    /// One texture definition referring to `patches` at the origin zero.
    fn def(text: &str, width: u16, height: u16, patches: &[u16]) -> TextureDef {
        TextureDef {
            name: text.to_owned(),
            flags_raw: 0,
            width,
            height,
            patches: patches
                .iter()
                .map(|&patch| PatchRef { patch, x: 0, y: 0 })
                .collect(),
        }
    }

    /// A `TEXTURE1` lump: WALL1 (three refs) and STEP1 (one ref).
    fn texture1_lump() -> Lump {
        let lump = TextureLump {
            layout: TextureLayout::Doom,
            textures: vec![
                def("WALL1", 64, 128, &[0, 0, 1]),
                def("STEP1", 64, 32, &[1]),
            ],
        };
        Lump::new(
            name("TEXTURE1"),
            write_texture_lump(&lump).expect("serializable"),
        )
    }

    /// A session on one `IWAD` layer holding the fixture texture set.
    fn session() -> Session {
        let mut session = Session::default();
        session.set_iwad(Layer::new(
            PathBuf::from("base.wad"),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps: vec![
                    pnames_lump(&["WALLP1", "STEPP1"]),
                    texture1_lump(),
                    Lump::new(name("WALLP1"), vec![b'P']),
                ],
            },
        ));
        session
    }

    /// The set of an explicit session.
    fn set_of(session: &Session) -> TextureSet {
        TextureSet::load(session).expect("the fixture lumps decode")
    }

    /// The error code of a [`CyberError`], for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    /// A page for the listing calls.
    fn page(offset: usize, limit: usize) -> Page {
        Page { offset, limit }
    }

    /// Run one operation end to end against the fixture session.
    fn run(operation: Operation<'_>, force: bool) -> Result<EditReport, CyberError> {
        let mut session = session();
        edit_pnames_run(&mut session, operation, force)
    }

    #[test]
    fn the_load_decodes_the_winning_lumps_and_finds_by_name() {
        let loaded = set_of(&session());
        assert_eq!(
            loaded.pnames,
            vec!["WALLP1".to_owned(), "STEPP1".to_owned()],
            "PNAMES decoded"
        );
        let (wall, which) = loaded.find("wall1").expect("case-insensitive hit");
        assert_eq!(wall.width, 64, "the fixture width");
        assert_eq!(which, WhichLump::Texture1, "came from TEXTURE1");
        assert!(loaded.find("NOPE").is_none(), "an unknown name misses");
    }

    #[test]
    fn texture2_entries_override_texture1_names_in_find() {
        let override_lump = TextureLump {
            layout: TextureLayout::Doom,
            textures: vec![def("WALL1", 8, 8, &[0])],
        };
        let mut session = session();
        session
            .edit("add TEXTURE2", |wad| {
                let bytes = write_texture_lump(&override_lump).expect("serializable");
                wad.lumps.push(Lump::new(name("TEXTURE2"), bytes));
                Ok(())
            })
            .expect("plain append");
        let loaded = set_of(&session);
        let (wall, which) = loaded.find("WALL1").expect("found");
        assert_eq!(which, WhichLump::Texture2, "TEXTURE2 wins");
        assert_eq!(wall.height, 8, "and it is the override definition");
    }

    #[test]
    fn the_listing_rows_carry_lump_and_source() {
        let rows = set_of(&session()).texture_rows();
        assert_eq!(rows.len(), 2, "both textures");
        assert_eq!(rows[0].lump, "TEXTURE1", "TEXTURE1 rows first");
        assert_eq!(rows[1].patches, 1, "the mappatch count");
        assert_eq!(rows[0].source, Source::Layer(0), "read from the layer");
    }

    #[test]
    fn find_global_returns_the_last_copy_of_the_merged_view() {
        let mut session = session();
        session
            .edit("override PNAMES", |wad| {
                wad.lumps.push(pnames_lump(&["ONLY"]));
                Ok(())
            })
            .expect("plain append");
        let found = find_global(&session, super::PNAMES_RAW).expect("found");
        assert_eq!(found.source, Source::Working, "the working copy wins");
        let loaded = set_of(&session);
        assert_eq!(loaded.pnames, vec!["ONLY".to_owned()], "loaded the winner");
    }

    #[test]
    fn pages_default_and_reject_out_of_range_limits() {
        let fresh = parse_page(None, None).expect("defaults");
        assert_eq!((fresh.offset, fresh.limit), (0, 200), "documented defaults");
        assert_eq!(
            parse_page(Some(3), Some(2000)).expect("max").limit,
            2000,
            "the cap"
        );
        for bad in [0_usize, 2001] {
            let error = parse_page(None, Some(bad)).expect_err("out of range");
            assert_eq!(code(&error), "invalid_argument", "limit {bad} code");
            assert!(
                error.to_string().contains("between 1 and 2000"),
                "the message states the range: {error}"
            );
        }
    }

    #[test]
    fn listing_pages_filters_and_counts_the_total() {
        let session = session();
        let all = list_textures_run(&session, &page(0, 10), None)
            .expect("runs")
            .textures;
        assert_eq!(all.len(), 2, "both rows");
        let filtered = list_textures_run(&session, &page(0, 10), Some("wall*")).expect("runs");
        assert_eq!(filtered.total, 1, "the pattern counts");
        assert_eq!(filtered.textures.len(), 1, "one row");
        let report = list_textures_run(&session, &page(1, 1), None).expect("runs");
        assert_eq!(
            (report.total, report.textures.len()),
            (2, 1),
            "the second page"
        );
        assert_eq!(
            report.textures.first().expect("one row").name,
            "STEP1",
            "in lump order"
        );
        let past = list_textures_run(&session, &page(9, 1), None).expect("runs");
        assert!(
            past.textures.is_empty() && past.total == 2,
            "past the end is empty"
        );
        let names: PnamesReport = list_pnames_run(&session, &page(0, 10), None).expect("runs");
        assert_eq!(names.total, 2, "both entries");
        assert!(
            names
                .pnames
                .iter()
                .any(|row| row.name == "WALLP1" && row.index == 0 && row.lump_found),
            "WALLP1 has a lump outside the flats and sprites regions"
        );
        assert!(
            names
                .pnames
                .iter()
                .any(|row| row.name == "STEPP1" && !row.lump_found),
            "STEPP1 has no lump in this fixture"
        );
        let one = list_pnames_run(&session, &page(0, 10), Some("s*")).expect("runs");
        assert_eq!(one.total, 1, "the pattern counts");
        assert_eq!(
            one.pnames.first().expect("one row").name,
            "STEPP1",
            "glob match"
        );
    }

    #[test]
    fn a_missing_pnames_lump_leaves_the_index_unbounded() {
        let mut bare = Session::default();
        bare.edit("add TEXTURE1 only", |wad| {
            wad.lumps.push(texture1_lump());
            Ok(())
        })
        .expect("plain append");
        let loaded = set_of(&bare);
        assert!(loaded.pnames.is_empty(), "no names");
        assert_eq!(loaded.texture_rows().len(), 2, "the lump still decodes");
        let report = list_pnames_run(&bare, &page(0, 10), None).expect("runs");
        assert_eq!(report.total, 0, "an empty list is not an error");
    }

    #[test]
    fn a_malformed_pnames_lump_surfaces_as_a_format_error() {
        let mut session = session();
        session
            .edit("junk PNAMES", |wad| {
                wad.lumps.push(Lump::new(name("PNAMES"), vec![0_u8; 3]));
                Ok(())
            })
            .expect("plain append");
        let error = TextureSet::load(&session).expect_err("the count claims more names than fit");
        assert_eq!(code(&error), "format_error", "malformed PNAMES code");
    }

    #[test]
    fn the_add_edit_lands_in_the_working_archive_and_undoes_as_one_step() {
        let mut session = session();
        let iwad_before = session.layers()[0].wad.lumps.len();
        let add = ["NEWP1".to_owned(), "WALLP1".to_owned()];
        let report = edit_pnames_run(&mut session, Operation::Add(&add), false).expect("runs");
        assert_eq!(report.pnames, 3, "one new entry");
        assert_eq!(report.added, vec!["NEWP1".to_owned()], "the new name");
        assert_eq!(report.skipped, vec!["WALLP1".to_owned()], "already present");
        assert_eq!(report.textures_changed, 0, "appends never shift indices");
        let working: Vec<String> = session
            .working()
            .lumps
            .iter()
            .map(|lump| lump.name.to_string())
            .collect();
        assert_eq!(
            working,
            vec!["PNAMES".to_owned()],
            "the override was appended"
        );
        assert_eq!(
            session.undo_labels(),
            vec!["edit_pnames add"],
            "one undoable edit"
        );
        assert!(session.is_dirty(), "and unsaved work");
        assert_eq!(
            set_of(&session).pnames,
            vec!["WALLP1".to_owned(), "STEPP1".to_owned(), "NEWP1".to_owned()],
            "the working PNAMES won the merge"
        );
        session.undo().expect("undo the whole edit");
        assert_eq!(set_of(&session).pnames.len(), 2, "back to the IWAD list");
        assert!(
            session.working().lumps.is_empty(),
            "and no working copy at all"
        );
        assert_eq!(
            session.layers()[0].wad.lumps.len(),
            iwad_before,
            "the IWAD layer never changed"
        );
    }

    #[test]
    fn an_add_of_only_present_names_changes_nothing_and_records_no_history() {
        let mut session = session();
        let add = ["wallp1".to_owned()];
        let report = edit_pnames_run(&mut session, Operation::Add(&add), false).expect("runs");
        assert_eq!(report.pnames, 2, "the count stands");
        assert_eq!(
            report.skipped,
            vec!["WALLP1".to_owned()],
            "reported skipped"
        );
        assert!(session.undo_labels().is_empty(), "no edit recorded");
        assert!(!session.is_dirty(), "the session stayed clean");
    }

    #[test]
    fn an_add_without_any_pnames_creates_the_lump() {
        let mut bare = Session::default();
        let add = ["FIRST".to_owned()];
        let report = edit_pnames_run(&mut bare, Operation::Add(&add), false).expect("creation");
        assert_eq!(report.pnames, 1, "the list started fresh");
        assert_eq!(
            bare.working().lumps.len(),
            1,
            "PNAMES landed in the working archive"
        );
        assert_eq!(
            bare.working().lumps.first().expect("one lump").name,
            name("PNAMES"),
            "named PNAMES"
        );
    }

    #[test]
    fn edit_pnames_remove_refuses_referenced_names_until_forced() {
        let remove = ["WALLP1".to_owned()];
        let error = run(Operation::Remove(&remove), false).expect_err("WALL1 refers to it");
        assert_eq!(code(&error), "conflict", "conflict code");
        let message = error.to_string();
        assert!(
            message.contains("WALL1 (TEXTURE1)") && message.contains("force=true"),
            "the message lists the affected texture and the escape: {message}"
        );
        assert!(
            !message.contains("STEP1"),
            "STEP1 only renumbers, it loses no reference: {message}"
        );
    }

    #[test]
    fn forced_remove_prunes_renumbers_and_writes_one_edit() {
        let mut session = session();
        let remove = ["WALLP1".to_owned()];
        let report = edit_pnames_run(&mut session, Operation::Remove(&remove), true)
            .expect("the force runs");
        assert_eq!(report.pnames, 1, "only STEPP1 is left");
        assert_eq!(
            report.removed,
            vec!["WALLP1".to_owned()],
            "reported removed"
        );
        assert_eq!(
            report.textures_changed, 2,
            "both textures shifted or pruned"
        );
        assert_eq!(
            session.undo_labels(),
            vec!["edit_pnames remove"],
            "one labelled edit for PNAMES and TEXTURE1 together"
        );
        let working: Vec<String> = session
            .working()
            .lumps
            .iter()
            .map(|lump| lump.name.to_string())
            .collect();
        assert_eq!(working, vec!["PNAMES", "TEXTURE1"], "both overrides exist");
        let loaded = set_of(&session);
        assert_eq!(
            loaded.pnames,
            vec!["STEPP1".to_owned()],
            "the shortened list"
        );
        let (wall, _) = loaded.find("WALL1").expect("still there");
        assert_eq!(wall.patches.len(), 1, "WALL1 kept only its renumbered ref");
        assert_eq!(
            wall.patches.first().expect("one ref").patch,
            0,
            "index 1 became 0"
        );
    }

    #[test]
    fn a_remove_of_absent_names_skips_without_writing() {
        let mut session = session();
        let remove = ["NOPE".to_owned()];
        let report = edit_pnames_run(&mut session, Operation::Remove(&remove), false)
            .expect("nothing to do");
        assert_eq!(report.skipped, vec!["NOPE".to_owned()], "reported skipped");
        assert_eq!(report.pnames, 2, "the count stands");
        assert!(session.undo_labels().is_empty(), "a no-op records nothing");
    }

    #[test]
    fn edit_pnames_rename_keeps_indices_and_textures() {
        let mut session = session();
        let pairs = vec![RenamePair {
            from: "wallp1".to_owned(),
            to: "WALLPX".to_owned(),
        }];
        let report = edit_pnames_run(&mut session, Operation::Rename(&pairs), false)
            .expect("the rename runs");
        assert_eq!(report.pnames, 2, "the count stands");
        assert_eq!(
            report.renamed,
            vec![super::RenameRow {
                from: "WALLP1".to_owned(),
                to: "WALLPX".to_owned()
            }],
            "the uppercased pair is reported"
        );
        assert_eq!(report.textures_changed, 0, "indices did not move");
        let loaded = set_of(&session);
        assert_eq!(loaded.pnames[0], "WALLPX", "renamed in place");
        assert_eq!(
            loaded.texture_rows()[0].patches,
            3,
            "WALL1 still has its refs"
        );
        let working: Vec<String> = session
            .working()
            .lumps
            .iter()
            .map(|lump| lump.name.to_string())
            .collect();
        assert_eq!(working, vec!["PNAMES"], "only PNAMES was copied in");
    }

    #[test]
    fn rename_reports_missing_sources_and_taken_targets() {
        let missing = vec![RenamePair {
            from: "NOPE".to_owned(),
            to: "WALLPX".to_owned(),
        }];
        let missing_error = run(Operation::Rename(&missing), false).expect_err("no such entry");
        assert_eq!(code(&missing_error), "not_found", "missing from code");
        let taken = vec![RenamePair {
            from: "WALLP1".to_owned(),
            to: "STEPP1".to_owned(),
        }];
        let taken_error = run(Operation::Rename(&taken), false).expect_err("target is taken");
        assert_eq!(code(&taken_error), "conflict", "taken to code");
        let bad = vec![RenamePair {
            from: "WALLP1".to_owned(),
            to: "bad name".to_owned(),
        }];
        let bad_error = run(Operation::Rename(&bad), false).expect_err("bad name");
        assert_eq!(code(&bad_error), "invalid_argument", "bad name code");
        let empty: Vec<RenamePair> = Vec::new();
        let empty_error = run(Operation::Rename(&empty), false).expect_err("no pairs");
        assert_eq!(code(&empty_error), "invalid_argument", "empty rename code");
    }

    #[test]
    fn edit_pnames_replace_all_reindexes_by_name() {
        let names = ["STEPP1".to_owned(), "WALLPX".to_owned()];
        let error = run(Operation::ReplaceAll(&names), false)
            .expect_err("WALLP1 vanishes while WALL1 still refers to it");
        assert_eq!(code(&error), "conflict", "conflict without force");
        let mut session = session();
        let report = edit_pnames_run(&mut session, Operation::ReplaceAll(&names), true)
            .expect("the force runs");
        assert_eq!(report.pnames, 2, "the new list");
        assert_eq!(
            report.removed,
            vec!["WALLP1".to_owned()],
            "the vanished entry"
        );
        assert_eq!(
            report.added,
            vec!["WALLPX".to_owned()],
            "the brought-in entry"
        );
        assert_eq!(
            report.textures_changed, 2,
            "STEP1 moved 1 to 0, WALL1 pruned"
        );
        let loaded = set_of(&session);
        assert_eq!(
            loaded.pnames,
            vec!["STEPP1".to_owned(), "WALLPX".to_owned()],
            "the replacement stands"
        );
    }

    #[test]
    fn replace_all_validates_its_list_and_replaces_nothing_when_there_is_no_list() {
        let dupes = ["A".to_owned(), "a".to_owned()];
        let dupe_error = run(Operation::ReplaceAll(&dupes), false).expect_err("case-folded dupe");
        assert_eq!(code(&dupe_error), "invalid_argument", "duplicate code");
        assert!(
            dupe_error.to_string().contains("twice"),
            "the message names the problem: {dupe_error}"
        );
        let bad = ["TOO LONG".to_owned()];
        let bad_error = run(Operation::ReplaceAll(&bad), false).expect_err("bad name");
        assert_eq!(code(&bad_error), "invalid_argument", "bad name code");
        // A session without PNAMES but with textures: no old names vanish,
        // so the empty replacement just creates the lump.
        let mut bare = Session::default();
        bare.edit("add TEXTURE1", |wad| {
            wad.lumps.push(texture1_lump());
            Ok(())
        })
        .expect("plain append");
        let empty: Vec<String> = Vec::new();
        let report = edit_pnames_run(&mut bare, Operation::ReplaceAll(&empty), false)
            .expect("nothing was removed, so nothing conflicts");
        assert_eq!(report.pnames, 0, "the fresh list is empty");
        assert_eq!(
            report.textures_changed, 0,
            "the unbounded lump is left alone"
        );
    }

    #[test]
    fn an_empty_add_or_remove_list_is_invalid_argument() {
        let empty: Vec<String> = Vec::new();
        let add_error = run(Operation::Add(&empty), false).expect_err("empty add");
        assert_eq!(code(&add_error), "invalid_argument", "empty add code");
        let remove_error = run(Operation::Remove(&empty), false).expect_err("empty remove");
        assert_eq!(code(&remove_error), "invalid_argument", "empty remove code");
        assert!(
            add_error.to_string().contains("at least one"),
            "the message states the rule: {add_error}"
        );
    }

    #[test]
    fn put_global_overwrites_the_last_working_copy_in_place() {
        let mut wad = Wad {
            kind: WadKind::Pwad,
            lumps: vec![
                Lump::new(name("PNAMES"), vec![1_u8]),
                Lump::new(name("X"), vec![2_u8]),
                Lump::new(name("PNAMES"), vec![3_u8]),
            ],
        };
        put_global(&mut wad, name("PNAMES"), vec![9_u8]).expect("the overwrite runs");
        assert_eq!(wad.lumps.len(), 3, "no lump grew in");
        assert_eq!(
            wad.lumps.last().expect("the last PNAMES").data.to_vec(),
            vec![9_u8],
            "the last copy changed"
        );
        assert_eq!(
            wad.lumps.first().expect("the first PNAMES").data.to_vec(),
            vec![1_u8],
            "the earlier copy stands"
        );
    }

    #[test]
    fn put_global_appends_a_lump_the_archive_lacks() {
        let mut wad = Wad::default();
        put_global(&mut wad, name("TEXTURE1"), vec![7_u8]).expect("the append runs");
        assert_eq!(wad.lumps.len(), 1, "one lump arrived");
        assert_eq!(
            wad.lumps.first().expect("one lump").name,
            name("TEXTURE1"),
            "its name"
        );
    }

    #[test]
    fn the_operation_picker_accepts_exactly_one_field() {
        let args = EditPnamesParams {
            add: Some(vec!["A".to_owned()]),
            remove: None,
            rename: None,
            replace_all: None,
            force: None,
        };
        assert!(
            matches!(pick_operation(&args), Ok(Operation::Add(_))),
            "add is picked"
        );
        let two = EditPnamesParams {
            remove: Some(vec!["A".to_owned()]),
            ..args
        };
        let two_error = pick_operation(&two).expect_err("two operations");
        assert_eq!(code(&two_error), "invalid_argument", "two-op code");
        assert!(
            two_error.to_string().contains("exactly one"),
            "the message states the rule: {two_error}"
        );
        let none = EditPnamesParams {
            add: None,
            remove: None,
            rename: None,
            replace_all: None,
            force: None,
        };
        let none_error = pick_operation(&none).expect_err("no operation");
        assert_eq!(code(&none_error), "invalid_argument", "zero-op code");
        let renamed = EditPnamesParams {
            rename: Some(Vec::new()),
            ..none
        };
        assert!(
            matches!(pick_operation(&renamed), Ok(Operation::Rename(_))),
            "rename is picked"
        );
    }

    #[test]
    fn the_name_mapping_tracks_moves_vanishing_and_standing_still() {
        let current = ["A".to_owned(), "B".to_owned(), "C".to_owned()];
        let names = ["C".to_owned(), "A".to_owned()];
        let mapping = name_mapping(&current, &names)
            .expect("small lists")
            .expect("names moved");
        assert_eq!(
            mapping,
            vec![Some(1_u16), None, Some(0_u16)],
            "B vanished, A moved to 1, C to 0"
        );
        assert!(
            name_mapping(&current, &current)
                .expect("small lists")
                .is_none(),
            "an unchanged list needs no mapping"
        );
        let appended = [
            "A".to_owned(),
            "B".to_owned(),
            "C".to_owned(),
            "D".to_owned(),
        ];
        assert!(
            name_mapping(&current, &appended)
                .expect("small lists")
                .is_none(),
            "a pure append keeps every old index"
        );
    }

    #[test]
    fn a_plan_without_a_mapping_skips_the_conflict_scan() {
        let current = ["A".to_owned(), "B".to_owned()];
        let list = ["C".to_owned()];
        let plan = plan_edit(&current, Operation::Add(&list), false, &[]).expect("a trivial plan");
        assert!(plan.mapping.is_none(), "an append never renumbers");
        assert_eq!(
            plan.names,
            vec!["A".to_owned(), "B".to_owned(), "C".to_owned()],
            "the list grew"
        );
        assert_eq!(plan.added, vec!["C".to_owned()], "one addition");
    }

    #[test]
    fn scope_rows_stay_visible_through_the_set() {
        let rows =
            crate::tools::lumps::rows::build_rows(&session(), Scope::Merged).expect("merged rows");
        assert!(
            rows.iter().any(|row| row.name == name("WALLP1")),
            "the fixture lump is visible"
        );
    }
}
