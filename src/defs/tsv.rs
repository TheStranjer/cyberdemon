//! The shared tab-separated table reader and the embedded-table accessor.
//!
//! Every embedded `data/**/*.tsv` file has one shape: a header line naming
//! the columns, then one line per row with exactly that many tab-separated
//! fields, blank and `#` comment lines standing between them freely.
//! [`Table::parse`] checks the header and splits the rows into fields while
//! keeping their 1-based file line numbers; [`Row::columns`] hands one row
//! out as a fixed-width array, and [`Row`]'s typed readers ([`Row::u32`],
//! [`Row::i32`], [`Row::i64`], [`Row::fail`]) report a bad column with the
//! table, line and column named. The definition tables ([`crate::defs`]),
//! the theme tables ([`crate::themes::table`]) and the translation table
//! ([`crate::wad::map::xlat`]) all read through here, so the header check,
//! the tab split, the column-count check and the line-numbered integer
//! parse exist exactly once.
//!
//! [`embedded`] is the matching `LazyLock` policy for the compiled-in
//! files: a table is parsed once on first use, and a parse failure — a
//! build bug, never a client problem — is kept as an error string and
//! answered as [`CyberError::Internal`] by every accessor, never as a
//! silent empty table.

use std::sync::LazyLock;

use crate::error::CyberError;

/// The line prefix [`Table::parse`] skips as a comment.
const COMMENT: char = '#';

/// The character every column sits between.
const SEPARATOR: char = '\t';

/// One parsed tab-separated file: its rows, in file order.
#[derive(Debug)]
pub struct Table<'text> {
    /// The data rows below the checked header.
    rows: Vec<Row<'text>>,
}

/// One data row of a [`Table`], with the line it stands on.
#[derive(Debug)]
pub struct Row<'text> {
    /// The table's name in error text.
    label: &'static str,
    /// The row's 1-based line number in the file.
    number: usize,
    /// The tab-separated fields, in header order.
    fields: Vec<&'text str>,
}

impl<'text> Table<'text> {
    /// Parse `text` as the table `label`: the first non-blank,
    /// non-comment line must name exactly `header`'s columns,
    /// tab-separated, and every later non-blank, non-comment line becomes
    /// a [`Row`] carrying its file line number. The rows keep the file's
    /// order.
    ///
    /// # Errors
    ///
    /// A `String` naming the missing or wrong header line.
    #[inline]
    pub fn parse(label: &'static str, header: &[&str], text: &'text str) -> Result<Self, String> {
        let mut content = text
            .lines()
            .enumerate()
            .filter(|entry| !entry.1.is_empty() && !entry.1.starts_with(COMMENT));
        let Some((_header_offset, first)) = content.next() else {
            return Err(format!("{label} is empty: no header line"));
        };
        check_header(label, header, first)?;
        // The enumerate offset is the file's 0-based line, so adding one
        // gives the row's 1-based line number even around blank lines.
        let rows = content
            .map(|(offset, line)| Row {
                label,
                number: offset.saturating_add(1),
                fields: line.split(SEPARATOR).collect(),
            })
            .collect();
        Ok(Self { rows })
    }

    /// The parsed rows, in file order.
    #[must_use]
    #[inline]
    pub fn rows(&self) -> &[Row<'text>] {
        &self.rows
    }
}

/// Reject a line that is not `expected`'s columns, tab-separated.
///
/// # Errors
///
/// A `String` showing both the found and the expected header.
fn check_header(label: &str, expected: &[&str], line: &str) -> Result<(), String> {
    if line.split(SEPARATOR).eq(expected.iter().copied()) {
        return Ok(());
    }
    let wanted = expected.join("\t");
    Err(format!("bad {label} header {line:?}: expected {wanted:?}"))
}

