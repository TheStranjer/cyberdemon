//! The `REJECT` lump: the per-pair visibility matrix between sectors.
//!
//! A `REJECT` lump is `sectors × sectors` bits, row-major with the least
//! significant bit first: bit `i * sectors + j` set means sector `j` is
//! never visible from sector `i`. The array's expected size is
//! `ceil(sectors² / 8)` bytes, but engines tolerate wrong sizes and so do
//! the tools: [`Reject`] stores the bytes as they are, [`Reject::size_ok`]
//! reports the mismatch and out-of-range lookups read as "not rejected".
//! Nothing here knows about sessions or MCP.

/// The `REJECT` visibility matrix of one map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reject {
    /// The number of sectors the matrix spans.
    pub sectors: usize,
    /// The raw lump bytes, possibly a wrong size.
    pub bits: Vec<u8>,
}

impl Reject {
    /// Wraps the lump `bits` for a map with `sectors` sectors.
    #[must_use]
    #[inline]
    pub fn new(sectors: usize, bits: &[u8]) -> Self {
        Self {
            sectors,
            bits: bits.to_vec(),
        }
    }

    /// The expected lump size for `sectors` sectors: `sectors²` bits
    /// rounded up to whole bytes, saturating on absurd counts.
    #[must_use]
    #[inline]
    pub const fn expected_bytes(sectors: usize) -> usize {
        match sectors.checked_mul(sectors) {
            Some(bits) => bits.saturating_add(7).saturating_div(8),
            None => usize::MAX,
        }
    }

    /// Whether the stored bytes are exactly [`Reject::expected_bytes`].
    #[must_use]
    #[inline]
    pub fn size_ok(&self) -> bool {
        self.bits.len() == Self::expected_bytes(self.sectors)
    }

    /// Whether sector `other` is never visible from sector `from`.
    /// Out-of-range pairs read as visible.
    #[must_use]
    #[inline]
    pub fn is_rejected(&self, from: usize, other: usize) -> bool {
        if from >= self.sectors || other >= self.sectors {
            return false;
        }
        let Some(bit) = from
            .checked_mul(self.sectors)
            .and_then(|row| row.checked_add(other))
        else {
            return false;
        };
        self.bit_at(bit)
    }

    /// How many set bits the matrix holds within its `sectors²` logical
    /// bits (bytes beyond the logical size never count).
    #[must_use]
    #[inline]
    pub fn rejected_pairs(&self) -> usize {
        let total_bits = self.logical_bits();
        let full_bytes = total_bits.checked_div(8).unwrap_or(usize::MAX);
        let mut count: usize = self
            .bits
            .iter()
            .take(full_bytes)
            .map(|byte| usize::try_from(byte.count_ones()).unwrap_or(0))
            .sum();
        let keep = total_bits.saturating_sub(full_bytes.saturating_mul(8));
        count = count.saturating_add(
            self.bits
                .get(full_bytes)
                .map_or(0, |byte| low_bits(*byte, keep)),
        );
        count
    }

    /// Whether no pair is rejected inside the logical matrix.
    #[must_use]
    #[inline]
    pub fn all_zero(&self) -> bool {
        self.rejected_pairs() == 0_usize
    }

    /// Row `sector` of the matrix as a `'0'`/`'1'` string, one character
    /// per sector, least significant bit of each stored byte first.
    #[must_use]
    #[inline]
    pub fn row(&self, sector: usize) -> String {
        (0..self.sectors)
            .map(|column| self.rejection_marker(sector, column))
            .collect()
    }

    /// `'1'` for a rejected pair, `'0'` for a visible one.
    #[must_use]
    fn rejection_marker(&self, sector: usize, column: usize) -> char {
        if self.is_rejected(sector, column) {
            '1'
        } else {
            '0'
        }
    }

    /// The logical bit count, `sectors²`, saturating.
    #[must_use]
    #[inline]
    pub const fn logical_bits(&self) -> usize {
        self.sectors.saturating_mul(self.sectors)
    }

