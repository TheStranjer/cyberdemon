//! The animation and switch tables the content sweeps keep frames for.
//!
//! A vanilla engine does not store an animation as a list of frames: its
//! table names the range's two endpoints and the engine walks its own
//! order — the directory for flats, the texture list for walls — from the
//! first name to the last. So a map that uses `NUKAGE1` depends on
//! `NUKAGE2` and `NUKAGE3` sitting between it and `NUKAGE3`: dropping the
//! other frames leaves an animation half-deleted, which is data loss and
//! not cleanup. `Boom` moved both tables into the optional `ANIMATED`
//! (ranges) and `SWITCHES` (pairs: a `SW1…` texture and the `SW2…` it
//! turns into when used) lumps, which is why a `PWAD`'s own custom ranges
//! have to be read as well as the built-in ones.
//!
//! [`Animations::read`] folds the built-in `Doom` rows with the merged
//! view's two lumps; [`Animations::expand`] grows one sweep's used-name
//! set with every frame a used name needs, resolved over that sweep's own
//! name order, and keeps outright every name a merged `ANIMATED` lump
//! references. A numbered range is read twice over: as the run between its
//! two endpoints, and as the family of names that share its stem and count
//! between its two frame numbers, so a space holding one endpoint and not
//! the other still keeps the run it does hold. Nothing here decodes a map
//! or a texture lump. Neither parser fails: both lumps are optional engine
//! data, so an unreadable record is skipped rather than guessed at, and a
//! skipped record can only ever cost a keep, never drop a name the sweep's
//! own rule proved in use.

use alloc::collections::BTreeSet;

use crate::wad::lumps;
use crate::wad::optimize::Context;
use crate::wad::optimize::redundant::last_global;

/// Bytes of one `ANIMATED` record: the range's last frame's name, its
/// first frame's name, the four-byte speed and the one-byte kind.
const ANIMATED_RECORD: usize = 23;
/// Bytes of one `ANIMATED` name field: eight for the name, one for its
/// `NUL`, which is why the field is wider than a directory name.
const ANIMATED_NAME: usize = 9;
/// The byte that ends the `ANIMATED` list: `Boom` writes it as the first
/// byte of the terminating record and as a row's kind.
const ANIMATED_LAST: u8 = 0xFF;
/// Bytes of one `SWITCHES` record: the two-byte episode and two eight-byte
/// texture names.
const SWITCHES_RECORD: usize = 18;
/// Bytes of one `SWITCHES` name field, exactly a directory name's width.
const SWITCHES_NAME: usize = 8;
/// Longest name either lump can hold.
const NAME_LIMIT: usize = 8;
/// Lowest byte a name may hold: `ASCII` `!`.
const MIN_NAME_BYTE: u8 = 0x21;
/// Highest byte a name may hold: `ASCII` `~`.
const MAX_NAME_BYTE: u8 = 0x7E;
/// Prefix of a switch texture's unused state.
const SWITCH_OFF: &str = "SW1";
/// Prefix of a switch texture's used state.
const SWITCH_ON: &str = "SW2";

/// How a group's two endpoint names sit in the engine's own order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    /// An animation: every frame from the first name to the last belongs
    /// to it, so one used frame keeps the whole run.
    Range,
    /// A switch: the two states of one wall texture, each keeping the
    /// other and nothing between them.
    Pair,
}

/// One animation group: the two names a table row carries and how they
/// relate.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Group {
    /// The range's named end first, then the other: the order the row was
    /// written in never matters, only where the two names land.
    first: String,
    /// The group's other endpoint name.
    second: String,
    /// Whether the names between the endpoints belong to the group.
    shape: Shape,
}

impl Group {
    /// The range running between `first` and `second`.
    fn range(first: &str, second: &str) -> Self {
        Self {
            first: first.to_owned(),
            second: second.to_owned(),
            shape: Shape::Range,
        }
    }

    /// The two switch states `first` and `second`.
    fn pair(first: &str, second: &str) -> Self {
        Self {
            first: first.to_owned(),
            second: second.to_owned(),
            shape: Shape::Pair,
        }
    }

    /// Both endpoint names.
    const fn names(&self) -> [&String; 2] {
        [&self.first, &self.second]
    }
}

