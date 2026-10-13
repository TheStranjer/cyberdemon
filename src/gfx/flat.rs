//! `Doom` flat lumps: raw `PLAYPAL` indices, row-major, no header and no
//! transparency.
//!
//! [`decode_flat`] maps a flat's bytes onto the shape its size implies:
//! `4096` bytes for the vanilla `64x64`, `8192` for the `Heretic`/`Hexen`
//! `64x128`, `16384` for `128x128` and `65536` for `256x256`.
//! [`encode_flat`] writes such an image back and reports how many
//! transparent pixels it flattened to index `0`.

use crate::error::CyberError;
use crate::gfx::image::IndexedImage;
use crate::gfx::pixel_count;

/// The supported flat sizes with the shape they imply.
const FLAT_SHAPES: [(usize, u32, u32); 4] = [
    (4_096_usize, 64_u32, 64_u32),
    (8_192_usize, 64_u32, 128_u32),
    (0x4000_usize, 128_u32, 128_u32),
    (0x1_0000_usize, 256_u32, 256_u32),
];

/// The `(width, height)` a flat of `len` raw bytes must have, or `None`
/// for an unsupported size.
fn flat_shape(len: usize) -> Option<(u32, u32)> {
    FLAT_SHAPES
        .into_iter()
        .find(|&(size, _, _)| size == len)
        .map(|(_, width, height)| (width, height))
}

/// Whether a `width`x`height` image has a supported flat shape.
fn is_flat_shape(width: u32, height: u32) -> bool {
    FLAT_SHAPES
        .into_iter()
        .any(|(_, shape_width, shape_height)| shape_width == width && shape_height == height)
}

/// Decodes a flat lump into a fully opaque [`IndexedImage`].
///
/// # Errors
///
/// [`CyberError::Format`] when the lump's size implies no supported flat
/// shape.
#[inline]
pub fn decode_flat(bytes: &[u8]) -> Result<IndexedImage, CyberError> {
    let Some((width, height)) = flat_shape(bytes.len()) else {
        return Err(CyberError::Format(format!(
            "flat lumps hold 4096, 8192, 16384 or 65536 bytes, got {}; \
             check the lump is a real flat",
            bytes.len()
        )));
    };
    let opaque = vec![true; bytes.len()];
    IndexedImage::new(width, height, bytes.to_vec(), opaque)
}

/// The one row's flat bytes; transparent (and out-of-bounds) pixels are
/// written as index `0` and counted into `flattened`.
fn flatten_row(img: &IndexedImage, y: u32, bytes: &mut Vec<u8>, flattened: &mut usize) {
    for x in 0..img.width {
        let (index, visible) = img.get(x, y).unwrap_or((0, false));
        bytes.push(index.checked_mul(u8::from(visible)).unwrap_or(0));
        if !visible {
            *flattened = flattened.saturating_add(1);
        }
    }
}

/// Encodes an [`IndexedImage`] as a flat lump.
///
/// Flat lumps cannot store transparency: transparent pixels are written
/// as palette index `0`, and how many were flattened is returned with the
/// bytes.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when the image is not one of the four
/// supported flat shapes.
#[inline]
pub fn encode_flat(img: &IndexedImage) -> Result<(Vec<u8>, usize), CyberError> {
    if !is_flat_shape(img.width, img.height) {
        return Err(CyberError::InvalidArgument(format!(
            "flats are 64x64, 64x128, 128x128 or 256x256, got {}x{}; \
             resize the image before encoding",
            img.width, img.height
        )));
    }
    let count = pixel_count(img.width, img.height).ok_or_else(|| {
        CyberError::InvalidArgument(format!(
            "flat shape {}x{} does not fit",
            img.width, img.height
        ))
    })?;
    let mut bytes = Vec::with_capacity(count);
    let mut flattened = 0_usize;
    for y in 0..img.height {
        flatten_row(img, y, &mut bytes, &mut flattened);
    }
    Ok((bytes, flattened))
}

