//! The per-kind field tables and the block plans one run drives.
//!
//! Every [`apply`](super::apply) edit runs on one [`KindPlan`]: the
//! element kind's field names with their [`FieldKind`]s and `add`ability,
//! the kind's flag table when the block carries a `flags` column, and the
//! typed readers and writers that touch that block's rows.
//! [`plan_for`] picks the plan: every block has one of its own, and the
//! linedef block follows the [`EditScope`](super::EditScope)'s
//! [`FlagFamily`], so the `Doom` table's `tag` and the `Hexen` one's
//! `arg0..arg4` never both exist.

use alloc::string::String;

use crate::wad::map::flags::{FlagFamily, FlagKind};
use crate::wad::map::model::Map;
use crate::wad::map::query::ElementKind;

use super::access::{
    line_flags, line_num, line_text, no_flags, sector_num, sector_text, side_num, side_text,
    thing_flags, thing_num, thing_text, vertex_num, vertex_text, write_line_flags, write_line_num,
    write_line_text, write_no_flags, write_sector_num, write_sector_text, write_side_num,
    write_side_text, write_thing_flags, write_thing_num, write_thing_text, write_vertex_num,
    write_vertex_text,
};
use super::{FieldKind, FieldSpec, NumRange, Stock};

/// Sector fields: the heights, light, flats, effect and tag.
const SECTOR_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "ceiling",
        addable: true,
        kind: FieldKind::Num(NumRange::Small),
    },
    FieldSpec {
        name: "ceiling_flat",
        addable: false,
        kind: FieldKind::Text(Stock::Flats),
    },
    FieldSpec {
        name: "floor",
        addable: true,
        kind: FieldKind::Num(NumRange::Small),
    },
    FieldSpec {
        name: "floor_flat",
        addable: false,
        kind: FieldKind::Text(Stock::Flats),
    },
    FieldSpec {
        name: "light",
        addable: true,
        kind: FieldKind::Num(NumRange::Light),
    },
    FieldSpec {
        name: "special",
        addable: false,
        kind: FieldKind::Num(NumRange::Unsigned),
    },
    FieldSpec {
        name: "tag",
        addable: true,
        kind: FieldKind::Num(NumRange::Unsigned),
    },
];

/// Sidedef fields: the three texture faces and the offsets.
const SIDEDEF_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "lower",
        addable: false,
        kind: FieldKind::Text(Stock::Walls),
    },
    FieldSpec {
        name: "middle",
        addable: false,
        kind: FieldKind::Text(Stock::Walls),
    },
    FieldSpec {
        name: "upper",
        addable: false,
        kind: FieldKind::Text(Stock::Walls),
    },
    FieldSpec {
        name: "x_offset",
        addable: true,
        kind: FieldKind::Num(NumRange::Small),
    },
    FieldSpec {
        name: "y_offset",
        addable: true,
        kind: FieldKind::Num(NumRange::Small),
    },
];

/// Vertex fields: the coordinates, `add` only.
const VERTEX_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "x",
        addable: true,
        kind: FieldKind::Num(NumRange::Small),
    },
    FieldSpec {
        name: "y",
        addable: true,
        kind: FieldKind::Num(NumRange::Small),
    },
];

/// `Doom`-family linedef fields: the action and the trigger tag. The
/// `Hexen` family stores its parameters in `LINEDEF_HEXEN_FIELDS` instead,
/// and no linedef number takes an `add` amount.
const LINEDEF_DOOM_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "special",
        addable: false,
        kind: FieldKind::Num(NumRange::LineSpecial),
    },
    FieldSpec {
        name: "tag",
        addable: false,
        kind: FieldKind::Num(NumRange::Unsigned),
    },
];

/// `Hexen`-family linedef fields: the action and the five script
/// arguments; `tag` belongs to the `Doom` family and is gated out here.
const LINEDEF_HEXEN_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "arg0",
        addable: false,
        kind: FieldKind::Num(NumRange::Byte),
    },
    FieldSpec {
        name: "arg1",
        addable: false,
        kind: FieldKind::Num(NumRange::Byte),
    },
    FieldSpec {
        name: "arg2",
        addable: false,
        kind: FieldKind::Num(NumRange::Byte),
    },
    FieldSpec {
        name: "arg3",
        addable: false,
        kind: FieldKind::Num(NumRange::Byte),
    },
    FieldSpec {
        name: "arg4",
        addable: false,
        kind: FieldKind::Num(NumRange::Byte),
    },
    FieldSpec {
        name: "special",
        addable: false,
        kind: FieldKind::Num(NumRange::LineSpecial),
    },
];

/// Thing fields: type, facing and position. The facing stays within a
/// circle in every format; the type is an integer doomednum here, names
/// resolve in the tool layer.
const THING_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "angle",
        addable: true,
        kind: FieldKind::Num(NumRange::Angle),
    },
    FieldSpec {
        name: "type",
        addable: false,
        kind: FieldKind::Num(NumRange::Unsigned),
    },
    FieldSpec {
        name: "x",
        addable: true,
        kind: FieldKind::Num(NumRange::Small),
    },
    FieldSpec {
        name: "y",
        addable: true,
        kind: FieldKind::Num(NumRange::Small),
    },
];