/// The vanilla `Doom` animated flats, each row the range's two endpoints
/// as `Doom`'s own table lists them.
const VANILLA_FLATS: [(&str, &str); 9] = [
    ("NUKAGE1", "NUKAGE3"),
    ("FWATER1", "FWATER4"),
    ("SWATER1", "SWATER4"),
    ("LAVA1", "LAVA4"),
    ("BLOOD1", "BLOOD3"),
    ("RROCK05", "RROCK08"),
    ("SLIME01", "SLIME04"),
    ("SLIME05", "SLIME08"),
    ("SLIME09", "SLIME12"),
];

/// The vanilla `Doom` animated wall textures: the same shape as
/// [`VANILLA_FLATS`], the run of frames between two named endpoints, here
/// the run a patch's texture belongs to.
const VANILLA_TEXTURES: [(&str, &str); 13] = [
    ("BLODGR1", "BLODGR4"),
    ("SLADRIP1", "SLADRIP3"),
    ("BLODRIP1", "BLODRIP4"),
    ("FIREWALA", "FIREWALL"),
    ("GSTFONT1", "GSTFONT3"),
    ("FIRELAV3", "FIRELAVA"),
    ("FIREMAG1", "FIREMAG3"),
    ("FIREBLU1", "FIREBLU2"),
    ("ROCKRED1", "ROCKRED3"),
    ("BFALL1", "BFALL4"),
    ("SFALL1", "SFALL4"),
    ("WFALL1", "WFALL4"),
    ("DBRAIN1", "DBRAIN4"),
];

/// The vanilla `Doom` switch textures by stem: each row is the
/// `SW1<stem>` wall and the `SW2<stem>` texture it becomes when used, the
/// whole table of every episode.
const VANILLA_SWITCHES: [&str; 40] = [
    "BRCOM", "BRN1", "BRN2", "BRNGN", "BROWN", "COMM", "COMP", "DIRT", "EXIT", "GRAY", "GRAY1",
    "METAL", "PIPE", "SLAD", "STARG", "STON1", "STON2", "STONE", "STRTN", "BLUE", "CMT", "GARG",
    "GSTON", "HOT", "LION", "SATYR", "SKIN", "VINE", "WOOD", "PANEL", "ROCK", "MET2", "WDMET",
    "BRIK", "MOD1", "ZIM", "STON6", "TEK", "MARB", "SKULL",
];

/// Every built-in row: the flat ranges, then the texture ranges, then the
/// switch pairs, each table in its own order.
fn vanilla() -> Vec<Group> {
    let mut out = Vec::new();
    for (first, second) in VANILLA_FLATS {
        out.push(Group::range(first, second));
    }
    for (first, second) in VANILLA_TEXTURES {
        out.push(Group::range(first, second));
    }
    for stem in VANILLA_SWITCHES {
        out.push(Group::pair(
            &format!("{SWITCH_OFF}{stem}"),
            &format!("{SWITCH_ON}{stem}"),
        ));
    }
    out
}

/// The animation tables one [`super::plan`] call reads: the built-in
/// `Doom` rows plus the merged view's own two lumps.
pub(super) struct Animations {
    /// Every group the sweeps expand over.
    groups: Vec<Group>,
    /// Every name a merged `ANIMATED` lump names. Its author declared an
    /// animation for the name, so the sweeps keep it whether or not
    /// anything in the snapshot uses it yet.
    referenced: BTreeSet<String>,
}

impl Animations {
    /// The built-in tables folded with the merged `ANIMATED` and
    /// `SWITCHES` lumps. A lump that is absent contributes nothing, and so
    /// does one whose records cannot be read.
    pub(super) fn read(context: &Context<'_>) -> Self {
        let mut groups = vanilla();
        let mut referenced = BTreeSet::new();
        if let Some(lump) = last_global(context.merged, context.merged_info, lumps::ANIMATED) {
            let rows = animated(&lump.data);
            referenced.extend(rows.iter().flat_map(Group::names).cloned());
            groups.extend(rows);
        }
        if let Some(lump) = last_global(context.merged, context.merged_info, lumps::SWITCHES) {
            groups.extend(switches(&lump.data));
        }
        Self { groups, referenced }
    }