    /// The flat bit `index`, `false` past the stored bytes.
    fn bit_at(&self, index: usize) -> bool {
        let byte_index = index.checked_div(8).unwrap_or(usize::MAX);
        let shift = u32::try_from(index.checked_rem(8).unwrap_or(8)).unwrap_or(8);
        let mask = 1_u8.checked_shl(shift).unwrap_or(0);
        self.bits
            .get(byte_index)
            .is_some_and(|byte| (byte & mask) != 0_u8)
    }
}

/// The number of set bits among the `keep` least significant bits of
/// `byte` (`keep` below 8 where this is called).
#[must_use]
fn low_bits(byte: u8, keep: usize) -> usize {
    let shift = u32::try_from(keep).unwrap_or(8);
    let boundary = 1_u16.checked_shl(shift).unwrap_or(u16::MAX);
    let masked = u16::from(byte) & boundary.checked_sub(1).unwrap_or(u16::MAX);
    usize::try_from(masked.count_ones()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::Reject;

    #[test]
    fn expected_bytes_rounds_the_bit_matrix_up() {
        assert_eq!(Reject::expected_bytes(0), 0, "no sectors, no bits");
        assert_eq!(Reject::expected_bytes(1), 1, "one bit is one byte");
        assert_eq!(Reject::expected_bytes(2), 1, "four bits");
        assert_eq!(Reject::expected_bytes(3), 2, "nine bits");
        assert_eq!(Reject::expected_bytes(8), 8, "sixty-four bits");
        assert_eq!(Reject::expected_bytes(9), 11, "eighty-one bits");
    }

    #[test]
    fn bits_read_row_major_least_significant_first() {
        // 3 sectors: rows are bits 0..2 (sector 0), 3..5 (sector 1) and
        // 6..8 (sector 2). Byte 0b0000_1001 sets bit 0 -> (0,0) and bit 3
        // -> (1,0); byte 1's bit 0 is bit 8 -> (2,2).
        let reject = Reject::new(3, &[0b0000_1001_u8, 0b0000_0001_u8]);
        assert!(reject.is_rejected(0, 0), "bit 0");
        assert!(!reject.is_rejected(0, 1), "bit 1 clear");
        assert!(reject.is_rejected(1, 0), "bit 3, across the byte edge");
        assert!(reject.is_rejected(2, 2), "bit 8 into the second byte");
        assert!(!reject.is_rejected(2, 0), "bit 6 clear");
        assert_eq!(reject.rejected_pairs(), 3, "three set bits");
        assert!(!reject.all_zero(), "the matrix is not empty");
        assert_eq!(reject.row(0), "100", "row zero");
        assert_eq!(reject.row(1), "100", "row one");
        assert_eq!(reject.row(2), "001", "row two");
    }

    #[test]
    fn out_of_range_lookups_read_as_visible_and_short_arrays_cope() {
        let reject = Reject::new(4, &[0xFF_u8]);
        assert!(!reject.is_rejected(4, 0), "row past the sectors");
        assert!(!reject.is_rejected(0, 4), "column past the sectors");
        assert!(reject.is_rejected(0, 0), "first bit set");
        assert!(reject.is_rejected(0, 3), "the whole first row is set");
        assert!(
            !reject.is_rejected(2, 3),
            "bit 11 is beyond the one stored byte"
        );
        assert!(!reject.size_ok(), "one byte is not the expected two");
        assert_eq!(reject.rejected_pairs(), 8, "only stored bits count");
    }

    #[test]
    fn oversized_arrays_ignore_bits_past_the_matrix() {
        // 2 sectors: 4 logical bits; the second byte is junk beyond them.
        let reject = Reject::new(2, &[0b0000_0011_u8, 0xFF_u8]);
        assert_eq!(reject.rejected_pairs(), 2, "the junk byte does not count");
        assert!(!reject.size_ok(), "five bytes to two");
        let empty = Reject::new(2, b"");
        assert!(empty.all_zero(), "no bytes, nothing rejected");
        assert_eq!(empty.rejected_pairs(), 0, "and no pairs");
        assert_eq!(empty.row(0), "00", "rows still read, as zeros");
    }
}
