//! The per-kind field tables and the block plan one run drives.
//!
//! Every [`apply`](super::apply) edit runs on one [`KindPlan`]: the
//! element kind's [`Field`](super::fields::Field)s with their
//! [`FieldKind`]s and `add`ability, and the kind's flag table when the
//! block carries a `flags` column. The fields are the
//! [`fields`](super::fields) enums themselves, so an entry can only name
//! a column its block's accessors match; the reads and writes dispatch on
//! the field ([`access`](super::access)), not on the plan.
//! [`plan_for`] picks the plan: every block has one of its own, and the
//! linedef block follows the [`EditScope`](super::EditScope)'s
//! [`FlagFamily`], so the `Doom` table's `tag` and the `Hexen` one's
//! `arg0..arg4` never both exist.

use crate::wad::map::flags::{FlagFamily, FlagKind};
use crate::wad::map::query::ElementKind;

use super::fields::{Field, LinedefField, SectorField, SidedefField, ThingField, VertexField};
use super::{FieldKind, FieldSpec, NumRange, Stock};

/// The sector field table entry.
const fn sector_spec(field: SectorField, addable: bool, kind: FieldKind) -> FieldSpec {
    FieldSpec {
        field: Field::Sector(field),
        addable,
        kind,
    }
}

/// The sidedef field table entry.
const fn side_spec(field: SidedefField, addable: bool, kind: FieldKind) -> FieldSpec {
    FieldSpec {
        field: Field::Sidedef(field),
        addable,
        kind,
    }
}

/// The linedef field table entry.
const fn line_spec(field: LinedefField, addable: bool, kind: FieldKind) -> FieldSpec {
    FieldSpec {
        field: Field::Linedef(field),
        addable,
        kind,
    }
}

/// The thing field table entry.
const fn thing_spec(field: ThingField, addable: bool, kind: FieldKind) -> FieldSpec {
    FieldSpec {
        field: Field::Thing(field),
        addable,
        kind,
    }
}

/// Sector fields: the heights, light, flats, effect and tag.
const SECTOR_FIELDS: &[FieldSpec] = &[
    sector_spec(SectorField::Ceiling, true, FieldKind::Num(NumRange::Small)),
    sector_spec(
        SectorField::CeilingFlat,
        false,
        FieldKind::Text(Stock::Flats),
    ),
    sector_spec(SectorField::Floor, true, FieldKind::Num(NumRange::Small)),
    sector_spec(SectorField::FloorFlat, false, FieldKind::Text(Stock::Flats)),
    sector_spec(SectorField::Light, true, FieldKind::Num(NumRange::Light)),
    sector_spec(
        SectorField::Special,
        false,
        FieldKind::Num(NumRange::Unsigned),
    ),
    sector_spec(SectorField::Tag, true, FieldKind::Num(NumRange::Unsigned)),
];

/// Sidedef fields: the three texture faces and the offsets.
const SIDEDEF_FIELDS: &[FieldSpec] = &[
    side_spec(SidedefField::Lower, false, FieldKind::Text(Stock::Walls)),
    side_spec(SidedefField::Middle, false, FieldKind::Text(Stock::Walls)),
    side_spec(SidedefField::Upper, false, FieldKind::Text(Stock::Walls)),
    side_spec(SidedefField::XOffset, true, FieldKind::Num(NumRange::Small)),
    side_spec(SidedefField::YOffset, true, FieldKind::Num(NumRange::Small)),
];

/// Vertex fields: the coordinates, `add` only.
const VERTEX_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        field: Field::Vertex(VertexField::X),
        addable: true,
        kind: FieldKind::Num(NumRange::Small),
    },
    FieldSpec {
        field: Field::Vertex(VertexField::Y),
        addable: true,
        kind: FieldKind::Num(NumRange::Small),
    },
];

/// `Doom`-family linedef fields: the action and the trigger tag. The
/// `Hexen` family stores its parameters in `LINEDEF_HEXEN_FIELDS` instead,
/// and no linedef number takes an `add` amount.
const LINEDEF_DOOM_FIELDS: &[FieldSpec] = &[
    line_spec(
        LinedefField::Special,
        false,
        FieldKind::Num(NumRange::LineSpecial),
    ),
    line_spec(LinedefField::Tag, false, FieldKind::Num(NumRange::Unsigned)),
];

/// `Hexen`-family linedef fields: the action and the five script
/// arguments; `tag` belongs to the `Doom` family and is gated out here.
const LINEDEF_HEXEN_FIELDS: &[FieldSpec] = &[
    line_spec(LinedefField::Arg0, false, FieldKind::Num(NumRange::Byte)),
    line_spec(LinedefField::Arg1, false, FieldKind::Num(NumRange::Byte)),
    line_spec(LinedefField::Arg2, false, FieldKind::Num(NumRange::Byte)),
    line_spec(LinedefField::Arg3, false, FieldKind::Num(NumRange::Byte)),
    line_spec(LinedefField::Arg4, false, FieldKind::Num(NumRange::Byte)),
    line_spec(
        LinedefField::Special,
        false,
        FieldKind::Num(NumRange::LineSpecial),
    ),
];

/// Thing fields: type, facing and position. The facing stays within a
/// circle in every format; the type is an integer doomednum here, names
/// resolve in the tool layer.
const THING_FIELDS: &[FieldSpec] = &[
    thing_spec(ThingField::Angle, true, FieldKind::Num(NumRange::Angle)),
    thing_spec(ThingField::Type, false, FieldKind::Num(NumRange::Unsigned)),
    thing_spec(ThingField::X, true, FieldKind::Num(NumRange::Small)),
    thing_spec(ThingField::Y, true, FieldKind::Num(NumRange::Small)),
];

/// The block-specific plumbing one [`apply`](super::apply) run drives.
/// The field reads and writes are not here: they dispatch on the
/// [`Field`](super::fields::Field) every planned value is keyed by. The
/// element's own name in a message is [`ElementKind::singular`], read
/// off the [`Job`](super::job::Job)'s kind, never repeated here.
pub(super) struct KindPlan {
    /// The fields this kind edits.
    pub(super) fields: &'static [FieldSpec],
    /// The kind's flag table, `None` for the kinds without a `flags`
    /// column; when it names a kind, that is the block whose rows'
    /// `flags` the run reads and writes.
    pub(super) flag_kind: Option<FlagKind>,
}

/// The sector block's plan.
const SECTOR_PLAN: KindPlan = KindPlan {
    fields: SECTOR_FIELDS,
    flag_kind: None,
};

/// The sidedef block's plan.
const SIDEDEF_PLAN: KindPlan = KindPlan {
    fields: SIDEDEF_FIELDS,
    flag_kind: None,
};

/// The vertex block's plan.
const VERTEX_PLAN: KindPlan = KindPlan {
    fields: VERTEX_FIELDS,
    flag_kind: None,
};

/// The `Doom`-family linedef plan.
const LINEDEF_DOOM_PLAN: KindPlan = KindPlan {
    fields: LINEDEF_DOOM_FIELDS,
    flag_kind: Some(FlagKind::Linedef),
};

/// The `Hexen`-family linedef plan.
const LINEDEF_HEXEN_PLAN: KindPlan = KindPlan {
    fields: LINEDEF_HEXEN_FIELDS,
    flag_kind: Some(FlagKind::Linedef),
};

/// The thing plan, shared by both families (only the flag table differs,
/// and that read goes through the scope's family).
const THING_PLAN: KindPlan = KindPlan {
    fields: THING_FIELDS,
    flag_kind: Some(FlagKind::Thing),
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