    /// Every name a sweep must keep: `used` grown with the frames and
    /// partners the tables need alongside them and with the names a merged
    /// `ANIMATED` lump references.
    ///
    /// `order` is that sweep's own name space in engine order: the merged
    /// flats for [`Operation::UnusedFlats`](super::Operation::UnusedFlats),
    /// the `PNAMES` list for the two patch sweeps. A range therefore only
    /// ever expands over names that really sit in the space the engine
    /// walks.
    pub(super) fn expand(&self, order: &[String], used: &BTreeSet<String>) -> BTreeSet<String> {
        let mut keeps = Keeps::new(&self.referenced, order, used);
        for group in &self.groups {
            keeps.keep_group(group, order);
        }
        keeps.kept
    }
}

/// One [`Animations::expand`] pass: the names a sweep ends up keeping, and
/// the fixed set that started the expansion.
struct Keeps {
    /// The names that start an expansion: what the sweep's own rule read
    /// as used, plus the declared names its space holds. Groups are
    /// triggered by these, never by what an earlier group added, so the
    /// result does not depend on table order.
    trigger: BTreeSet<String>,
    /// The names the sweep keeps: [`Keeps::trigger`] grown by the tables.
    kept: BTreeSet<String>,
}

impl Keeps {
    /// A pass whose used names are `used`, with every `ANIMATED` name
    /// `order` holds added to what starts the expansion.
    fn new(referenced: &BTreeSet<String>, order: &[String], used: &BTreeSet<String>) -> Self {
        let mut trigger = used.clone();
        trigger.extend(
            order
                .iter()
                .filter(|name| referenced.contains(*name))
                .cloned(),
        );
        Self {
            kept: trigger.clone(),
            trigger,
        }
    }

    /// Applies one group: a range both by the run between its endpoints and
    /// by the naming family its endpoints share, a pair by its two states
    /// alone.
    fn keep_group(&mut self, group: &Group, order: &[String]) {
        match group.shape {
            Shape::Range => {
                self.keep_range(group, order);
                self.keep_family(group, order);
            }
            Shape::Pair => self.keep_pair(group),
        }
    }

    /// Keeps the whole run between a range's endpoints when a starting name
    /// lies in it. A range whose endpoints do not both live in this space
    /// keeps nothing here.
    fn keep_range(&mut self, group: &Group, order: &[String]) {
        let (Some(first), Some(second)) = (place(order, &group.first), place(order, &group.second))
        else {
            return;
        };
        let (low, high) = if first <= second {
            (first, second)
        } else {
            (second, first)
        };
        if !touches(order, &self.trigger, low, high) {
            return;
        }
        self.kept
            .extend((low..=high).filter_map(|index| order.get(index)).cloned());
    }

    /// Keeps both states of a switch pair when a starting name is one of
    /// them. The two states are anywhere in the engine's order, so nothing
    /// between them is claimed.
    fn keep_pair(&mut self, group: &Group) {
        if group
            .names()
            .iter()
            .any(|name| self.trigger.contains(*name))
        {
            self.kept.extend(group.names().into_iter().cloned());
        }
    }

    /// Keeps a range's numbered family: every name of the space carrying
    /// the stem its endpoints share and a frame number between theirs. This
    /// is the fallback for the run the engine's order cannot answer, an
    /// endpoint the space does not hold or frames the author listed out of
    /// order, and it is where a range whose names carry no numbers adds
    /// nothing: `FIREWALA` to `FIREWALL` is only ever the directory's run.
    fn keep_family(&mut self, group: &Group, order: &[String]) {
        let Some(family) = Family::of(&group.first, &group.second) else {
            return;
        };
        let run: Vec<String> = order
            .iter()
            .filter(|name| family.holds(name))
            .cloned()
            .collect();
        if run.iter().any(|name| self.trigger.contains(name)) {
            self.kept.extend(run);
        }
    }
}

/// The numbered family a range's two endpoint names describe: the stem
/// every frame of the run shares and the two frame numbers the engine
/// counts between.
struct Family<'names> {
    /// The name every frame of the run starts with.
    stem: &'names str,
    /// The lower of the two endpoints' frame numbers.
    low: usize,
    /// The higher of the two.
    high: usize,
}