impl<'text> Row<'text> {
    /// The row's fields as a `[&str; N]`.
    ///
    /// # Errors
    ///
    /// A `String` naming the line and showing the fields when the row
    /// carries a different number of them than `N`.
    #[inline]
    pub fn columns<const N: usize>(&self) -> Result<[&'text str; N], String> {
        let found: [&'text str; N] = self.fields.clone().try_into().map_err(|fields| {
            self.fail(&format!(
                "expected {N} tab-separated fields, found {fields:?}"
            ))
        })?;
        Ok(found)
    }

    /// The row's 1-based file line number, for the caller's own error
    /// text.
    #[must_use]
    #[inline]
    pub const fn number(&self) -> usize {
        self.number
    }

    /// The standard error for this row's table and line: `"{label} line
    /// {number}: "` followed by `detail`.
    #[must_use]
    #[inline]
    pub fn fail(&self, detail: &str) -> String {
        format!("{} line {}: {detail}", self.label, self.number)
    }

    /// `text` as a decimal `u32`; `column` names it in the error.
    ///
    /// # Errors
    ///
    /// A `String` naming the line and column.
    #[inline]
    pub fn u32(&self, column: &str, text: &str) -> Result<u32, String> {
        text.parse::<u32>()
            .map_err(|error| self.fail(&format!("column {column:?} is {text:?}: {error}")))
    }

    /// `text` as a decimal `i32`; `column` names it in the error.
    ///
    /// # Errors
    ///
    /// A `String` naming the line and column.
    #[inline]
    pub fn i32(&self, column: &str, text: &str) -> Result<i32, String> {
        text.parse::<i32>()
            .map_err(|error| self.fail(&format!("column {column:?} is {text:?}: {error}")))
    }

    /// `text` as a decimal `i64`; `column` names it in the error.
    ///
    /// # Errors
    ///
    /// A `String` naming the line and column.
    #[inline]
    pub fn i64(&self, column: &str, text: &str) -> Result<i64, String> {
        text.parse::<i64>()
            .map_err(|error| self.fail(&format!("column {column:?} is {text:?}: {error}")))
    }
}

