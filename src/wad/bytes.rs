//! Bounds-checked little-endian byte access: a read cursor, an append
//! writer, and offset readers.
//!
//! [`Reader`] walks a borrowed slice, [`Writer`] accumulates bytes for a
//! future lump payload, and the `*_at` functions read one integer at a
//! fixed offset without a cursor. Every read past the end fails with
//! [`CyberError::Format`] naming the offset that ran out; nothing here
//! panics, and multi-byte values are little-endian, which is all `Doom`
//! formats ever use. [`usize_to_u32`] and [`u32_to_usize`] are the
//! checked conversions later parsers use instead of `as`.

use crate::error::CyberError;
use crate::wad::name::{LumpName, NAME_LEN};

/// The standard truncation failure: `what` could not be read at `pos`.
fn truncated(pos: usize, what: &str) -> CyberError {
    CyberError::Format(format!(
        "unexpected end of data at offset {pos} reading {what}"
    ))
}

/// A bounds-checked read cursor over borrowed bytes.
#[derive(Debug)]
pub struct Reader<'data> {
    /// The bytes behind the cursor.
    data: &'data [u8],
    /// The offset of the next byte to read.
    pos: usize,
}

impl<'data> Reader<'data> {
    /// A cursor at the start of `data`.
    #[must_use]
    #[inline]
    pub const fn new(data: &'data [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// A cursor positioned at `offset`. The offset is not validated here;
    /// reads from a cursor past the end fail as any other truncated read.
    #[must_use]
    #[inline]
    pub const fn at(data: &'data [u8], offset: usize) -> Self {
        Self { data, pos: offset }
    }

    /// The offset of the next byte this cursor would read.
    #[must_use]
    #[inline]
    pub const fn pos(&self) -> usize {
        self.pos
    }

    /// Bytes left to read; zero when the cursor sits at or past the end.
    #[must_use]
    #[inline]
    pub const fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    /// Moves the cursor to `offset`.
    ///
    /// # Errors
    ///
    /// [`CyberError::Format`] when `offset` is past the end of the data;
    /// the cursor keeps its old position.
    #[inline]
    pub fn seek(&mut self, offset: usize) -> Result<(), CyberError> {
        let len = self.data.len();
        if offset > len {
            return Err(CyberError::Format(format!(
                "cannot seek to offset {offset}, the data holds {len} bytes"
            )));
        }
        self.pos = offset;
        Ok(())
    }

    /// Consumes the next `count` bytes, advancing the cursor.
    #[inline]
    fn take_bytes(&mut self, count: usize, what: &str) -> Result<&'data [u8], CyberError> {
        let Some(end) = self.pos.checked_add(count) else {
            return Err(truncated(self.pos, what));
        };
        let Some(bytes) = self.data.get(self.pos..end) else {
            return Err(truncated(self.pos, what));
        };
        self.pos = end;
        Ok(bytes)
    }

    /// Consumes the next `count` bytes as an array.
    #[inline]
    fn read_array_as<const N: usize>(&mut self, what: &str) -> Result<[u8; N], CyberError> {
        let bytes = self.take_bytes(N, what)?;
        let mut out = [0_u8; N];
        out.copy_from_slice(bytes);
        Ok(out)
    }

    /// Reads one byte.
    ///
    /// # Errors
    ///
    /// [`CyberError::Format`] when the cursor is at the end of the data.
    #[inline]
    pub fn read_u8(&mut self) -> Result<u8, CyberError> {
        Ok(u8::from_le_bytes(self.read_array_as::<1>("u8")?))
    }

    /// Reads one byte as a signed value.
    ///
    /// # Errors
    ///
    /// [`CyberError::Format`] when the cursor is at the end of the data.
    #[inline]
    pub fn read_i8(&mut self) -> Result<i8, CyberError> {
        Ok(i8::from_le_bytes(self.read_array_as::<1>("i8")?))
    }

