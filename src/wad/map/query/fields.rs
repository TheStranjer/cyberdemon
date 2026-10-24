//! The field tables: what each element kind can be filtered by, and the
//! comparable shape each of those fields reads as.
//!
//! [`FieldType`] is the classification the op and value checks and the
//! comparisons switch on; [`FieldSpec`] pairs one wire name with it. Every
//! kind carries its own table ([`base_fields`]) and the flag families add
//! their own rows through [`extra_fields`]: the linedef `tag` or `arg0..arg4`
//! and the thing `tid` and `special`. [`field_spec`] finds the row a filter
//! names and [`valid_fields`] joins the names an unknown-field error quotes.
//!
//! [`FValue`] is what reading one field of one element answers with: the
//! stored or derived value widened into the shape [`FieldType`] promises, a
//! name borrowed from the map when the model already owns it.

use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec::Vec;

use crate::wad::map::flags::{FlagFamily, FlagKind};

use super::ElementKind;

/// The comparable shape one field reads as.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FieldType {
    /// An integer: a stored number, an index or a derived measure.
    Number,
    /// A name: a flat, texture, definition name or category.
    Text,
    /// A derived boolean.
    Boolean,
    /// A raw bitset read through the linedef flag table.
    LineFlags,
    /// A raw bitset read through the thing flag table.
    ThingFlags,
    /// A sector's role names.
    Roles,
}

/// One queryable field: its wire name and comparable shape.
pub(super) struct FieldSpec {
    /// The name a filter spells to read this field.
    pub(super) name: &'static str,
    /// How its value reads.
    pub(super) ty: FieldType,
}

/// One table row, for writing the tables below.
const fn field(name: &'static str, ty: FieldType) -> FieldSpec {
    FieldSpec { name, ty }
}

/// The fields of every sector.
const SECTOR_FIELDS: &[FieldSpec] = &[
    field("index", FieldType::Number),
    field("floor", FieldType::Number),
    field("ceiling", FieldType::Number),
    field("height", FieldType::Number),
    field("light", FieldType::Number),
    field("floor_flat", FieldType::Text),
    field("ceiling_flat", FieldType::Text),
    field("special", FieldType::Number),
    field("tag", FieldType::Number),
    field("area", FieldType::Number),
    field("roles", FieldType::Roles),
];

/// The linedef fields both families share.
const LINEDEF_FIELDS: &[FieldSpec] = &[
    field("index", FieldType::Number),
    field("v1", FieldType::Number),
    field("v2", FieldType::Number),
    field("flags", FieldType::LineFlags),
    field("special", FieldType::Number),
    field("front_sector", FieldType::Number),
    field("back_sector", FieldType::Number),
    field("two_sided", FieldType::Boolean),
    field("length", FieldType::Number),
    field("front_upper", FieldType::Text),
    field("front_middle", FieldType::Text),
    field("front_lower", FieldType::Text),
    field("back_upper", FieldType::Text),
    field("back_middle", FieldType::Text),
    field("back_lower", FieldType::Text),
];

/// The `Doom` family's extra linedef field: the trigger tag.
const LINEDEF_DOOM_EXTRA: &[FieldSpec] = &[field("tag", FieldType::Number)];

/// The `Hexen` family's extra linedef fields: the script arguments.
const LINEDEF_HEXEN_EXTRA: &[FieldSpec] = &[
    field("arg0", FieldType::Number),
    field("arg1", FieldType::Number),
    field("arg2", FieldType::Number),
    field("arg3", FieldType::Number),
    field("arg4", FieldType::Number),
];

/// The fields of every sidedef.
const SIDEDEF_FIELDS: &[FieldSpec] = &[
    field("index", FieldType::Number),
    field("sector", FieldType::Number),
    field("upper", FieldType::Text),
    field("middle", FieldType::Text),
    field("lower", FieldType::Text),
    field("x_offset", FieldType::Number),
    field("y_offset", FieldType::Number),
    field("linedef", FieldType::Number),
];

/// The thing fields both families share.
const THING_FIELDS: &[FieldSpec] = &[
    field("index", FieldType::Number),
    field("type", FieldType::Number),
    field("name", FieldType::Text),
    field("category", FieldType::Text),
    field("x", FieldType::Number),
    field("y", FieldType::Number),
    field("angle", FieldType::Number),
    field("flags", FieldType::ThingFlags),
    field("sector", FieldType::Number),
];

/// The `Hexen` family's extra thing fields.
const THING_HEXEN_EXTRA: &[FieldSpec] = &[
    field("tid", FieldType::Number),
    field("special", FieldType::Number),
];

/// The fields of every vertex.
const VERTEX_FIELDS: &[FieldSpec] = &[
    field("index", FieldType::Number),
    field("x", FieldType::Number),
    field("y", FieldType::Number),
];

/// The kind's own fields, the ones every family carries.
const fn base_fields(kind: ElementKind) -> &'static [FieldSpec] {
    match kind {
        ElementKind::Sectors => SECTOR_FIELDS,
        ElementKind::Linedefs => LINEDEF_FIELDS,
        ElementKind::Sidedefs => SIDEDEF_FIELDS,
        ElementKind::Things => THING_FIELDS,
        ElementKind::Vertices => VERTEX_FIELDS,
    }
}

/// The `Doom`/`Hexen`-only fields one kind carries.
const fn extra_fields(kind: ElementKind, family: FlagFamily) -> &'static [FieldSpec] {
    match (kind, family) {
        (ElementKind::Linedefs, FlagFamily::Doom) => LINEDEF_DOOM_EXTRA,
        (ElementKind::Linedefs, FlagFamily::Hexen) => LINEDEF_HEXEN_EXTRA,
        (ElementKind::Things, FlagFamily::Hexen) => THING_HEXEN_EXTRA,
        (ElementKind::Things, FlagFamily::Doom)
        | (
            ElementKind::Sectors | ElementKind::Sidedefs | ElementKind::Vertices,
            FlagFamily::Doom | FlagFamily::Hexen,
        ) => &[],
    }
}

/// The field named `name` in the kind's table, `None` when it has none.
pub(super) fn field_spec(
    kind: ElementKind,
    family: FlagFamily,
    name: &str,
) -> Option<&'static FieldSpec> {
    base_fields(kind)
        .iter()
        .chain(extra_fields(kind, family))
        .find(|spec| spec.name == name)
}

/// The valid field names of one kind and family, comma-joined for errors.
pub(super) fn valid_fields(kind: ElementKind, family: FlagFamily) -> String {
    base_fields(kind)
        .iter()
        .chain(extra_fields(kind, family))
        .map(|spec| spec.name)
        .collect::<Vec<&str>>()
        .join(", ")
}

/// A field value read off one element, widened to comparable shapes.
pub(super) enum FValue<'map> {
    /// A number.
    Num(i64),
    /// A name.
    Text(Cow<'map, str>),
    /// A derived boolean.
    Bool(bool),
    /// A flag bitset with the kind whose table names its bits.
    Bits(u32, FlagKind),
    /// A sector's role names.
    Roles(Vec<String>),
}