impl<'names> Family<'names> {
    /// The family `first` and `second` describe, `None` when either name
    /// carries no frame number to count between.
    fn of(first: &'names str, second: &str) -> Option<Self> {
        let stem = first.trim_end_matches(|letter: char| letter.is_ascii_digit());
        let (low, high) = (
            number(first.strip_prefix(stem)?)?,
            number(second.strip_prefix(stem)?)?,
        );
        Some(Self {
            stem,
            low: low.min(high),
            high: low.max(high),
        })
    }

    /// Whether `name` is one of the family's frames.
    fn holds(&self, name: &str) -> bool {
        name.strip_prefix(self.stem)
            .and_then(number)
            .is_some_and(|frame| self.low <= frame && frame <= self.high)
    }
}

/// The frame number a name's tail carries: whole `ASCII` digits, so a
/// letter-suffixed name is never read as one.
fn number(tail: &str) -> Option<usize> {
    if tail.is_empty() || !tail.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    tail.parse().ok()
}

/// The last position `name` holds in `order`, the way the engine's own name
/// lookup answers when a name appears more than once.
fn place(order: &[String], name: &str) -> Option<usize> {
    order.iter().rposition(|entry| entry == name)
}

/// Whether any starting name sits in the run `low..=high` of `order`.
fn touches(order: &[String], trigger: &BTreeSet<String>, low: usize, high: usize) -> bool {
    (low..=high)
        .filter_map(|index| order.get(index))
        .any(|name| trigger.contains(name))
}

/// The ranges of one `ANIMATED` lump, in record order. Each record is the
/// range's last frame's name, then its first frame's, then the speed and
/// the kind; the pair is symmetric here because the run between the two
/// names is what matters, whichever way the author wrote them.
fn animated(bytes: &[u8]) -> Vec<Group> {
    let mut out = Vec::new();
    for record in bytes.chunks_exact(ANIMATED_RECORD) {
        if ends_animated(record) {
            break;
        }
        let Some((last, rest)) = record.split_at_checked(ANIMATED_NAME) else {
            break;
        };
        let Some(second_bytes) = rest.get(..ANIMATED_NAME) else {
            break;
        };
        let Some(second) = field_name(second_bytes) else {
            continue;
        };
        let Some(first) = field_name(last) else {
            continue;
        };
        out.push(Group::range(&first, &second));
    }
    out
}

/// Whether `record` is the `ANIMATED` list's terminator: `Boom` writes
/// `255` as either its first byte or its kind.
fn ends_animated(record: &[u8]) -> bool {
    record.first() == Some(&ANIMATED_LAST) || record.last() == Some(&ANIMATED_LAST)
}

/// The switch pairs of one `SWITCHES` lump, in record order.
fn switches(bytes: &[u8]) -> Vec<Group> {
    let mut out = Vec::new();
    for record in bytes.chunks_exact(SWITCHES_RECORD) {
        let Some((episode, names)) = record.split_first_chunk::<2>() else {
            break;
        };
        if i16::from_le_bytes(*episode) == 0 {
            break;
        }
        let Some((off, rest)) = names.split_first_chunk::<SWITCHES_NAME>() else {
            break;
        };
        let Some(on) = rest.get(..SWITCHES_NAME) else {
            break;
        };
        let (Some(first), Some(second)) = (field_name(off), field_name(on)) else {
            continue;
        };
        out.push(Group::pair(&first, &second));
    }
    out
}

/// The name one lump field holds: the bytes up to its first `NUL`,
/// uppercased. `None` when the field is empty, longer than a directory
/// name, or holds a byte no name can — a record that fails here is simply
/// not a name pair and contributes no group.
fn field_name(bytes: &[u8]) -> Option<String> {
    let end = bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len());
    let text = bytes.get(..end)?;
    if text.is_empty() || text.len() > NAME_LIMIT {
        return None;
    }
    if !text
        .iter()
        .all(|&byte| (MIN_NAME_BYTE..=MAX_NAME_BYTE).contains(&byte))
    {
        return None;
    }
    Some(
        text.iter()
            .map(|&byte| char::from(byte.to_ascii_uppercase()))
            .collect(),
    )
}