    /// Reads a two-byte little-endian unsigned value.
    ///
    /// # Errors
    ///
    /// [`CyberError::Format`] when fewer than two bytes remain.
    #[inline]
    pub fn read_u16(&mut self) -> Result<u16, CyberError> {
        Ok(u16::from_le_bytes(self.read_array_as::<2>("u16")?))
    }

    /// Reads a two-byte little-endian signed value.
    ///
    /// # Errors
    ///
    /// [`CyberError::Format`] when fewer than two bytes remain.
    #[inline]
    pub fn read_i16(&mut self) -> Result<i16, CyberError> {
        Ok(i16::from_le_bytes(self.read_array_as::<2>("i16")?))
    }

    /// Reads a four-byte little-endian unsigned value.
    ///
    /// # Errors
    ///
    /// [`CyberError::Format`] when fewer than four bytes remain.
    #[inline]
    pub fn read_u32(&mut self) -> Result<u32, CyberError> {
        Ok(u32::from_le_bytes(self.read_array_as::<4>("u32")?))
    }

    /// Reads a four-byte little-endian signed value.
    ///
    /// # Errors
    ///
    /// [`CyberError::Format`] when fewer than four bytes remain.
    #[inline]
    pub fn read_i32(&mut self) -> Result<i32, CyberError> {
        Ok(i32::from_le_bytes(self.read_array_as::<4>("i32")?))
    }

    /// Reads `count` raw bytes as a slice of the underlying data.
    ///
    /// # Errors
    ///
    /// [`CyberError::Format`] when fewer than `count` bytes remain.
    #[inline]
    pub fn read_bytes(&mut self, count: usize) -> Result<&'data [u8], CyberError> {
        self.take_bytes(count, "bytes")
    }

    /// Reads `N` raw bytes as an array.
    ///
    /// # Errors
    ///
    /// [`CyberError::Format`] when fewer than `N` bytes remain.
    #[inline]
    pub fn read_array<const N: usize>(&mut self) -> Result<[u8; N], CyberError> {
        self.read_array_as::<N>("array")
    }

    /// Reads an eight-byte directory name through [`LumpName::from_raw`].
    ///
    /// # Errors
    ///
    /// [`CyberError::Format`] when fewer than eight bytes remain.
    #[inline]
    pub fn read_name(&mut self) -> Result<LumpName, CyberError> {
        Ok(LumpName::from_raw(self.read_array_as::<NAME_LEN>("name")?))
    }
}

/// Reads the `N` little-endian bytes stored at `offset` in `data`.
fn array_at<const N: usize>(data: &[u8], offset: usize, what: &str) -> Result<[u8; N], CyberError> {
    let Some(end) = offset.checked_add(N) else {
        return Err(truncated(offset, what));
    };
    let Some(bytes) = data.get(offset..end) else {
        return Err(truncated(offset, what));
    };
    let mut out = [0_u8; N];
    out.copy_from_slice(bytes);
    Ok(out)
}

/// Reads the little-endian `u16` at `offset`, without a cursor.
///
/// # Errors
///
/// [`CyberError::Format`] when `data` holds fewer than two bytes at
/// `offset`.
#[inline]
pub fn u16_at(data: &[u8], offset: usize) -> Result<u16, CyberError> {
    Ok(u16::from_le_bytes(array_at(data, offset, "u16")?))
}

/// Reads the little-endian `i16` at `offset`, without a cursor.
///
/// # Errors
///
/// [`CyberError::Format`] when `data` holds fewer than two bytes at
/// `offset`.
#[inline]
pub fn i16_at(data: &[u8], offset: usize) -> Result<i16, CyberError> {
    Ok(i16::from_le_bytes(array_at(data, offset, "i16")?))
}

/// Reads the little-endian `u32` at `offset`, without a cursor.
///
/// # Errors
///
/// [`CyberError::Format`] when `data` holds fewer than four bytes at
/// `offset`.
#[inline]
pub fn u32_at(data: &[u8], offset: usize) -> Result<u32, CyberError> {
    Ok(u32::from_le_bytes(array_at(data, offset, "u32")?))
}

