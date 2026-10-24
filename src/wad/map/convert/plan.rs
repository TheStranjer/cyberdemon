//! The resolved destination of one [`Target`]: [`Plan`], the layout a
//! request names for one map together with the direction its fields
//! travel, [`Direction`], whether that is inside the map's own flag family
//! or across it, and [`direction_between`], the family comparison that
//! decides. [`Plan::resolve`] runs before anything is rewritten, so a
//! request naming the layout and namespace the map already carries refuses
//! [`CyberError::InvalidArgument`] while `map` still stands unchanged.

use crate::error::CyberError;
use crate::wad::classify::MapFormat;
use crate::wad::map::flags::FlagFamily;
use crate::wad::map::model::Map;

use super::layout::{default_namespace, family_of};
use super::{Target, format_name};

/// The destination of one request, resolved against the map it applies to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Plan {
    /// The format to write.
    pub(super) format: MapFormat,
    /// The namespace to declare; `Some` exactly for a `UDMF` target, and
    /// then always the family default when the caller named none.
    pub(super) namespace: Option<String>,
    /// Which way the field translation runs between the map's family and
    /// the target's.
    pub(super) direction: Direction,
}

/// Whether one conversion stays inside the map's flag family, and if not,
/// which way the [`crate::wad::map::xlat`] stages translate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Direction {
    /// Source and target store their `flags` in the same family.
    SameFamily,
    /// `Doom` family to `Hexen` family: the table runs forward.
    ToHexen,
    /// `Hexen` family to `Doom` family: the table runs in reverse.
    FromHexen,
}

impl Plan {
    /// Resolves `target` against `map`: the namespace to declare, the
    /// direction the step takes and the check that it means anything (not
    /// the layout the map is already in).
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] as [`convert_map`](super::convert_map)
    /// reports it.
    pub(super) fn resolve(map: &Map, target: &Target) -> Result<Self, CyberError> {
        let namespace = target.namespace.clone().or_else(|| {
            (target.format == MapFormat::Udmf).then(|| default_namespace(map.flag_family()))
        });
        let plan = Self {
            format: target.format,
            direction: direction_between(map.flag_family(), target.format, namespace.as_deref()),
            namespace,
        };
        plan.require_change(map)?;
        Ok(plan)
    }

    /// Refuses a target that names the layout and namespace the map uses.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] saying the map already is in that
    /// format.
    fn require_change(&self, map: &Map) -> Result<(), CyberError> {
        if !self.names_the_same_layout(map) {
            return Ok(());
        }
        Err(CyberError::InvalidArgument(format!(
            "{} already is a {} map: nothing to convert; pass a different to (and, for UDMF, \
             namespace) pair",
            map.name,
            self.label()
        )))
    }

    /// Whether this plan rewrites nothing at all.
    fn names_the_same_layout(&self, map: &Map) -> bool {
        if map.format != self.format {
            return false;
        }
        match (self.namespace.as_deref(), map.udmf_namespace.as_deref()) {
            (Some(wanted), Some(current)) => wanted.eq_ignore_ascii_case(current),
            (None, None) => true,
            (Some(_), None) | (None, Some(_)) => false,
        }
    }

    /// The plan's layout as text: `hexen`, or `udmf (zdoom)`.
    fn label(&self) -> String {
        let format = format_name(self.format);
        self.namespace.as_deref().map_or_else(
            || format.to_owned(),
            |namespace| format!("{format} ({namespace})"),
        )
    }
}

/// Which way the field translation runs from a `source` family into the
/// target layout; a UDMF target that names no namespace reads as the
/// source's own family default, so it never changes family.
fn direction_between(source: FlagFamily, format: MapFormat, namespace: Option<&str>) -> Direction {
    match (source, family_of(format, namespace)) {
        (FlagFamily::Doom, FlagFamily::Hexen) => Direction::ToHexen,
        (FlagFamily::Hexen, FlagFamily::Doom) => Direction::FromHexen,
        (FlagFamily::Doom, FlagFamily::Doom) | (FlagFamily::Hexen, FlagFamily::Hexen) => {
            Direction::SameFamily
        }
    }
}
