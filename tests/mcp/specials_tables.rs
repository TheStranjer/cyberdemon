//! The CD-292 duplication fold's guard: the family-aware line-special
//! tables and their classification have one owner under `src/` —
//! `src/wad/map/specials.rs` and its `specials/tables.rs`. The `Doom` and
//! `Hexen` action rows, the exit tables, the trigger read and the door
//! question every texture pass asks are defined there once, so no door
//! detection can grow a second, game-flavoured copy of the numbers.
//! `DOOM_WALK_EXITS` — the describe module's old four-number echo of the
//! `Doom` exit table's walk rows — is banned outright, and `door_specials`
//! (the door builder's style-to-action write table) may only be defined by
//! `src/wad/map/features/doors.rs`, its one writer. Test code — sidecar
//! `tests.rs` files, `tests/` directories and everything below an inline
//! `#[cfg(test)]` — and comments are exempt, exactly as the other
//! single-owner guards walk it.

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;
    use std::path::{Path, PathBuf};

    /// The prefix every skipped walk directory answers with (`target/`
    /// build products and any `.hidden` directory).
    const SKIPPED_DIRECTORY: &str = "target";

    /// The classification root: the file the entry-point functions belong
    /// to, `src/wad/map/specials.rs`, under the `map` directory.
    const SPECIALS_FILE: &str = "specials.rs";

    /// The tables file, `tables.rs`, under the `specials` directory.
    const TABLES_FILE: &str = "tables.rs";

    /// The directory the tables file sits in.
    const SPECIALS_DIR: &str = "specials";

    /// The directory the classification root sits in.
    const MAP_DIR: &str = "map";

    /// The door builder's file, the one writer allowed to define
    /// `door_specials`, under the `features` directory.
    const DOORS_FILE: &str = "doors.rs";

    /// The directory the door writer sits in.
    const FEATURES_DIR: &str = "features";

    /// Names that must stand nowhere: the describe module's duplicate of
    /// the `Doom` family's walk-over exits, folded into the shared
    /// classification by CD-292.
    const BANNED_NAMES: [&str; 1] = ["DOOM_WALK_EXITS"];

    /// The writer-only name: the door builder's style-to-action table,
    /// banned outside [`DOORS_FILE`] (the tools' old game-table copy of
    /// CD-292 folded into the shared classification).
    const WRITER_ONLY: &str = "door_specials";

    /// One report row: `path:line: the definition there`.
    fn offender(path: &Path, number: usize, line: &str) -> String {
        format!(
            "{}:{}: {}",
            path.display(),
            number.saturating_add(1),
            line.trim()
        )
    }

    /// The line's text past a leading `//` guard and any `pub` or
    /// `pub(..)` visibility, when the line is a definition candidate.
    fn definition_text(line: &str) -> Option<&str> {
        let text = line.trim_start();
        if text.starts_with("//") {
            return None;
        }
        let rest = text.strip_prefix("pub").unwrap_or(text);
        let unprefixed = match rest
            .strip_prefix('(')
            .and_then(|inner| inner.split_once(')'))
        {
            Some((_path, tail)) => tail,
            None => rest,
        };
        Some(unprefixed.trim_start())
    }

    /// The name of a `fn NAME(` definition a line carries, past any
    /// `const`/`unsafe`/`async`/`extern` qualifier.
    fn defined_fn(line: &str) -> Option<&str> {
        let mut words = definition_text(line)?;
        for qualifier in ["const ", "unsafe ", "async ", "extern "] {
            words = words.strip_prefix(qualifier).unwrap_or(words);
        }
        let body = words.strip_prefix("fn ")?;
        let name_end = body.find('(')?;
        let name = body.get(..name_end)?;
        (!name.is_empty()).then_some(name)
    }

    /// The name of a `const NAME` definition a line carries: an
    /// all-uppercase constant past any visibility.
    fn defined_const(line: &str) -> Option<&str> {
        let after = definition_text(line)?.strip_prefix("const ")?.trim_start();
        if after.starts_with("fn ") {
            return None;
        }
        let name = after.split([':', '=', ' ', '(']).next()?;
        (name.len() > 1
            && name
                .chars()
                .all(|letter| letter.is_ascii_uppercase() || letter == '_'))
        .then_some(name)
    }

    /// The name a line defines, either way.
    fn defined_name(line: &str) -> Option<&str> {
        defined_fn(line).or_else(|| defined_const(line))
    }

    /// The production part of one file as `(line index, text)` pairs:
    /// everything above its first test module, whether the inline
    /// `mod tests {` form or a `#[cfg(test)]` attribute atop a sidecar
    /// declaration.
    fn production_lines(text: &str) -> Vec<(usize, &str)> {
        text.lines()
            .enumerate()
            .take_while(|&(_number, line)| {
                let trimmed = line.trim_start();
                !trimmed.starts_with("#[cfg(test)]") && !trimmed.starts_with("mod tests")
            })
            .collect()
    }

    /// The names the two owner files hold, read from the files themselves
    /// so the guard follows the tables without a second list to keep in
    /// step.
    fn owned_names(wad_dir: &Path) -> Vec<String> {
        [
            wad_dir.join(MAP_DIR).join(SPECIALS_FILE),
            wad_dir.join(MAP_DIR).join(SPECIALS_DIR).join(TABLES_FILE),
        ]
        .iter()
        .filter_map(|path| fs::read_to_string(path).ok())
        .flat_map(|text| {
            production_lines(&text)
                .into_iter()
                .filter_map(|(_number, line)| defined_name(line))
                .map(str::to_owned)
                .collect::<Vec<String>>()
        })
        .collect()
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

    /// Whether `path` is one of the classification's own files.
    fn owns_names(path: &Path) -> bool {
        let parent = path.parent();
        match path.file_name().and_then(OsStr::to_str) {
            Some(TABLES_FILE) => {
                parent.is_some_and(|dir| dir.file_name() == Some(OsStr::new(SPECIALS_DIR)))
            }
            Some(SPECIALS_FILE) => {
                parent.is_some_and(|dir| dir.file_name() == Some(OsStr::new(MAP_DIR)))
            }
            _ => false,
        }
    }

    /// Whether a definition line inside an owner file still owes the walk
    /// a look: the outright-banned names and the writer-only one.
    fn checked_inside_owner(line: &str) -> bool {
        defined_name(line).is_some_and(|name| BANNED_NAMES.contains(&name) || name == WRITER_ONLY)
    }

    /// Whether `path` is the door writer allowed to define `door_specials`.
    fn is_door_writer(path: &Path) -> bool {
        path.file_name() == Some(OsStr::new(DOORS_FILE))
            && path
                .parent()
                .is_some_and(|dir| dir.file_name() == Some(OsStr::new(FEATURES_DIR)))
    }

    /// The row for one production line that defines a name the file may
    /// not hold.
    fn duplicate_row(
        path: &Path,
        number: usize,
        line: &str,
        vocabulary: &[String],
    ) -> Option<String> {
        let name = defined_name(line)?;
        let duplicated = vocabulary.iter().any(|held| held == name);
        let banned = BANNED_NAMES.contains(&name);
        let writer_rule = name == WRITER_ONLY && !is_door_writer(path);
        (duplicated || banned || writer_rule).then(|| offender(path, number, line))
    }

    /// Appends every second copy of an owned name in one `*.rs` file,
    /// checking only the banned rows inside the owners themselves.
    fn check_file(path: &Path, vocabulary: &[String], offenders: &mut Vec<String>) {
        if path.is_dir() || path.extension() != Some(OsStr::new("rs")) || is_test_path(path) {
            return;
        }
        let Ok(text) = fs::read_to_string(path) else {
            return;
        };
        let owner = owns_names(path);
        let rows = production_lines(&text)
            .into_iter()
            .filter(|&(_number, line)| !owner || checked_inside_owner(line))
            .filter_map(|(number, line)| duplicate_row(path, number, line, vocabulary));
        offenders.extend(rows);
    }

    /// Whether one directory entry's name is walked: `target/` and every
    /// hidden directory are not.
    fn is_walked_dir(name: &OsStr) -> bool {
        let text = name.to_string_lossy();
        text != SKIPPED_DIRECTORY && !text.starts_with('.')
    }

    /// Recurses into one entry when it is a directory the walk keeps.
    fn follow_dir(entry: &fs::DirEntry, vocabulary: &[String], offenders: &mut Vec<String>) {
        if entry.path().is_dir() && is_walked_dir(&entry.file_name()) {
            collect_offenders(&entry.path(), vocabulary, offenders);
        }
    }

    /// Appends every duplicate definition under `dir`, recursing into the
    /// directories the walk keeps.
    fn collect_offenders(dir: &Path, vocabulary: &[String], offenders: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            follow_dir(&entry, vocabulary, offenders);
            check_file(&entry.path(), vocabulary, offenders);
        }
    }

    /// The repository's `src/` tree.
    fn src_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
    }

    /// The names the classification owns.
    fn vocabulary() -> Vec<String> {
        owned_names(&src_dir().join("wad"))
    }

    #[test]
    fn the_special_tables_and_classification_are_defined_once() {
        let owned = vocabulary();
        assert!(
            owned.len() > 15,
            "the vocabulary read from the specials files is implausibly short: {owned:?}"
        );
        let mut offenders: Vec<String> = Vec::new();
        collect_offenders(&src_dir(), &owned, &mut offenders);
        offenders.sort();
        assert!(
            offenders.is_empty(),
            "the line-special tables and their family-aware classification live in \
             src/wad/map/specials.rs and src/wad/map/specials/tables.rs, and door_specials \
             in src/wad/map/features/doors.rs;\nthese files carry a second copy:\n{}",
            offenders.join("\n"),
        );
    }

    #[test]
    fn the_owners_still_hold_the_classifications_vocabulary() {
        let owned = vocabulary();
        for name in [
            "DOOM_ROWS",
            "DOOM_EXITS",
            "HEXEN_ROWS",
            "HEXEN_EXITS",
            "KEY_THINGS",
            "special_class",
            "is_door_action",
            "exit_for",
            "hexen_trigger",
            "lock_color",
        ] {
            assert!(
                owned.iter().any(|held| held == name),
                "the classification no longer owns {name}: {owned:?}"
            );
        }
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
        let map = root.join(MAP_DIR);
        write(
            &root.join("copier.rs"),
            "const DOOM_ROWS: [u8; 1] = [0_u8];\nfn is_door_action(special: i32) -> bool {\n    true\n}\n",
        );
        write(
            &root.join("banned.rs"),
            "const DOOM_WALK_EXITS: [i32; 4] = [52_i32, 124, 197, 198];\n",
        );
        write(
            &root.join(FEATURES_DIR).join(DOORS_FILE),
            "pub const fn door_specials(special: i32) -> i32 {\n    special\n}\n",
        );
        write(
            &root.join("elsewhere.rs"),
            "fn door_specials(special: i32) -> i32 {\n    special\n}\n",
        );
        write(
            &root.join("commented.rs"),
            "// const DOOM_ROWS: [u8; 1] = [];\nfn other_work() {}\n",
        );
        write(
            &map.join(SPECIALS_FILE),
            "pub(crate) fn is_door_action(special: i32) -> bool {\n    true\n}\n",
        );
        write(
            &map.join(SPECIALS_DIR).join(TABLES_FILE),
            "pub(crate) const DOOM_ROWS: [u8; 1] = [0_u8];\n",
        );
        write(
            &root.join(SKIPPED_DIRECTORY).join("generated.rs"),
            "const DOOM_ROWS: [u8; 1] = [0_u8];\n",
        );
        let owned = vocabulary();
        let mut offenders: Vec<String> = Vec::new();
        collect_offenders(root, &owned, &mut offenders);
        let rows: String = offenders.join("\n");
        assert!(
            rows.contains("copier.rs:1")
                && rows.contains("copier.rs:2")
                && rows.contains("banned.rs:1")
                && rows.contains("elsewhere.rs:1"),
            "every second copy is reported: {rows}"
        );
        assert!(
            !rows.contains("commented.rs")
                && !rows.contains(SKIPPED_DIRECTORY)
                && !rows.contains(DOORS_FILE)
                && !rows.contains("specials.rs")
                && !rows.contains("tables.rs"),
            "comments, the build tree, the door writer and the owner files pass: {rows}"
        );
    }
}
