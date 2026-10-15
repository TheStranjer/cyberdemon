//! Connected-component labelling of a [`Mask`]: [`label_components`]
//! numbers every run of matching pixels and measures it.
//!
//! The fill is an explicit-stack flood over the raster, scanning pixels in
//! row-major order: component ids start at `1` and follow the scan order of
//! each component's first pixel, so identical masks always label
//! identically. Alongside the [`Labels`] raster the pass returns one
//! [`Component`] per region with area, inclusive bounding box, coordinate
//! sums (for centroids) and a border-touch flag (used to tell enclosed
//! regions from background).

use crate::gfx::{pixel_count, pixel_index};
use crate::sketch::mask::Mask;

/// Pixel neighbourhood used by [`label_components`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Connectivity {
    /// Four orthogonal neighbours.
    Four,
    /// Four orthogonal plus four diagonal neighbours.
    Eight,
}

/// A labelled pixel raster; `0` means unlabelled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Labels {
    /// Pixels per row, `1..=8192`.
    pub width: u32,
    /// Rows, `1..=8192`.
    pub height: u32,
    /// One component id per pixel, row-major; `0` is unlabelled.
    pub ids: Vec<u32>,
}

impl Labels {
    /// A `width` x `height` raster of zeroes (unlabelled).
    #[must_use]
    #[inline]
    pub fn blank(width: u32, height: u32) -> Self {
        let count = pixel_count(width, height).unwrap_or_default();
        Self {
            width,
            height,
            ids: vec![0_u32; count],
        }
    }

    /// The id at `(x, y)`, or `None` when out of bounds.
    #[must_use]
    #[inline]
    pub fn get(&self, x: u32, y: u32) -> Option<u32> {
        let index = self.index_of(x, y)?;
        self.ids.get(index).copied()
    }

    /// Writes `id` at `(x, y)`; returns `false` when out of bounds.
    #[inline]
    pub fn set(&mut self, x: u32, y: u32, id: u32) -> bool {
        let Some(index) = self.index_of(x, y) else {
            return false;
        };
        let Some(slot) = self.ids.get_mut(index) else {
            return false;
        };
        *slot = id;
        true
    }

    /// The row-major index of `(x, y)`, `None` when out of bounds.
    #[must_use]
    #[inline]
    fn index_of(&self, x: u32, y: u32) -> Option<usize> {
        if x >= self.width || y >= self.height {
            return None;
        }
        pixel_index(self.width, x, y)
    }
}

/// One measured component of a [`label_components`] run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Component {
    /// Label written into [`Labels::ids`], `1..` in first-pixel scan order.
    pub id: u32,
    /// Pixels in the component.
    pub area: u32,
    /// Inclusive bounding box `(x0, y0, x1, y1)`.
    pub bbox: (u32, u32, u32, u32),
    /// Sum of the pixel x coordinates.
    pub sum_x: u64,
    /// Sum of the pixel y coordinates.
    pub sum_y: u64,
    /// Whether the component reaches the image border.
    pub touches_border: bool,
}

/// Labels every pixel whose bit equals `value`; `false` labels the paper,
/// `true` the ink. Ids start at `1` and follow the scan order of each
/// component's first pixel, so the result is deterministic.
#[must_use]
#[inline]
pub fn label_components(
    mask: &Mask,
    value: bool,
    connectivity: Connectivity,
) -> (Labels, Vec<Component>) {
    let mut sweep = Sweep::new(mask, value, connectivity);
    let mut components: Vec<Component> = Vec::new();
    for index in 0..mask.bits.len() {
        if mask.count_at(index) == Some(value) && sweep.unvisited(index) {
            let id = u32::try_from(components.len().saturating_add(1)).unwrap_or(u32::MAX);
            components.push(grow(&mut sweep, index, id));
        }
    }
    (sweep.into_labels(), components)
}

/// Mutable state of one [`label_components`] run, kept out of the
/// function's argument list.
struct Sweep<'mask> {
    /// Raster being labelled.
    mask: &'mask Mask,
    /// Bit whose components are collected.
    value: bool,
    /// Ids assigned so far; `0` means unvisited.
    ids: Vec<u32>,
    /// Pixels found but not yet expanded.
    stack: Vec<usize>,
    /// Id of the component currently growing.
    id: u32,
    /// Neighbour offsets of the chosen connectivity.
    offsets: &'static [(i64, i64)],
    /// Image width as a `usize`, `0` when it does not fit.
    width: usize,
}

impl<'mask> Sweep<'mask> {
    /// A sweep over `mask` collecting components of `value`.
    fn new(mask: &'mask Mask, value: bool, connectivity: Connectivity) -> Self {
        Self {
            mask,
            value,
            ids: vec![0_u32; mask.bits.len()],
            stack: Vec::new(),
            id: 0,
            offsets: offsets_for(connectivity),
            width: usize::try_from(mask.width).unwrap_or(0),
        }
    }

    /// Whether `index` has no id yet.
    fn unvisited(&self, index: usize) -> bool {
        self.ids.get(index).copied() == Some(0)
    }