#[cfg(test)]
mod tests {
    use super::{decode_flat, encode_flat, flat_shape};
    use crate::error::CyberError;
    use crate::gfx::image::IndexedImage;

    /// An image of a flat shape with a position-dependent index in every
    /// (opaque) pixel.
    fn checker(width: u32, height: u32) -> IndexedImage {
        let columns = usize::try_from(width).expect("test width");
        let total = columns
            .checked_mul(usize::try_from(height).expect("test height"))
            .expect("test size");
        let mut indices = Vec::with_capacity(total);
        for n in 0..total {
            let x = u32::try_from(n.checked_rem(columns).expect("non-zero columns"))
                .expect("within width");
            let y = u32::try_from(n.checked_div(columns).expect("non-zero columns"))
                .expect("within height");
            let value = x.wrapping_add(y.saturating_mul(255)).to_le_bytes();
            indices.push(*value.first().expect("u32 has a low byte"));
        }
        let opaque = vec![true; total];
        IndexedImage::new(width, height, indices, opaque).expect("test flats are valid")
    }

    #[test]
    fn every_supported_size_implies_its_shape() {
        assert_eq!(flat_shape(4_096), Some((64, 64)), "vanilla");
        assert_eq!(flat_shape(8_192), Some((64, 128)), "Heretic/Hexen");
        assert_eq!(flat_shape(0x4000), Some((128, 128)), "wide");
        assert_eq!(flat_shape(0x1_0000), Some((256, 256)), "hi-res");
        assert_eq!(flat_shape(4_095), None, "just under");
        assert_eq!(flat_shape(0), None, "empty");
    }

    #[test]
    fn every_supported_size_round_trips() {
        for (width, height) in [(64_u32, 64_u32), (64, 128), (128, 128), (256, 256)] {
            let img = checker(width, height);
            let (bytes, flattened) = encode_flat(&img).expect("supported shapes encode");
            assert_eq!(flattened, 0, "an all-opaque flat has nothing to flatten");
            assert_eq!(bytes, img.indices, "the bytes are the indices verbatim");
            let back = decode_flat(&bytes).expect("what we wrote decodes");
            assert_eq!(back, img, "{width}x{height} round trips");
            assert_eq!((back.width, back.height), (width, height), "shape restored");
        }
    }

    #[test]
    fn unsupported_sizes_and_shapes_are_rejected() {
        let error = decode_flat(&[0_u8; 1_000]).expect_err("1000 bytes is no flat");
        assert_eq!(error.code(), "format_error", "bad size code");
        assert!(
            error.to_string().contains("4096"),
            "lists the sizes: {error}"
        );
        let CyberError::Format(empty) = decode_flat(&[]).expect_err("an empty lump") else {
            panic!("expected format_error for an empty flat");
        };
        assert!(empty.contains("flat"), "message is about flats: {empty}");
        let CyberError::InvalidArgument(message) =
            encode_flat(&checker(32, 32)).expect_err("32x32 is no flat")
        else {
            panic!("expected invalid_argument for an odd shape");
        };
        assert!(
            message.contains("64x64"),
            "message lists the shapes: {message}"
        );
    }

    #[test]
    fn transparent_pixels_become_index_zero_and_are_counted() {
        let mut img = checker(64, 64);
        assert!(img.set(1, 1, (0, false)), "poke one hole");
        assert!(img.set(2, 5, (9, false)), "poke another");
        let (bytes, flattened) = encode_flat(&img).expect("64x64 encodes");
        assert_eq!(flattened, 2, "two holes were flattened");
        assert!(bytes.len() > 322, "a full 64x64 flat");
        assert_eq!(bytes[65], 0, "the (1,1) pixel becomes index 0");
        assert_eq!(bytes[322], 0, "the (2,5) pixel becomes index 0");
        let back = decode_flat(&bytes).expect("the flat decodes back");
        assert!(
            back.opaque.iter().all(|visible| *visible),
            "flats are always fully opaque"
        );
        assert_eq!(
            back.get(1, 1),
            Some((0, true)),
            "a hole reads back as index 0"
        );
    }
}