/// The value behind one embedded table's `LazyLock`, parsed once on first
/// use.
///
/// A parse failure means the compiled-in data is broken — a build bug
/// the accessors report as [`CyberError::Internal`], never a silent empty
/// table. `label` names the table in the message.
///
/// # Errors
///
/// [`CyberError::Internal`] when the embedded table failed to parse.
#[inline]
pub fn embedded<T>(
    parsed: &'static LazyLock<Result<T, String>>,
    label: &str,
) -> Result<&'static T, CyberError> {
    parsed.as_ref().map_err(|reason| {
        CyberError::Internal(format!(
            "embedded {label} is malformed (this is a server bug): {reason}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use std::sync::LazyLock;

    use super::{Table, embedded};
    use crate::defs::dehacked::embedded_mobjs;
    use crate::defs::registry::definitions_for;
    use crate::themes::table::game_themes;
    use crate::wad::game::Game;
    use crate::wad::map::xlat::xlat_rows;

    /// A three-column table for the reader tests.
    const MINI: &str = "a\tb\tc\n1\ttwo\t3.5\n";

    /// A broken embedded cell: the accessor must report it, not an empty
    /// table.
    static POISONED: LazyLock<Result<Vec<u8>, String>> =
        LazyLock::new(|| Err("column \"b\" is \"x\"".to_owned()));

    #[test]
    fn parses_the_header_and_keeps_the_row_order_and_line_numbers() {
        let text = format!("{MINI}\n2\tthree\t4\n");
        let table = Table::parse("mini table", &["a", "b", "c"], &text).expect("the mini table");
        let rows: Vec<usize> = table.rows().iter().map(super::Row::number).collect();
        assert_eq!(rows, vec![2_usize, 4], "blank lines keep the count out");
        let first = table.rows().first().expect("one row per data line");
        let columns: [&str; 3] = first.columns::<3>().expect("three columns");
        assert_eq!(columns, ["1", "two", "3.5"], "raw fields in order");
        assert_eq!(first.u32("a", columns[0]), Ok(1_u32), "typed reads");
    }

    #[test]
    fn skips_a_comment_line_before_the_header_and_between_rows() {
        let text = format!("# built by hand\n{MINI}# a note\n2\tthree\t4\n");
        let table = Table::parse("mini table", &["a", "b", "c"], &text).expect("comments skip");
        assert_eq!(table.rows().len(), 2, "only the data rows stand");
    }

    #[test]
    fn a_missing_header_is_the_empty_table_error() {
        let error = Table::parse("mini table", &["a", "b", "c"], "").expect_err("no header");
        assert!(error.contains("empty"), "the message says so: {error}");
        let blank = Table::parse("mini table", &["a", "b", "c"], "\n\n").expect_err("all blank");
        assert!(
            blank.contains("empty"),
            "blank lines are no header: {blank}"
        );
    }

    #[test]
    fn a_wrong_header_names_both_headers() {
        let error = Table::parse("mini table", &["a", "b"], "a\tc\n").expect_err("columns differ");
        assert!(error.contains("header"), "the header is named: {error}");
        assert!(
            error.contains("expected \"a\\tb\""),
            "wanted shown: {error}"
        );
    }

    #[test]
    fn a_row_of_another_width_shows_the_line_and_fields() {
        let text = "a\tb\tc\n1\ttwo\n";
        let table = Table::parse("mini table", &["a", "b", "c"], text).expect("header stands");
        let error = table
            .rows()
            .first()
            .expect("the short row is a row")
            .columns::<3>()
            .expect_err("two fields are not three");
        assert!(
            error.contains("mini table line 2"),
            "table and line: {error}"
        );
        assert!(error.contains("\"1\", \"two\""), "the fields show: {error}");
    }

    #[test]
    fn the_typed_readers_name_the_bad_column() {
        let table = Table::parse("mini table", &["a", "b", "c"], MINI).expect("the mini table");
        let row = table.rows().first().expect("one row");
        let bad = row.u32("a", "two").expect_err("\"two\" is no number");
        assert!(bad.contains("mini table line 2"), "line named: {bad}");
        assert!(bad.contains("column \"a\""), "column named: {bad}");
        let decimal = row.i32("c", "3.5").expect_err("3.5 is no integer");
        assert!(decimal.contains("column \"c\""), "column named: {decimal}");
        assert_eq!(row.i64("a", "1"), Ok(1_i64), "i64 reads");
        assert_eq!(row.i32("a", "-2"), Ok(-2_i32), "negatives too");
    }

    #[test]
    fn a_broken_embedded_table_is_reported_never_emptied() {
        let error = embedded(&POISONED, "poisoned").expect_err("the cell is poisoned");
        assert_eq!(error.code(), "internal", "a build bug");
        assert!(
            error.message().contains("embedded poisoned is malformed"),
            "the label names it: {}",
            error.message()
        );
        assert!(
            error.message().contains("column \"b\""),
            "the parse failure travels: {}",
            error.message()
        );
    }

    #[test]
    fn every_embedded_definition_table_parses() {
        for game in [
            Game::Doom,
            Game::Doom2,
            Game::Heretic,
            Game::Hexen,
            Game::Strife,
            Game::Unknown,
        ] {
            let defs = definitions_for(game).expect("the embedded tables parse");
            assert!(!defs.things.is_empty(), "{game:?} has thing rows");
            assert!(!defs.specials.is_empty(), "{game:?} has special rows");
            assert!(!defs.sector_types.is_empty(), "{game:?} has sector rows");
        }
    }

    #[test]
    fn every_embedded_theme_table_parses() {
        for game in [
            Game::Doom,
            Game::Doom2,
            Game::Heretic,
            Game::Hexen,
            Game::Strife,
            Game::Unknown,
        ] {
            let themes = game_themes(game).expect("the embedded theme file parses");
            assert!(!themes.themes.is_empty(), "{game:?} has themes");
        }
    }

    #[test]
    fn the_embedded_translation_and_mobj_tables_parse() {
        let rows = xlat_rows().expect("the embedded xlat table parses");
        assert!(!rows.is_empty(), "the xlat table has rows");
        let mobjs = embedded_mobjs().expect("the embedded mobj table parses");
        assert!(!mobjs.is_empty(), "the mobj table has rows");
    }
}
