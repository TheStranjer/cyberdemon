//! The rows a rebuild is writing up and the dressing they carry.
//!
//! [`Rebuilt`] is the finished set of new vertices, linedefs and sidedefs
//! beside its report: it is what [`apply_overlay`](super::apply_overlay)
//! commits, and writing it is the only step that cannot fail. [`Plan`]
//! holds the vertex rows and the grid lookup [`Built`] appends its lines
//! against, one [`LineCtx`] per arrangement piece.
//!
//! The dressing is the `Doom` rule set: [`orient_piece`] puts a piece's
//! sector on its right, [`shell_of`] normalizes the flags around it,
//! [`transition_face`] keeps a face only where the two sides' heights
//! differ, and [`reconcile_textures`] moves a copied side's textures when
//! the line's sidedness changed under it.

use alloc::collections::BTreeMap;

use crate::error::CyberError;
use crate::geom::label::Label;
use crate::geom::{Piece, Pt};
use crate::wad::map::flags::{BLOCKING, TWO_SIDED};
use crate::wad::map::model::{Linedef, Map, Sector, Sidedef, Vertex};

use super::geometry::plan_point;
use super::{NO_TEXTURE, OverlayResult, Source};

/// The rebuilt rows plus the report, waiting to be written into the map.
pub(super) struct Rebuilt {
    /// The new vertices.
    pub(super) vertices: Vec<Vertex>,
    /// The new linedefs.
    pub(super) linedefs: Vec<Linedef>,
    /// The new sidedefs.
    pub(super) sidedefs: Vec<Sidedef>,
    /// The sector record a [`super::Paint::Sector`] appends.
    pub(super) new_record: Option<Sector>,
    /// The report to return.
    pub(super) result: OverlayResult,
}

impl Rebuilt {
    /// Write the rebuild into `map`. Infallible, so an error anywhere in
    /// [`apply_overlay`](super::apply_overlay) leaves the model untouched.
    pub(super) fn apply_to(self, map: &mut Map) {
        map.vertices = self.vertices;
        map.linedefs = self.linedefs;
        map.sidedefs = self.sidedefs;
        if let Some(record) = self.new_record {
            map.sectors.push(record);
        }
    }
}

/// The planned vertex rows: the rebuilt list, the grid lookup and the
/// indices of the appended corners.
pub(super) struct Plan {
    /// The new vertex list: reused originals in index order, then the
    /// new points in ascending coordinate order.
    pub(super) vertices: Vec<Vertex>,
    /// Grid point to index in `vertices`.
    pub(super) index: BTreeMap<Pt, usize>,
    /// Indices of the appended vertices, ascending.
    pub(super) created: Vec<usize>,
}

/// The linedefs and sidedefs under construction and their reports.
#[derive(Default)]
pub(super) struct Built {
    /// The new linedefs, in piece order.
    pub(super) linedefs: Vec<Linedef>,
    /// The new sidedefs, front before back per linedef.
    pub(super) sidedefs: Vec<Sidedef>,
    /// New indices of linedefs no original line contributed to.
    pub(super) created_lines: Vec<usize>,
    /// New indices of freshly dressed sidedefs.
    pub(super) created_sides: Vec<usize>,
    /// Old linedef index to the new index of its first piece.
    pub(super) first_of: BTreeMap<usize, usize>,
}

impl Built {
    /// Append one side and report its index, listing freshly dressed
    /// sides in `created_sides`.
    pub(super) fn push_side(&mut self, side: Sidedef, fresh: bool) -> usize {
        let index = self.sidedefs.len();
        if fresh {
            self.created_sides.push(index);
        }
        self.sidedefs.push(side);
        index
    }

    /// Record the finished `line` and its origin.
    pub(super) fn register(&mut self, line: Linedef, old: Option<usize>) {
        let index = self.linedefs.len();
        match old {
            Some(previous) => {
                self.first_of.entry(previous).or_insert(index);
            }
            None => self.created_lines.push(index),
        }
        self.linedefs.push(line);
    }
}

/// One piece's finished linedef context: where it came from and which
/// sectors its two sides face.
pub(super) struct LineCtx<'ctx> {
    /// The original linedef the piece continues, when it runs along one.
    pub(super) source: Option<&'ctx Linedef>,
    /// The original start's side shift: the distance from the original
    /// start to the piece's near end.
    pub(super) start_len: i32,
    /// The original end's side shift: the distance from the piece's far
    /// end to the original end.
    pub(super) end_len: i32,
    /// The sector the front side faces.
    pub(super) front: usize,
    /// The sector the back side faces, `None` when the back is void.
    pub(super) back: Option<usize>,
    /// The line's start point.
    pub(super) start: Pt,
    /// The line's end point.
    pub(super) finish: Pt,
}