/// Reads the little-endian `i32` at `offset`, without a cursor.
///
/// # Errors
///
/// [`CyberError::Format`] when `data` holds fewer than four bytes at
/// `offset`.
#[inline]
pub fn i32_at(data: &[u8], offset: usize) -> Result<i32, CyberError> {
    Ok(i32::from_le_bytes(array_at(data, offset, "i32")?))
}

/// An append-only little-endian byte sink for building lump payloads.
#[derive(Debug, Default)]
pub struct Writer {
    /// The bytes written so far.
    buf: Vec<u8>,
}

impl Writer {
    /// An empty writer.
    #[must_use]
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty writer with room preallocated for `capacity` bytes.
    #[must_use]
    #[inline]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            buf: Vec::with_capacity(capacity),
        }
    }

    /// Appends one byte.
    #[inline]
    pub fn put_u8(&mut self, value: u8) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    /// Appends one signed byte.
    #[inline]
    pub fn put_i8(&mut self, value: i8) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    /// Appends a two-byte little-endian unsigned value.
    #[inline]
    pub fn put_u16(&mut self, value: u16) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    /// Appends a two-byte little-endian signed value.
    #[inline]
    pub fn put_i16(&mut self, value: i16) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    /// Appends a four-byte little-endian unsigned value.
    #[inline]
    pub fn put_u32(&mut self, value: u32) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    /// Appends a four-byte little-endian signed value.
    #[inline]
    pub fn put_i32(&mut self, value: i32) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    /// Appends raw bytes.
    #[inline]
    pub fn put_bytes(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Appends a lump name as its eight raw directory bytes.
    #[inline]
    pub fn put_name(&mut self, name: LumpName) {
        self.buf.extend_from_slice(&name.raw());
    }

    /// Appends `text` as its bytes, padded with `NUL` to exactly `len`
    /// bytes. WAD fields hold `ASCII`; multi-byte input contributes its
    /// whole `UTF-8` encoding to the length check, so validate names with
    /// [`LumpName::parse`] before writing them.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] when `text` is longer than `len`
    /// bytes; nothing is appended in that case.
    #[inline]
    pub fn put_padded_str(&mut self, text: &str, len: usize) -> Result<(), CyberError> {
        let bytes = text.as_bytes();
        let count = bytes.len();
        let Some(pad) = len.checked_sub(count) else {
            return Err(CyberError::InvalidArgument(format!(
                "text {text:?} is {count} bytes, longer than the {len}-byte field; shorten it or widen the field"
            )));
        };
        self.buf.extend_from_slice(bytes);
        for _ in 0_usize..pad {
            self.buf.push(0);
        }
        Ok(())
    }

    /// Bytes written so far.
    #[must_use]
    #[inline]
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// Whether nothing has been written yet.
    #[must_use]
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Consumes the writer and returns the bytes.
    #[must_use]
    #[inline]
    pub fn into_inner(self) -> Vec<u8> {
        self.buf
    }
}

/// The `u32` form of a `usize` count or offset, for writing WAD fields.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] when `value` does not fit in `u32`;
/// `what` names the quantity so the error points at the offending lump
/// or buffer.
#[inline]
pub fn usize_to_u32(value: usize, what: &str) -> Result<u32, CyberError> {
    u32::try_from(value).map_err(|error| {
        CyberError::LimitExceeded(format!(
            "{what} count {value} does not fit in 32 bits: {error}"
        ))
    })
}