    /// The `(x, y)` of a raster index, `None` when the shape does not fit.
    fn at_xy(&self, index: usize) -> Option<(u32, u32)> {
        let x = index.checked_rem(self.width)?;
        let y = index.checked_div(self.width)?;
        Some((u32::try_from(x).ok()?, u32::try_from(y).ok()?))
    }

    /// The raster index of `(x, y)` shifted by `offset`, `None` outside.
    fn neighbour_index(&self, x: u32, y: u32, offset: (i64, i64)) -> Option<usize> {
        let (dx, dy) = offset;
        let nx = i64::from(x)
            .checked_add(dx)
            .and_then(|v| u32::try_from(v).ok())?;
        let ny = i64::from(y)
            .checked_add(dy)
            .and_then(|v| u32::try_from(v).ok())?;
        self.mask.index_of(nx, ny)
    }

    /// Consumes the sweep, returning the label raster.
    fn into_labels(self) -> Labels {
        Labels {
            width: self.mask.width,
            height: self.mask.height,
            ids: self.ids,
        }
    }
}

/// Floods the component starting at `start`, assigns `id` and measures it.
fn grow(sweep: &mut Sweep<'_>, start: usize, id: u32) -> Component {
    sweep.id = id;
    if let Some(slot) = sweep.ids.get_mut(start) {
        *slot = id;
    }
    sweep.stack.clear();
    sweep.stack.push(start);
    let mut stats = Stats::new(sweep.mask.width, sweep.mask.height);
    while let Some(current) = sweep.stack.pop() {
        let Some((x, y)) = sweep.at_xy(current) else {
            continue;
        };
        stats.visit(x, y);
        push_unvisited(sweep, x, y);
    }
    stats.into_component(id)
}

/// Queues every in-bounds, matching, unvisited neighbour of `(x, y)`.
fn push_unvisited(sweep: &mut Sweep<'_>, x: u32, y: u32) {
    for &offset in sweep.offsets {
        let Some(index) = sweep.neighbour_index(x, y, offset) else {
            continue;
        };
        mark(sweep, index);
    }
}

/// Joins `index` to the growing component when it matches and is free.
fn mark(sweep: &mut Sweep<'_>, index: usize) {
    if sweep.mask.count_at(index) != Some(sweep.value) {
        return;
    }
    let Some(slot) = sweep.ids.get_mut(index) else {
        return;
    };
    if *slot != 0 {
        return;
    }
    *slot = sweep.id;
    sweep.stack.push(index);
}

/// Running statistics of one flood fill.
#[derive(Debug)]
struct Stats {
    /// Pixels visited.
    area: usize,
    /// Smallest x seen, `None` before the first pixel.
    x0: Option<u32>,
    /// Largest x seen.
    x1: u32,
    /// Smallest y seen, `None` before the first pixel.
    y0: Option<u32>,
    /// Largest y seen.
    y1: u32,
    /// Sum of the x coordinates.
    sum_x: u64,
    /// Sum of the y coordinates.
    sum_y: u64,
    /// Whether a visited pixel touches the image border.
    border: bool,
    /// Image width for the border test.
    width: u32,
    /// Image height for the border test.
    height: u32,
}

impl Stats {
    /// Empty statistics for a `width` x `height` image.
    const fn new(width: u32, height: u32) -> Self {
        Self {
            area: 0,
            x0: None,
            x1: 0,
            y0: None,
            y1: 0,
            sum_x: 0,
            sum_y: 0,
            border: false,
            width,
            height,
        }
    }

    /// Adds one pixel to the statistics.
    fn visit(&mut self, x: u32, y: u32) {
        self.area = self.area.saturating_add(1);
        self.x0 = Some(self.x0.unwrap_or(x).min(x));
        self.y0 = Some(self.y0.unwrap_or(y).min(y));
        self.x1 = self.x1.max(x);
        self.y1 = self.y1.max(y);
        self.sum_x = self.sum_x.saturating_add(u64::from(x));
        self.sum_y = self.sum_y.saturating_add(u64::from(y));
        if x == 0
            || y == 0
            || x.checked_add(1) == Some(self.width)
            || y.checked_add(1) == Some(self.height)
        {
            self.border = true;
        }
    }

    /// The measured component with label `id`.
    fn into_component(self, id: u32) -> Component {
        Component {
            id,
            area: u32::try_from(self.area).unwrap_or(u32::MAX),
            bbox: (self.x0.unwrap_or(0), self.y0.unwrap_or(0), self.x1, self.y1),
            sum_x: self.sum_x,
            sum_y: self.sum_y,
            touches_border: self.border,
        }
    }
}

/// 4-connected neighbour offsets.
const FOUR: [(i64, i64); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
/// 8-connected neighbour offsets.
const EIGHT: [(i64, i64); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
];

/// The neighbour offsets for one connectivity.
const fn offsets_for(connectivity: Connectivity) -> &'static [(i64, i64)] {
    match connectivity {
        Connectivity::Four => &FOUR,
        Connectivity::Eight => &EIGHT,
    }
}

#[cfg(test)]
mod tests {
    use super::{Connectivity, Labels, label_components};
    use crate::sketch::mask::Mask;

