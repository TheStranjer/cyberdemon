//! The per-kind field enums and their wire-name tables.
//!
//! Every editable field of every element block is one [`Field`]: a
//! [`SectorField`], [`SidedefField`], [`VertexField`], [`LinedefField`] or
//! [`ThingField`] variant. The [`KindPlan`](super::tables::KindPlan)
//! tables hold these values, the [`access`](super::access) readers and
//! writers dispatch on them arm by arm, and [`Job::field`] parses a
//! request's wire name onto one variant exactly once per request, so a
//! field the engine lists can never silently read or write nothing.
//! Each enum's `ALL` is every variant in wire order; the engine's table
//! tests keep the name tables, the `ALL` lists and the plan tables
//! agreeing.
//!
//! [`Job::field`]: super::job::Job::field

use crate::wad::map::query::ElementKind;

/// One editable field of one element kind: the key the field tables, the
/// [`Pending`](super::job::Pending) plans and the
/// [`access`](super::access) dispatch all run on. The payload carries the
/// element kind, so a sector field can never be looked up on a sector row
/// by accident.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Field {
    /// A [`Sector`](crate::wad::map::Sector) row's field.
    Sector(SectorField),
    /// A [`Sidedef`](crate::wad::map::Sidedef) row's field.
    Sidedef(SidedefField),
    /// A [`Vertex`](crate::wad::map::Vertex) row's field.
    Vertex(VertexField),
    /// A [`Linedef`](crate::wad::map::Linedef) row's field.
    Linedef(LinedefField),
    /// A [`Thing`](crate::wad::map::Thing) row's field.
    Thing(ThingField),
}

impl Field {
    /// The kind's field one exact wire name names, `None` for a name the
    /// kind does not carry at all. Cross-kind names ("x" is a vertex
    /// coordinate and a thing coordinate alike) stay unambiguous because
    /// the parse runs on the requested kind.
    pub(super) fn of(kind: ElementKind, name: &str) -> Option<Self> {
        match kind {
            ElementKind::Sectors => SectorField::from_name(name).map(Self::Sector),
            ElementKind::Sidedefs => SidedefField::from_name(name).map(Self::Sidedef),
            ElementKind::Vertices => VertexField::from_name(name).map(Self::Vertex),
            ElementKind::Linedefs => LinedefField::from_name(name).map(Self::Linedef),
            ElementKind::Things => ThingField::from_name(name).map(Self::Thing),
        }
    }

    /// The exact (lowercase) wire name this field is tabled and
    /// requested under.
    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::Sector(field) => field.name(),
            Self::Sidedef(field) => field.name(),
            Self::Vertex(field) => field.name(),
            Self::Linedef(field) => field.name(),
            Self::Thing(field) => field.name(),
        }
    }
}

/// A [`Sector`](crate::wad::map::Sector) row's editable field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum SectorField {
    /// `ceiling`: the row's `ceiling_height`.
    Ceiling,
    /// `ceiling_flat`: the row's `ceiling_flat` name.
    CeilingFlat,
    /// `floor`: the row's `floor_height`.
    Floor,
    /// `floor_flat`: the row's `floor_flat` name.
    FloorFlat,
    /// `light`: the row's light level.
    Light,
    /// `special`: the row's sector effect number.
    Special,
    /// `tag`: the row's trigger tag.
    Tag,
}

impl SectorField {
    /// Every sector field in table order, for the table tests.
    #[cfg(test)]
    pub(super) const ALL: [Self; 7] = [
        Self::Ceiling,
        Self::CeilingFlat,
        Self::Floor,
        Self::FloorFlat,
        Self::Light,
        Self::Special,
        Self::Tag,
    ];

    /// The wire name this field is tabled and requested under.
    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::Ceiling => "ceiling",
            Self::CeilingFlat => "ceiling_flat",
            Self::Floor => "floor",
            Self::FloorFlat => "floor_flat",
            Self::Light => "light",
            Self::Special => "special",
            Self::Tag => "tag",
        }
    }

    /// The field one exact wire name names.
    pub(super) fn from_name(name: &str) -> Option<Self> {
        match name {
            "ceiling" => Some(Self::Ceiling),
            "ceiling_flat" => Some(Self::CeilingFlat),
            "floor" => Some(Self::Floor),
            "floor_flat" => Some(Self::FloorFlat),
            "light" => Some(Self::Light),
            "special" => Some(Self::Special),
            "tag" => Some(Self::Tag),
            _ => None,
        }
    }
}

/// A [`Sidedef`](crate::wad::map::Sidedef) row's editable field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum SidedefField {
    /// `lower`: the lower texture name.
    Lower,
    /// `middle`: the middle texture name.
    Middle,
    /// `upper`: the upper texture name.
    Upper,
    /// `x_offset`: the texture shift along the line.
    XOffset,
    /// `y_offset`: the texture shift across the line.
    YOffset,
}