impl LineCtx<'_> {
    /// The original side that still owns `sector` for this slot, with
    /// the offset shift that keeps its texture in place. The source's
    /// front side is measured from the line's start, its back side from
    /// the line's end, whichever way the new line runs.
    pub(super) fn copied_side(
        &self,
        source: &Source,
        sector: usize,
        front_slot: bool,
    ) -> Option<(Sidedef, i32)> {
        let line = self.source?;
        let owns = |slot: Option<usize>, shift: i32| {
            slot.and_then(|index| source.map.sidedefs.get(index))
                .filter(|side| side.sector == sector)
                .cloned()
                .map(|side| (side, shift))
        };
        if front_slot {
            owns(line.front, self.start_len).or_else(|| owns(line.back, self.end_len))
        } else {
            owns(line.back, self.end_len).or_else(|| owns(line.front, self.start_len))
        }
    }
}

/// `texture` when the two sectors' face heights differ, `"-"` when they
/// match and the face would be invisible.
pub(super) fn transition_face(differ: bool, texture: &str) -> String {
    if differ {
        return texture.to_owned();
    }
    NO_TEXTURE.to_owned()
}

/// The linedef shell for `ctx`: endpoints, normalized flags and the
/// original line's special, tag, args and extras.
pub(super) fn shell_of(ctx: &LineCtx<'_>, plan: &Plan) -> Result<Linedef, CyberError> {
    let base = ctx.source.map_or(0_u32, |line| line.flags);
    let flags = if ctx.back.is_some() {
        (base | TWO_SIDED) & !BLOCKING
    } else {
        (base | BLOCKING) & !TWO_SIDED
    };
    Ok(Linedef {
        v1: plan_point(plan, ctx.start)?,
        v2: plan_point(plan, ctx.finish)?,
        flags,
        special: ctx.source.map_or(0_i32, |line| line.special),
        tag: ctx.source.map_or(0_i32, |line| line.tag),
        args: ctx.source.map_or([0_i32; 5], |line| line.args),
        front: None,
        back: None,
        extra: ctx
            .source
            .map_or_else(BTreeMap::new, |line| line.extra.clone()),
    })
}

/// The `Doom` orientation rule for one piece: the front side must be on
/// the right, so a piece whose right side is void runs backwards.
#[must_use]
pub(super) const fn orient_piece(piece: &Piece) -> (Pt, Pt, Label, Label) {
    if matches!(piece.right, Label::Void) {
        (piece.b, piece.a, piece.left, piece.right)
    } else {
        (piece.a, piece.b, piece.right, piece.left)
    }
}

/// Whether a copied side needs its textures moved because the line's
/// sidedness changed: `middle` onto free faces when a one-sided line
/// opens, an invisible face onto `middle` when a two-sided line closes.
pub(super) fn reconcile_textures(side: &mut Sidedef, ctx: &LineCtx<'_>) {
    let source_one_sided = ctx.source.is_none_or(|line| line.back.is_none());
    let now_two_sided = ctx.back.is_some();
    if source_one_sided && now_two_sided {
        move_middle_to_faces(side);
    }
    if !source_one_sided && !now_two_sided {
        move_faces_to_middle(side);
    }
}

/// Moves a side's middle texture onto its `upper` and `lower` where
/// those are free, then clears the middle.
fn move_middle_to_faces(side: &mut Sidedef) {
    if side.middle == NO_TEXTURE {
        return;
    }
    if side.upper == NO_TEXTURE {
        side.upper = side.middle.clone();
    }
    if side.lower == NO_TEXTURE {
        side.lower = side.middle.clone();
    }
    NO_TEXTURE.clone_into(&mut side.middle);
}

/// Moves an otherwise invisible `upper` (else `lower`) texture onto the
/// middle of a side whose line has just closed into one side.
fn move_faces_to_middle(side: &mut Sidedef) {
    if side.middle != NO_TEXTURE {
        return;
    }
    if let Some(texture) = [side.upper.as_str(), side.lower.as_str()]
        .into_iter()
        .find(|name| *name != NO_TEXTURE)
    {
        texture.clone_into(&mut side.middle);
    }
    NO_TEXTURE.clone_into(&mut side.upper);
    NO_TEXTURE.clone_into(&mut side.lower);
}

/// Drop repeated neighbours and a closing repeat so every ring edge has
/// positive length (the arrangement's own normalization, needed here for
/// the containment probes).
pub(super) fn normalize_ring(points: &[Pt]) -> Vec<Pt> {
    let mut out: Vec<Pt> = Vec::with_capacity(points.len());
    for point in points {
        if out.last() != Some(point) {
            out.push(*point);
        }
    }
    while out.len() > 1 && out.first() == out.last() {
        out.pop();
    }
    out
}