/// Reads one numeric field of the row at an index.
type NumReader = fn(&Map, usize, &str) -> Option<i32>;
/// Reads one text field of the row at an index.
type TextReader = fn(&Map, usize, &str) -> Option<String>;
/// Writes one planned integer onto the row at an index.
type NumWriter = fn(&mut Map, usize, &str, i32);
/// Writes one planned name onto the row at an index.
type TextWriter = fn(&mut Map, usize, &str, &str);
/// Reads the raw flag bits of the row at an index.
type FlagReader = fn(&Map, usize) -> Option<u32>;
/// Writes planned raw flag bits onto the row at an index.
type FlagWriter = fn(&mut Map, usize, u32);

/// The block-specific plumbing one [`apply`](super::apply) run drives.
pub(super) struct KindPlan {
    /// Singular element name for the messages ("sector").
    pub(super) thing: &'static str,
    /// The fields this kind edits.
    pub(super) fields: &'static [FieldSpec],
    /// The kind's flag table, `None` for the kinds without a `flags`
    /// column.
    pub(super) flag_kind: Option<FlagKind>,
    /// Reads a numeric field of the row at an index.
    pub(super) get_num: NumReader,
    /// Reads a text field of the row at an index.
    pub(super) get_text: TextReader,
    /// Writes one planned integer onto the row at its index.
    pub(super) write_num: NumWriter,
    /// Writes one planned name onto the row at its index.
    pub(super) write_text: TextWriter,
    /// Reads the row's flag bits.
    pub(super) get_flags: FlagReader,
    /// Writes the row's flag bits.
    pub(super) write_flags: FlagWriter,
}

/// The sector block's plan.
const SECTOR_PLAN: KindPlan = KindPlan {
    thing: "sector",
    fields: SECTOR_FIELDS,
    flag_kind: None,
    get_num: sector_num,
    get_text: sector_text,
    write_num: write_sector_num,
    write_text: write_sector_text,
    get_flags: no_flags,
    write_flags: write_no_flags,
};

/// The sidedef block's plan.
const SIDEDEF_PLAN: KindPlan = KindPlan {
    thing: "sidedef",
    fields: SIDEDEF_FIELDS,
    flag_kind: None,
    get_num: side_num,
    get_text: side_text,
    write_num: write_side_num,
    write_text: write_side_text,
    get_flags: no_flags,
    write_flags: write_no_flags,
};

/// The vertex block's plan.
const VERTEX_PLAN: KindPlan = KindPlan {
    thing: "vertex",
    fields: VERTEX_FIELDS,
    flag_kind: None,
    get_num: vertex_num,
    get_text: vertex_text,
    write_num: write_vertex_num,
    write_text: write_vertex_text,
    get_flags: no_flags,
    write_flags: write_no_flags,
};

/// The `Doom`-family linedef plan.
const LINEDEF_DOOM_PLAN: KindPlan = KindPlan {
    thing: "linedef",
    fields: LINEDEF_DOOM_FIELDS,
    flag_kind: Some(FlagKind::Linedef),
    get_num: line_num,
    get_text: line_text,
    write_num: write_line_num,
    write_text: write_line_text,
    get_flags: line_flags,
    write_flags: write_line_flags,
};

/// The `Hexen`-family linedef plan.
const LINEDEF_HEXEN_PLAN: KindPlan = KindPlan {
    thing: "linedef",
    fields: LINEDEF_HEXEN_FIELDS,
    flag_kind: Some(FlagKind::Linedef),
    get_num: line_num,
    get_text: line_text,
    write_num: write_line_num,
    write_text: write_line_text,
    get_flags: line_flags,
    write_flags: write_line_flags,
};

/// The thing plan, shared by both families (only the flag table differs,
/// and that read goes through the scope's family).
const THING_PLAN: KindPlan = KindPlan {
    thing: "thing",
    fields: THING_FIELDS,
    flag_kind: Some(FlagKind::Thing),
    get_num: thing_num,
    get_text: thing_text,
    write_num: write_thing_num,
    write_text: write_thing_text,
    get_flags: thing_flags,
    write_flags: write_thing_flags,
};

/// The block plan one [`apply`](super::apply) run drives: the linedef table follows the
/// scope's flag family, everything else is the block itself.
pub(super) const fn plan_for(kind: ElementKind, family: FlagFamily) -> &'static KindPlan {
    match kind {
        ElementKind::Sectors => &SECTOR_PLAN,
        ElementKind::Sidedefs => &SIDEDEF_PLAN,
        ElementKind::Vertices => &VERTEX_PLAN,
        ElementKind::Linedefs => match family {
            FlagFamily::Doom => &LINEDEF_DOOM_PLAN,
            FlagFamily::Hexen => &LINEDEF_HEXEN_PLAN,
        },
        ElementKind::Things => &THING_PLAN,
    }
}