impl SidedefField {
    /// Every sidedef field in table order, for the table tests.
    #[cfg(test)]
    pub(super) const ALL: [Self; 5] = [
        Self::Lower,
        Self::Middle,
        Self::Upper,
        Self::XOffset,
        Self::YOffset,
    ];

    /// The wire name this field is tabled and requested under.
    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::Lower => "lower",
            Self::Middle => "middle",
            Self::Upper => "upper",
            Self::XOffset => "x_offset",
            Self::YOffset => "y_offset",
        }
    }

    /// The field one exact wire name names.
    pub(super) fn from_name(name: &str) -> Option<Self> {
        match name {
            "lower" => Some(Self::Lower),
            "middle" => Some(Self::Middle),
            "upper" => Some(Self::Upper),
            "x_offset" => Some(Self::XOffset),
            "y_offset" => Some(Self::YOffset),
            _ => None,
        }
    }
}

/// A [`Vertex`](crate::wad::map::Vertex) row's editable field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum VertexField {
    /// `x`: the coordinate along the map's first axis.
    X,
    /// `y`: the coordinate along the map's second axis.
    Y,
}

impl VertexField {
    /// Every vertex field in table order, for the table tests.
    #[cfg(test)]
    pub(super) const ALL: [Self; 2] = [Self::X, Self::Y];

    /// The wire name this field is tabled and requested under.
    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::X => "x",
            Self::Y => "y",
        }
    }

    /// The field one exact wire name names.
    pub(super) fn from_name(name: &str) -> Option<Self> {
        match name {
            "x" => Some(Self::X),
            "y" => Some(Self::Y),
            _ => None,
        }
    }
}

/// A [`Linedef`](crate::wad::map::Linedef) row's editable field. The two
/// flag families table different subsets (the `Doom` family's `tag`, the
/// `Hexen` family's `arg0..arg4`), but both store them in the same model
/// columns, so one enum covers the two tables.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum LinedefField {
    /// `arg0`: the first `Hexen` script argument.
    Arg0,
    /// `arg1`: the second `Hexen` script argument.
    Arg1,
    /// `arg2`: the third `Hexen` script argument.
    Arg2,
    /// `arg3`: the fourth `Hexen` script argument.
    Arg3,
    /// `arg4`: the fifth `Hexen` script argument.
    Arg4,
    /// `special`: the row's action number.
    Special,
    /// `tag`: the row's trigger tag, tabled on the `Doom` family only.
    Tag,
}

impl LinedefField {
    /// Every linedef field in table order, for the table tests.
    #[cfg(test)]
    pub(super) const ALL: [Self; 7] = [
        Self::Arg0,
        Self::Arg1,
        Self::Arg2,
        Self::Arg3,
        Self::Arg4,
        Self::Special,
        Self::Tag,
    ];

    /// The wire name this field is tabled and requested under.
    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::Arg0 => "arg0",
            Self::Arg1 => "arg1",
            Self::Arg2 => "arg2",
            Self::Arg3 => "arg3",
            Self::Arg4 => "arg4",
            Self::Special => "special",
            Self::Tag => "tag",
        }
    }

    /// The field one exact wire name names.
    pub(super) fn from_name(name: &str) -> Option<Self> {
        match name {
            "arg0" => Some(Self::Arg0),
            "arg1" => Some(Self::Arg1),
            "arg2" => Some(Self::Arg2),
            "arg3" => Some(Self::Arg3),
            "arg4" => Some(Self::Arg4),
            "special" => Some(Self::Special),
            "tag" => Some(Self::Tag),
            _ => None,
        }
    }
}

/// A [`Thing`](crate::wad::map::Thing) row's editable field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum ThingField {
    /// `angle`: the spawn facing in degrees.
    Angle,
    /// `type`: the doomednum (the model row's `kind`).
    Type,
    /// `x`: the coordinate along the map's first axis.
    X,
    /// `y`: the coordinate along the map's second axis.
    Y,
}

impl ThingField {
    /// Every thing field in table order, for the table tests.
    #[cfg(test)]
    pub(super) const ALL: [Self; 4] = [Self::Angle, Self::Type, Self::X, Self::Y];

    /// The wire name this field is tabled and requested under.
    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::Angle => "angle",
            Self::Type => "type",
            Self::X => "x",
            Self::Y => "y",
        }
    }

    /// The field one exact wire name names.
    pub(super) fn from_name(name: &str) -> Option<Self> {
        match name {
            "angle" => Some(Self::Angle),
            "type" => Some(Self::Type),
            "x" => Some(Self::X),
            "y" => Some(Self::Y),
            _ => None,
        }
    }
}
