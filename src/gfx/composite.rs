//! Composite wall textures: folding a texture's patch stack into one image.
//!
//! [`compose`] draws a list of [`Component`] patches onto a transparent
//! indexed canvas in record order (later patches overdraw earlier ones)
//! and reports which components had no pixels to draw. It is pure: lump
//! lookup and decoding happen in `session/resolve/texture.rs`, which feeds
//! this with whatever it found.

use crate::error::CyberError;
use crate::gfx::image::IndexedImage;
use crate::gfx::pixel_count;

/// One placement of a texture's patch stack: the component's name, its
/// pixels (`None` when the patch lump is missing or undecodable) and its
/// origin in the texture.
#[derive(Clone, Copy, Debug)]
pub struct Component<'comp> {
    /// The component's patch name (or a placeholder naming its `PNAMES`
    /// index), used in the `missing` and `problems` reports.
    pub name: &'comp str,
    /// The patch's pixels; transparent pixels of a present image never
    /// overwrite what was drawn below.
    pub image: Option<&'comp IndexedImage>,
    /// Horizontal origin, in pixels, from the texture's left edge.
    pub x: i32,
    /// Vertical origin, in pixels, from the texture's top edge.
    pub y: i32,
}

/// A composed texture: the flattened image plus the report of what was
/// missing while drawing it.
#[derive(Clone, Debug)]
pub struct Composite {
    /// The texture's pixels: transparent wherever no component drew.
    pub image: IndexedImage,
    /// The names of the components that had no image, in draw order.
    pub missing: Vec<String>,
    /// One human-readable line per missing component, naming the texture.
    pub problems: Vec<String>,
}

