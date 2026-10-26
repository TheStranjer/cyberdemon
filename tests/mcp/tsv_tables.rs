//! The CD-319 duplication fold's guard: the TSV row-reader helpers and the
//! editor category words each have one owner under `src/`.
//!
//! `check_header` is defined once, in `src/defs/tsv.rs` — the shared
//! reader every embedded table parses through — and `fn rows_of` and
//! `fn parse_number` exist nowhere at all, so the per-file header checks
//! and line-numbered integer parses folded into that reader cannot come
//! back. The category path words (`"Monsters"`, `"Keys"`, `"Doors"`, ...)
//! stand once, in `src/defs/table.rs`' `Category::parse` table: no other
//! production code may match a category string. Test code — sidecar
//! `tests.rs` files, `tests/` directories and everything below an inline
//! `#[cfg(test)]` — and comments are exempt; only the reflex the server
//! runs is held to the rule.

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;
    use std::path::{Path, PathBuf};

    /// The prefix every skipped walk directory answers with (`target/`
    /// build products and any `.hidden` directory).
    const SKIPPED_DIRECTORY: &str = "target";

    /// The file the shared TSV row-reader helper belongs to:
    /// `src/defs/tsv.rs`.
    const TSV_FILE: &str = "tsv.rs";

    /// The file the category classification belongs to:
    /// `src/defs/table.rs`.
    const CATEGORY_FILE: &str = "table.rs";

    /// The category-owner directory: the file must sit in `defs/`.
    const DEFS_DIR: &str = "defs";

    /// The row-reader helper names folded into `src/defs/tsv.rs`: a `fn`
    /// definition of these belongs there (`check_header`) or nowhere at
    /// all (`rows_of` and `parse_number`, the per-table copies CD-319
    /// folded away).
    const SHARED_HELPER_FNS: [&str; 3] = ["check_header", "rows_of", "parse_number"];

    /// The editor category path words `Category::parse` owns. Production
    /// code outside its owner may not carry any of them as a string, so
    /// no category decision can regress to text matching.
    const CATEGORY_WORDS: [&str; 9] = [
        "\"Monsters\"",
        "\"Player Starts\"",
        "\"Weapons\"",
        "\"Ammunition\"",
        "\"Health\"",
        "\"Health And Armour\"",
        "\"Powerups\"",
        "\"Keys\"",
        "\"Doors\"",
    ];

    /// One report row: `path:line: the offending text`.
    fn offender(path: &Path, number: usize, line: &str) -> String {
        format!(
            "{}:{}: {}",
            path.display(),
            number.saturating_add(1),
            line.trim()
        )
    }

    /// Whether `path` is test code the walk never reads: a sidecar
    /// `tests.rs` or a file below a `tests` directory.
    fn is_test_path(path: &Path) -> bool {
        path.file_name() == Some(OsStr::new("tests.rs"))
            || path
                .ancestors()
                .skip(1)
                .any(|dir| dir.file_name() == Some(OsStr::new("tests")))
    }

    /// Whether the file is the category owner, `src/defs/table.rs`.
    fn owns_categories(path: &Path) -> bool {
        path.file_name() == Some(OsStr::new(CATEGORY_FILE))
            && path
                .parent()
                .is_some_and(|parent| parent.file_name() == Some(OsStr::new(DEFS_DIR)))
    }

    /// Whether the file is the shared TSV reader, `src/defs/tsv.rs`.
    fn owns_helpers(path: &Path) -> bool {
        path.file_name() == Some(OsStr::new(TSV_FILE))
            && path
                .parent()
                .is_some_and(|parent| parent.file_name() == Some(OsStr::new(DEFS_DIR)))
    }

    /// The category-word rows in one production, non-comment line.
    fn category_rows(path: &Path, number: usize, line: &str) -> Vec<String> {
        if owns_categories(path) || line.trim_start().starts_with("//") {
            return Vec::new();
        }
        CATEGORY_WORDS
            .iter()
            .filter(|word| line.contains(**word))
            .map(|word| {
                offender(
                    path,
                    number,
                    &format!("{word} matched outside the table reader"),
                )
            })
            .collect()
    }

    /// The row for one line redefining a shared reader helper outside the
    /// reader: `check_header` may stand only in `defs/tsv.rs`, and
    /// `rows_of` and `parse_number` were folded away entirely, so a
    /// definition of either anywhere is a second copy.
    fn helper_row(path: &Path, number: usize, line: &str) -> Option<String> {
        if owns_helpers(path) || line.trim_start().starts_with("//") {
            return None;
        }
        let name = SHARED_HELPER_FNS
            .iter()
            .find(|name| line.contains(&format!("fn {name}(")))?;
        Some(offender(
            path,
            number,
            &format!("redefines the shared reader's fn {name}"),
        ))
    }

    /// The rows of one file's production part: everything above its first
    /// `#[cfg(test)]` or inline `mod tests`, minus the test-path files.
    fn production_lines(text: &str) -> Vec<&str> {
        text.lines()
            .take_while(|line| {
                let trimmed = line.trim_start();
                !trimmed.starts_with("#[cfg(test)]") && !trimmed.starts_with("mod tests")
            })
            .collect()
    }

    /// Appends every offender of one file.
    fn check_file(path: &Path, offenders: &mut Vec<String>) {
        if path.is_dir() || path.extension() != Some(OsStr::new("rs")) || is_test_path(path) {
            return;
        }
        let Ok(text) = fs::read_to_string(path) else {
            return;
        };
        for (number, line) in production_lines(&text).into_iter().enumerate() {
            offenders.extend(category_rows(path, number, line));
            offenders.extend(helper_row(path, number, line));
        }
    }

    /// Whether one directory entry's name is walked.
    fn is_walked_dir(name: &OsStr) -> bool {
        let text = name.to_string_lossy();
        text != SKIPPED_DIRECTORY && !text.starts_with('.')
    }

    /// Recurses into one entry when it is a directory the walk keeps.
    fn follow_dir(entry: &fs::DirEntry, offenders: &mut Vec<String>) {
        if entry.path().is_dir() && is_walked_dir(&entry.file_name()) {
            collect_offenders(&entry.path(), offenders);
        }
    }

    /// Appends every offender under `dir`, recursing into kept directories.
    fn collect_offenders(dir: &Path, offenders: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            follow_dir(&entry, offenders);
            check_file(&entry.path(), offenders);
        }
    }

    /// The repository's `src/` tree.
    fn src_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
    }

    #[test]
    fn the_tsv_helpers_and_category_words_have_one_owner() {
        let mut offenders: Vec<String> = Vec::new();
        collect_offenders(&src_dir(), &mut offenders);
        offenders.sort();
        assert!(
            offenders.is_empty(),
            "the TSV row reader lives in src/defs/tsv.rs and the category \
             words in src/defs/table.rs's Category::parse;\nthese production \
             files carry a second copy:\n{}",
            offenders.join("\n"),
        );
    }

    #[test]
    fn the_owners_still_hold_their_vocabulary() {
        let table = fs::read_to_string(src_dir().join(DEFS_DIR).join(CATEGORY_FILE))
            .expect("read the category owner");
        for word in CATEGORY_WORDS {
            assert!(table.contains(word), "Category lost {word}: {table}");
        }
        let reader = fs::read_to_string(src_dir().join(DEFS_DIR).join(TSV_FILE))
            .expect("read the shared TSV reader");
        assert!(
            reader.contains("fn check_header("),
            "the reader no longer owns the header check"
        );
    }

    /// Writes one file at `path`, making its parent directory.
    fn write(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("make the file's directory");
        }
        fs::write(path, text).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
    }

    #[test]
    fn the_walk_reports_second_copies_and_skips_the_rest() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path();
        write(
            &root.join("copier.rs"),
            "fn door_specials(rows: &[u8]) -> usize {\n    rows.len()\n}\n",
        );
        write(
            &root.join("plan").join("matcher.rs"),
            "const KEYS: &str = \"Keys\";\n",
        );
        write(
            &root.join("commented.rs"),
            "// const MON: &str = \"Monsters\";\n/// e.g. `\"Doors\"` prefixes\nfn other() {}\n",
        );
        write(
            &root.join("tested.rs"),
            "fn works() {}\n#[cfg(test)]\nmod tests {\n    fn check_header() {}\n    const M: &str = \"Monsters\";\n}\n",
        );
        write(&root.join("tests.rs"), "const W: &str = \"Weapons\";\n");
        write(
            &root.join("tests").join("cases.rs"),
            "const K: &str = \"Keys\";\n",
        );
        write(
            &root.join(DEFS_DIR).join(CATEGORY_FILE),
            "const PREFIXES: [(&str, u8); 1] = [(\"Monsters\", 0)];\n",
        );
        write(
            &root.join(DEFS_DIR).join(TSV_FILE),
            "fn check_header(line: &str) -> bool {\n    line.is_empty()\n}\n",
        );
        write(
            &root.join(SKIPPED_DIRECTORY).join("generated.rs"),
            "const H: &str = \"Health\";\n",
        );
        let mut offenders: Vec<String> = Vec::new();
        collect_offenders(root, &mut offenders);
        let rows: String = offenders.join("\n");
        assert_eq!(
            offenders.len(),
            1,
            "only the second category copy offends: {rows}"
        );
        assert!(
            rows.contains("matcher.rs:1"),
            "the offender is the nested copy: {rows}"
        );
    }

    #[test]
    fn banned_helper_fns_are_reported_anywhere_but_the_reader() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path();
        write(
            &root.join("tablecopy.rs"),
            "fn parse_number(column: &str) -> u32 {\n    0_u32\n}\n",
        );
        write(&root.join("lines.rs"), "fn rows_of(text: &str) {}\n");
        let mut offenders: Vec<String> = Vec::new();
        collect_offenders(root, &mut offenders);
        let rows: String = offenders.join("\n");
        assert!(
            rows.contains("tablecopy.rs:1") && rows.contains("lines.rs:1"),
            "redefined reader helpers are reported: {rows}"
        );
    }
}