/// The `usize` form of a `u32` offset or count, for indexing buffers.
///
/// # Errors
///
/// [`CyberError::Internal`] when `u32` does not fit in `usize` on this
/// platform; on 32-bit and wider targets this cannot fail.
#[inline]
pub fn u32_to_usize(value: u32) -> Result<usize, CyberError> {
    usize::try_from(value).map_err(|error| {
        CyberError::Internal(format!(
            "{value} does not fit in a usize on this target: {error}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::{Reader, Writer, i16_at, i32_at, u16_at, u32_at, u32_to_usize, usize_to_u32};
    use crate::error::CyberError;
    use crate::wad::name::LumpName;

    #[test]
    fn cursor_walks_and_reports_position() {
        let data = [0xAB_u8, 0xCD, 0xEF];
        let mut reader = Reader::new(&data);
        assert_eq!(reader.pos(), 0, "starts at zero");
        assert_eq!(reader.remaining(), 3, "all bytes remain");
        assert_eq!(reader.read_u8().expect("one byte"), 0xAB, "first byte");
        assert_eq!(reader.pos(), 1, "position advanced by one");
        assert_eq!(reader.remaining(), 2, "remaining shrank by one");
        let mut jumped = Reader::at(&data, 2);
        assert_eq!(jumped.pos(), 2, "at positions the cursor");
        assert_eq!(
            jumped.read_u8().expect("one byte"),
            0xEF,
            "at skips the leading bytes"
        );
    }

    #[test]
    fn reads_every_type_at_the_boundaries() {
        let bytes = [0x2A_u8, 0x34, 0x12, 0x78, 0x56, 0x34, 0x12, 0x03];
        let mut reader = Reader::new(&bytes);
        assert_eq!(reader.read_u8().expect("u8"), 0x2A, "u8 at offset 0");
        assert_eq!(
            reader.read_u16().expect("u16"),
            0x1234,
            "u16 little-endian at offset 1"
        );
        assert_eq!(
            reader.read_u32().expect("u32"),
            0x1234_5678,
            "u32 little-endian at offset 3"
        );
        assert_eq!(
            reader.read_u8().expect("last u8"),
            0x03,
            "read exactly to the end"
        );
        assert_eq!(reader.pos(), bytes.len(), "cursor sits at the end");
        assert_eq!(reader.remaining(), 0, "nothing remains");
        let one_more = reader.read_u8().expect_err("one byte past the end fails");
        assert_eq!(one_more.code(), "format_error", "truncated read code");
        assert_eq!(
            one_more.to_string(),
            "unexpected end of data at offset 8 reading u8",
            "truncated read message"
        );
    }

    #[test]
    fn signed_reads_reproduce_negative_values() {
        let bytes = [0x80_u8, 0xFE, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];
        let mut reader = Reader::new(&bytes);
        assert_eq!(reader.read_i8().expect("i8"), -128, "i8 0x80 is -128");
        assert_eq!(reader.read_i16().expect("i16"), -2, "i16 FE FF is -2");
        assert_eq!(
            reader.read_i32().expect("i32"),
            -1_i32,
            "i32 FF FF FF FF is -1"
        );
        assert!(
            reader
                .read_i8()
                .expect_err("data ended")
                .to_string()
                .contains("i8"),
            "i8 label in error"
        );
    }

    #[test]
    fn slice_and_array_reads() {
        let bytes = [0x01_u8, 0x02, 0x03, 0x04, 0x05];
        let mut reader = Reader::new(&bytes);
        let nothing = reader.read_bytes(0).expect("zero bytes");
        assert!(nothing.is_empty(), "a zero-length read succeeds");
        assert_eq!(reader.pos(), 0, "a zero-length read does not move");
        assert_eq!(
            reader.read_bytes(2).expect("two bytes"),
            [0x01, 0x02],
            "raw slice"
        );
        assert_eq!(
            reader.read_array::<3>().expect("three bytes"),
            [0x03, 0x04, 0x05],
            "raw array"
        );
        let over = reader
            .read_bytes(1)
            .expect_err("one byte past the end fails");
        assert_eq!(over.code(), "format_error", "slice read code");
        assert_eq!(
            over.to_string(),
            "unexpected end of data at offset 5 reading bytes",
            "slice read message"
        );
        let array = Reader::new(&bytes)
            .read_array::<6>()
            .expect_err("six of five fails");
        assert_eq!(
            array.to_string(),
            "unexpected end of data at offset 0 reading array",
            "array read message"
        );
    }

    #[test]
    fn name_read_goes_through_from_raw() {
        let mut reader = Reader::new(b"E1M1\0\0\0\0TEXT");
        assert_eq!(
            reader.read_name().expect("eight name bytes"),
            LumpName::parse("E1M1").expect("valid name"),
            "name value"
        );
        assert_eq!(reader.pos(), 8, "a name is eight bytes wide");
        let short = Reader::new(b"short")
            .read_name()
            .expect_err("five bytes hold no name");
        assert_eq!(
            short.to_string(),
            "unexpected end of data at offset 0 reading name",
            "name read message"
        );
    }

    #[test]
    fn seek_moves_within_and_refuses_beyond_the_data() {
        let bytes = [0x00_u8, 0x01];
        let mut reader = Reader::new(&bytes);
        reader.seek(1).expect("offset 1 is inside two bytes");
        assert_eq!(reader.pos(), 1, "seek moved the cursor");
        reader.seek(2).expect("the end itself is a valid position");
        assert!(
            reader
                .read_u8()
                .expect_err("nothing remains")
                .to_string()
                .contains("at offset 2 reading u8"),
            "read at the end"
        );
        let CyberError::Format(message) = reader.seek(3).expect_err("offset 3 is past two bytes")
        else {
            panic!("expected Format for a beyond-end seek");
        };
        assert!(
            message.contains("cannot seek to offset 3"),
            "seek message: {message}"
        );
        assert_eq!(reader.pos(), 2, "a failed seek keeps the old position");
        let mut wild = Reader::at(&bytes, usize::MAX);
        assert_eq!(wild.remaining(), 0, "remaining saturates at a wild offset");
        let overflow = wild.read_u8().expect_err("offset overflow fails");
        assert_eq!(
            overflow.to_string(),
            format!("unexpected end of data at offset {} reading u8", usize::MAX),
            "checked addition reports the start offset, not a wrap"
        );
    }

    #[test]
    fn offset_reads_match_the_cursor_and_report_truncation() {
        let bytes = [0x34_u8, 0x12, 0x78, 0x56, 0x34, 0x12, 0xFE, 0xFF];
        assert_eq!(u16_at(&bytes, 0).expect("u16 at 0"), 0x1234, "u16_at value");
        assert_eq!(
            i16_at(&bytes, 6).expect("i16 at 6"),
            -2,
            "i16_at negative value"
        );
        assert_eq!(
            u32_at(&bytes, 2).expect("u32 at 2"),
            0x1234_5678,
            "u32_at value"
        );
        assert_eq!(
            i32_at(&bytes, 2).expect("i32 at 2"),
            0x1234_5678_i32,
            "i32_at positive value"
        );
    }

    #[test]
    fn offset_reads_fail_with_position_and_type() {
        let bytes = [0x00_u8, 0x01];
        let small = u16_at(&bytes, 1).expect_err("one byte is no u16");
        assert_eq!(small.code(), "format_error", "u16_at code");
        assert_eq!(
            small.to_string(),
            "unexpected end of data at offset 1 reading u16",
            "u16_at message"
        );
        let big = u32_at(&bytes, 0).expect_err("two bytes are no u32");
        assert_eq!(
            big.to_string(),
            "unexpected end of data at offset 0 reading u32",
            "u32_at message"
        );
        let signed = i16_at(&bytes, 2).expect_err("offset at the end holds nothing");
        assert_eq!(
            signed.to_string(),
            "unexpected end of data at offset 2 reading i16",
            "i16_at message at the end"
        );
        let negative = i32_at(&[0xFF_u8; 4], 0).expect("four bytes are an i32");
        assert_eq!(negative, -1_i32, "i32_at negative value");
    }

    #[test]
    fn writer_round_trips_every_type_through_reader() {
        let name = LumpName::parse("sky1").expect("valid name");
        let mut writer = Writer::with_capacity(16);
        assert_eq!(writer.len(), 0, "a fresh writer is empty");
        assert!(writer.is_empty(), "is_empty follows len");
        writer.put_u8(0x80);
        writer.put_i8(-128);
        writer.put_u16(0xABCD);
        writer.put_i16(-2);
        writer.put_u32(0xDEAD_BEEF);
        writer.put_i32(-1);
        writer.put_bytes(b"hi");
        writer.put_name(name);
        assert_eq!(writer.len(), 24, "1+1+2+2+4+4+2+8 bytes");
        let data = writer.into_inner();
        let mut reader = Reader::new(&data);
        assert_eq!(reader.read_u8().expect("u8"), 0x80, "u8 round trip");
        assert_eq!(reader.read_i8().expect("i8"), -128, "i8 round trip");
        assert_eq!(reader.read_u16().expect("u16"), 0xABCD, "u16 round trip");
        assert_eq!(reader.read_i16().expect("i16"), -2, "i16 round trip");
        assert_eq!(
            reader.read_u32().expect("u32"),
            0xDEAD_BEEF,
            "u32 round trip"
        );
        assert_eq!(reader.read_i32().expect("i32"), -1_i32, "i32 round trip");
        assert_eq!(
            reader.read_bytes(2).expect("bytes"),
            [b'h', b'i'],
            "bytes round trip"
        );
        assert_eq!(
            reader.read_name().expect("name"),
            name,
            "name round trip is uppercase"
        );
        assert_eq!(reader.pos(), data.len(), "everything was consumed");
    }

    #[test]
    fn padded_str_fits_pads_and_rejects_overflow() {
        let mut exact = Writer::new();
        exact
            .put_padded_str("E1M1", 4)
            .expect("exactly four bytes fit");
        assert_eq!(exact.into_inner(), *b"E1M1", "no padding at full width");
        let mut padded = Writer::new();
        padded
            .put_padded_str("TAIL", 8)
            .expect("four bytes fit in eight");
        assert_eq!(
            padded.into_inner(),
            *b"TAIL\0\0\0\0",
            "NUL padding to width"
        );
        let mut overflow = Writer::new();
        let CyberError::InvalidArgument(message) = overflow
            .put_padded_str("TOOLONG", 4)
            .expect_err("seven bytes do not fit in four")
        else {
            panic!("expected InvalidArgument for an overflowing field");
        };
        assert!(
            message.contains("TOOLONG"),
            "message names the text: {message}"
        );
        assert!(overflow.is_empty(), "a rejected write appends nothing");
    }

    #[test]
    fn index_conversions_guard_their_bounds() {
        assert_eq!(usize_to_u32(0, "lump").expect("zero fits"), 0_u32, "zero");
        assert_eq!(
            usize_to_u32(
                usize::try_from(u32::MAX).expect("u32::MAX fits in usize"),
                "lump"
            )
            .expect("u32::MAX fits"),
            u32::MAX,
            "the top of the range"
        );
        let beyond = usize::try_from(u32::MAX)
            .expect("u32::MAX fits in usize")
            .checked_add(1);
        if let Some(value) = beyond {
            let error = usize_to_u32(value, "lump").expect_err("one past u32::MAX must fail");
            assert_eq!(error.code(), "limit_exceeded", "oversized count code");
            assert!(
                error.to_string().contains("lump"),
                "error names the quantity: {error}"
            );
        }
        assert_eq!(
            u32_to_usize(0).expect("zero converts"),
            0_usize,
            "zero back"
        );
        assert_eq!(
            u32_to_usize(u32::MAX).expect("u32::MAX converts on any realistic target"),
            usize::try_from(u32::MAX).expect("u32::MAX fits in usize"),
            "the top of the range back"
        );
    }
}