    /// A 5x5 mask with the bits set, given as (x, y) points.
    fn mask_with(points: &[(u32, u32)]) -> Mask {
        let mut mask = Mask::blank(5, 5);
        for &(x, y) in points {
            assert!(mask.set(x, y, true), "fixture in bounds");
        }
        mask
    }

    #[test]
    fn ids_follow_first_pixel_scan_order() {
        let mask = mask_with(&[(4, 0), (0, 2), (1, 2), (3, 4)]);
        let (labels, components) = label_components(&mask, true, Connectivity::Four);
        assert_eq!(components.len(), 3, "three ink components");
        assert_eq!(
            components.iter().map(|c| c.id).collect::<Vec<_>>(),
            vec![1, 2, 3],
            "ids are 1.. in scan order"
        );
        assert_eq!(
            components[0].bbox,
            (4, 0, 4, 0),
            "first is the single (4,0)"
        );
        assert_eq!(components[1].bbox, (0, 2, 1, 2), "second is the pair");
        assert_eq!(components[2].bbox, (3, 4, 3, 4), "third is the last pixel");
        assert_eq!(components[1].area, 2, "the pair is two pixels");
        assert_eq!(labels.get(0, 2), Some(2), "pair pixels share the id");
        assert_eq!(labels.get(1, 2), Some(2), "pair pixels share the id");
        assert_eq!(labels.get(4, 0), Some(1), "single pixel id");
    }

    #[test]
    fn diagonal_is_one_component_at_eight_and_two_at_four() {
        let mask = mask_with(&[(1, 1), (2, 2)]);
        let (_, joined) = label_components(&mask, true, Connectivity::Eight);
        assert_eq!(joined.len(), 1, "8-connectivity joins the diagonal");
        assert_eq!(joined[0].area, 2, "both pixels in one component");
        let (_, split) = label_components(&mask, true, Connectivity::Four);
        assert_eq!(split.len(), 2, "4-connectivity keeps them apart");
        assert_eq!(split[0].bbox, (1, 1, 1, 1), "first scan pixel leads");
        assert_eq!(split[1].bbox, (2, 2, 2, 2), "second follows");
    }

    #[test]
    fn stats_carry_sums_and_border_flags() {
        let mask = mask_with(&[(1, 1), (2, 1), (1, 2), (2, 2)]);
        let (block_labels, components) = label_components(&mask, true, Connectivity::Four);
        assert_eq!(components.len(), 1, "the 2x2 block is one component");
        assert_eq!(block_labels.get(2, 1), Some(1), "block pixels carry id 1");
        assert_eq!(
            block_labels.get(0, 0),
            Some(0),
            "background stays unlabelled"
        );
        let block = &components[0];
        assert_eq!(block.bbox, (1, 1, 2, 2), "tight bounding box");
        assert_eq!(block.sum_x, 6, "1 + 2 + 1 + 2");
        assert_eq!(block.sum_y, 6, "1 + 1 + 2 + 2");
        assert!(!block.touches_border, "an interior block does not touch");
        let mut frame = Mask::blank(5, 5);
        let border = (0..5_u32).flat_map(|t| [(t, 0), (t, 4), (0, t), (4, t)]);
        for (x, y) in border {
            assert!(frame.set(x, y, true), "fixture in bounds");
        }
        let (labels, border_components) = label_components(&frame, true, Connectivity::Four);
        assert_eq!(border_components.len(), 1, "the frame is connected");
        assert!(
            border_components[0].touches_border,
            "and touches the border"
        );
        let holes = label_components(&frame, false, Connectivity::Four).1;
        assert_eq!(holes.len(), 1, "the paper inside the frame is one hole");
        assert!(!holes[0].touches_border, "the 3x3 hole does not touch");
        assert_eq!(holes[0].area, 9, "hole area");
        assert_eq!(
            labels.get(0, 2),
            Some(border_components[0].id),
            "frame pixels carry the ink id"
        );
        let paper_labels = label_components(&frame, false, Connectivity::Four).0;
        assert_eq!(
            paper_labels.get(2, 2),
            Some(holes[0].id),
            "hole id at its centre"
        );
    }

    #[test]
    fn labels_get_set_and_blank_shape() {
        let mut labels = Labels::blank(2, 2);
        assert_eq!(labels.ids.len(), 4, "blank sized by shape");
        assert_eq!(labels.get(0, 0), Some(0), "starts unlabelled");
        assert_eq!(labels.get(2, 0), None, "x out of bounds");
        assert_eq!(labels.get(0, 5), None, "y out of bounds");
        assert!(labels.set(1, 1, 7), "in-bounds set");
        assert_eq!(labels.get(1, 1), Some(7), "id reads back");
        assert!(!labels.set(4, 4, 1), "out-of-bounds set reports false");
    }

    #[test]
    fn empty_mask_labels_nothing() {
        let mask = Mask::blank(0, 0);
        let (labels, components) = label_components(&mask, false, Connectivity::Eight);
        assert!(components.is_empty(), "no components in an empty mask");
        assert!(labels.ids.is_empty(), "no ids in an empty raster");
    }
}