/// Compose the texture `name`, `width` by `height`, from `components` in
/// draw order.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when `width` or `height` does not
/// describe a valid [`IndexedImage`] shape: a zero edge, an edge over
/// [`crate::gfx::MAX_DIMENSION`], or a product that does not fit.
#[inline]
pub fn compose(
    name: &str,
    width: u32,
    height: u32,
    components: &[Component<'_>],
) -> Result<Composite, CyberError> {
    let total = pixel_count(width, height).unwrap_or(0);
    let mut image = IndexedImage::new(width, height, vec![0_u8; total], vec![false; total])?;
    let mut missing = Vec::new();
    let mut problems = Vec::new();
    for component in components {
        if let Some(patch) = component.image {
            blit(&mut image, patch, component.x, component.y);
        } else {
            missing.push(component.name.to_owned());
            problems.push(format!(
                "texture {name} is missing patch {}",
                component.name
            ));
        }
    }
    Ok(Composite {
        image,
        missing,
        problems,
    })
}

/// Draws `patch`'s opaque pixels onto `target` at the origin `x`/`y`,
/// clipping whatever falls outside the target.
fn blit(target: &mut IndexedImage, patch: &IndexedImage, x: i32, y: i32) {
    let Ok(row_len) = usize::try_from(patch.width) else {
        return;
    };
    for (row, (indices, mask)) in patch
        .indices
        .chunks(row_len)
        .zip(patch.opaque.chunks(row_len))
        .enumerate()
    {
        let Ok(dy) = u32::try_from(y.saturating_add(i32::try_from(row).unwrap_or(i32::MAX))) else {
            continue;
        };
        blit_row(target, (indices, mask), x, dy);
    }
}

/// Draws one patch row's opaque pixels onto `target` row `dy`, offset by
/// `x`; out-of-bounds columns are dropped by [`IndexedImage::set`].
fn blit_row(target: &mut IndexedImage, row: (&[u8], &[bool]), x: i32, dy: u32) {
    let (indices, mask) = row;
    for (column, (&index, &visible)) in indices.iter().zip(mask).enumerate() {
        if !visible {
            continue;
        }
        let Ok(dx) = u32::try_from(x.saturating_add(i32::try_from(column).unwrap_or(i32::MAX)))
        else {
            continue;
        };
        target.set(dx, dy, (index, true));
    }
}

#[cfg(test)]
mod tests {
    use super::{Component, blit, compose};
    use crate::gfx::image::IndexedImage;

    /// A `width`x`height` image, fully opaque, every pixel `fill`.
    fn solid(width: u32, height: u32, fill: u8) -> IndexedImage {
        let total = usize::try_from(width)
            .expect("test width")
            .checked_mul(usize::try_from(height).expect("test height"))
            .expect("test pixels fit");
        IndexedImage::new(width, height, vec![fill; total], vec![true; total])
            .expect("test shapes are valid")
    }

    /// A `width`x`height` image with a transparent hole at `(hole_x, hole_y)`.
    fn holed(width: u32, height: u32, fill: u8, hole: (u32, u32)) -> IndexedImage {
        let mut image = solid(width, height, fill);
        assert!(image.set(hole.0, hole.1, (0, false)), "the hole is inside");
        image
    }

    /// The first texture problem line, for message checks.
    fn only_problem(problems: &[String]) -> &str {
        problems
            .first()
            .map(String::as_str)
            .expect("the test reports one problem")
    }

    #[test]
    fn components_draw_in_order_at_their_origins() {
        let base = solid(2, 2, 10);
        let top = solid(2, 2, 20);
        let components = [
            Component {
                name: "BASE",
                image: Some(&base),
                x: 0,
                y: 0,
            },
            Component {
                name: "TOP",
                image: Some(&top),
                x: 2,
                y: 0,
            },
        ];
        let composite = compose("WALL1", 4, 2, &components).expect("4x2 composes");
        assert_eq!((composite.image.width, composite.image.height), (4, 2));
        assert_eq!(
            composite.image.get(1, 1),
            Some((10, true)),
            "the base shows"
        );
        assert_eq!(composite.image.get(2, 0), Some((20, true)), "the top shows");
        assert!(
            composite.missing.is_empty() && composite.problems.is_empty(),
            "nothing missing"
        );
    }

    #[test]
    fn transparent_source_pixels_leave_what_was_below() {
        let base = solid(2, 1, 10);
        let patch = holed(2, 1, 20, (1, 0));
        let components = [
            Component {
                name: "BASE",
                image: Some(&base),
                x: 0,
                y: 0,
            },
            Component {
                name: "PATCH",
                image: Some(&patch),
                x: 0,
                y: 0,
            },
        ];
        let composite = compose("WALL1", 2, 1, &components).expect("2x1 composes");
        assert_eq!(
            composite.image.get(0, 0),
            Some((20, true)),
            "the opaque pixel overdraws"
        );
        assert_eq!(
            composite.image.get(1, 0),
            Some((10, true)),
            "the hole keeps the base"
        );
    }

    #[test]
    fn components_are_clipped_at_every_edge() {
        let patch = solid(2, 2, 30);
        let mut shifted = solid(2, 2, 0);
        blit(&mut shifted, &patch, -1, -1);
        assert_eq!(
            shifted.get(0, 0),
            Some((30, true)),
            "the bottom-right corner lands inside"
        );
        assert_eq!(shifted.get(1, 1), Some((0, true)), "the rest stays");
        let mut corner = solid(2, 2, 0);
        blit(&mut corner, &patch, 1, 1);
        assert_eq!(
            corner.get(0, 0),
            Some((0, true)),
            "the off-canvas part is dropped"
        );
        assert_eq!(
            corner.get(1, 1),
            Some((30, true)),
            "the overlap corner draws"
        );
    }

    #[test]
    fn missing_components_stay_holes_and_are_reported() {
        let patch = solid(1, 1, 40);
        let components = [
            Component {
                name: "GONE",
                image: None,
                x: 0,
                y: 0,
            },
            Component {
                name: "HERE",
                image: Some(&patch),
                x: 1,
                y: 0,
            },
        ];
        let composite = compose("STEP1", 2, 1, &components).expect("2x1 composes");
        assert_eq!(
            composite.missing,
            vec!["GONE".to_owned()],
            "the name is reported"
        );
        assert_eq!(
            only_problem(&composite.problems),
            "texture STEP1 is missing patch GONE",
            "the line names the texture"
        );
        assert_eq!(
            composite.image.get(0, 0),
            Some((0, false)),
            "the hole stays transparent"
        );
        assert_eq!(
            composite.image.get(1, 0),
            Some((40, true)),
            "the present patch draws"
        );
    }

    #[test]
    fn a_zero_sized_texture_is_refused() {
        let error = compose("BAD", 0, 4, &[]).expect_err("width 0 is no texture");
        assert_eq!(error.code(), "invalid_argument", "shape code");
    }
}
