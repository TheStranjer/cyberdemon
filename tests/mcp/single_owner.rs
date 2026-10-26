//! The single-owner guard for the CD-318 duplication fold: a big-endian byte
//! helper is defined once, in `src/wad/bytes.rs`, and the `MUS` and `MIDI`
//! vocabulary the two music converters share is defined once, in
//! `src/music/consts.rs`. No other Rust file under `src/` may define a
//! constant or function of one of those names, so neither the byte order nor
//! the event type codes, the controller tables and the tempo ever get a
//! second copy that can fall out of step.

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;
    use std::path::{Path, PathBuf};

    /// The one directory name the repository walk never enters (build
    /// products, and any `.hidden` directory, are not this rule's business).
    const SKIPPED_DIRECTORY: &str = "target";

    /// The file the big-endian reads and writes belong to: `src/wad/bytes.rs`.
    const BYTES_FILE: &str = "bytes.rs";

    /// The file the shared `MUS` and `MIDI` vocabulary belongs to:
    /// `src/music/consts.rs`.
    const CONSTS_FILE: &str = "consts.rs";

    /// Every big-endian helper name: the `fn` definitions of these live once,
    /// as methods of `bytes.rs`' `Reader` and `Writer`. The `put_be_*`,
    /// `read_be_*` and `be_*` spellings are the private copies CD-318 folded
    /// into those methods, and stay banned so they cannot come back.
    const ENDIAN_FNS: [&str; 14] = [
        "read_u16_be",
        "read_u32_be",
        "read_i16_be",
        "read_i32_be",
        "put_u16_be",
        "put_u32_be",
        "put_i16_be",
        "put_i32_be",
        "put_be_u16",
        "put_be_u32",
        "read_be_u16",
        "read_be_u32",
        "be_u32",
        "be_i32",
    ];

    /// The line's text past a leading `//` guard and any `pub` or `pub(..)`
    /// visibility, when the line is a definition candidate.
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
    /// visibility and `const`/`unsafe`/`async` qualifiers.
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

    /// Whether `name` is an all-uppercase constant name of more than one
    /// character, so the anonymous `const _` block is not one.
    fn is_const_name(name: &str) -> bool {
        name.len() > 1
            && name
                .chars()
                .all(|letter| letter.is_ascii_uppercase() || letter == '_')
    }

    /// The name of a `const NAME` definition a line carries: a `const` item
    /// past any visibility. A `const fn` is a function, read by
    /// [`defined_fn`], and `let` bindings and uses are not definitions.
    fn defined_const(line: &str) -> Option<&str> {
        let after = definition_text(line)?.strip_prefix("const ")?.trim_start();
        if after.starts_with("fn ") {
            return None;
        }
        let name = after.split([':', '=', ' ', '(']).next()?;
        is_const_name(name).then_some(name)
    }

    /// The name a line defines, either way.
    fn defined_name(line: &str) -> Option<&str> {
        defined_fn(line).or_else(|| defined_const(line))
    }

    /// Every name the vocabulary module owns: each `const`, `const fn` and
    /// `fn` it defines above its own test module, read from the file itself so
    /// the guard follows the tables without a second list to keep in step.
    fn owned_names(consts_path: &Path) -> Vec<String> {
        let Ok(text) = fs::read_to_string(consts_path) else {
            return Vec::new();
        };
        let mut names: Vec<String> = text
            .lines()
            .take_while(|line| !line.trim_start().starts_with("#[cfg(test)]"))
            .filter_map(defined_name)
            .map(str::to_owned)
            .collect();
        names.sort();
        names
    }

    /// One report row: `path:line: the definition there`.
    fn offender(path: &Path, number: usize, line: &str) -> String {
        format!(
            "{}:{}: {}",
            path.display(),
            number.saturating_add(1),
            line.trim()
        )
    }

    /// Whether `path` is one of the two files these names belong to.
    fn owns_names(path: &Path) -> bool {
        path.file_name()
            .is_some_and(|name| name == BYTES_FILE || name == CONSTS_FILE)
    }

    /// Whether `name` duplicates an owned one: a banned byte helper or a
    /// name the vocabulary module holds.
    fn is_duplicate(name: &str, vocabulary: &[String]) -> bool {
        ENDIAN_FNS.contains(&name) || vocabulary.iter().any(|held| held == name)
    }

    /// The row for one line that defines a name the file may not hold.
    fn duplicate_row(
        path: &Path,
        number: usize,
        line: &str,
        vocabulary: &[String],
    ) -> Option<String> {
        let name = defined_name(line)?;
        is_duplicate(name, vocabulary).then(|| offender(path, number, line))
    }

    /// Appends every second copy of an owned name in one `*.rs` file.
    fn check_file(path: &Path, vocabulary: &[String], offenders: &mut Vec<String>) {
        if path.is_dir() || path.extension() != Some(OsStr::new("rs")) || owns_names(path) {
            return;
        }
        let Ok(text) = fs::read_to_string(path) else {
            return;
        };
        let rows = text
            .lines()
            .enumerate()
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

    /// The names `src/music/consts.rs` owns.
    fn vocabulary() -> Vec<String> {
        owned_names(&src_dir().join("music").join("consts.rs"))
    }

    #[test]
    fn the_byte_helpers_and_music_tables_are_defined_once() {
        let owned = vocabulary();
        assert!(
            owned.len() > 40,
            "the vocabulary read from consts.rs is implausibly short: {} names",
            owned.len()
        );
        let mut offenders: Vec<String> = Vec::new();
        collect_offenders(&src_dir(), &owned, &mut offenders);
        offenders.sort();
        assert!(
            offenders.is_empty(),
            "big-endian byte helpers live in src/wad/bytes.rs and the MUS and \
             MIDI vocabulary in src/music/consts.rs;\nthese files carry a \
             second copy:\n{}",
            offenders.join("\n"),
        );
    }

    #[test]
    fn the_vocabulary_names_the_tables_the_converters_share() {
        let owned = vocabulary();
        for name in [
            "MUS_MAGIC",
            "MUS_HEADER_LEN",
            "EVENT_NOTE",
            "MUS_PERCUSSION_CHANNEL",
            "MIDI_PERCUSSION_CHANNEL",
            "MUS_TICKS_PER_SECOND",
            "MIDI_META_TEMPO",
            "SMF_HEADER_ID",
            "midi_control",
            "mus_control",
            "system_control",
            "mus_system",
        ] {
            assert!(
                owned.iter().any(|held| held == name),
                "consts.rs no longer owns {name}: {owned:?}"
            );
        }
    }

    /// Writes one file into the walk fixture.
    fn write(dir: &Path, name: &str, text: &str) {
        fs::write(dir.join(name), text).unwrap_or_else(|error| panic!("write {name}: {error}"));
    }

    #[test]
    fn the_walk_reports_second_copies_and_skips_the_rest() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path();
        write(
            root,
            "copy.rs",
            "pub(crate) const MUS_MAGIC: [u8; 4] = *b\"MUS\\x1A\";\n",
        );
        write(
            root,
            "helper.rs",
            "fn put_be_u32(out: &mut Vec<u8>, value: u32) {\n    out.push(0_u8);\n}\n",
        );
        write(
            root,
            "table.rs",
            "pub(crate) const fn midi_control(number: u8) -> Option<u8> {\n    None\n}\n",
        );
        write(
            root,
            "commented.rs",
            "// const MUS_MAGIC: [u8; 4] = *b\"MUS\\x1A\";\n// fn be_u32(value: u32) {}\nfn other() {}\n",
        );
        write(
            root,
            "unrelated.rs",
            "const OTHER_TABLE: [u8; 2] = [1_u8, 2];\nfn unrelated_work() {}\nlet _ = MUS_MAGIC;\n",
        );
        write(
            root,
            CONSTS_FILE,
            "pub(crate) const MUS_MAGIC: [u8; 4] = *b\"MUS\\x1A\";\n",
        );
        fs::create_dir_all(root.join(SKIPPED_DIRECTORY)).expect("make the skipped directory");
        write(
            &root.join(SKIPPED_DIRECTORY),
            "generated.rs",
            "const MUS_MAGIC: u8 = 0_u8;\n",
        );
        let owned = vec!["MUS_MAGIC".to_owned(), "midi_control".to_owned()];
        let mut offenders: Vec<String> = Vec::new();
        collect_offenders(root, &owned, &mut offenders);
        let rows: String = offenders.join("\n");
        assert!(
            rows.contains("copy.rs:1")
                && rows.contains("helper.rs:1")
                && rows.contains("table.rs:1"),
            "every second copy is reported: {rows}"
        );
        assert!(
            !rows.contains("commented.rs")
                && !rows.contains("unrelated.rs")
                && !rows.contains(CONSTS_FILE)
                && !rows.contains(SKIPPED_DIRECTORY),
            "comments, unrelated names, the owner file and the build tree pass: {rows}"
        );
    }

    #[test]
    fn a_definition_is_a_const_or_fn_line() {
        assert_eq!(
            defined_fn("pub(crate) fn mus_system(control: u8) -> Option<u8>"),
            Some("mus_system")
        );
        assert_eq!(
            defined_const("pub(crate) const MUS_MAGIC: [u8; 4] = *b\"MUS\\x1A\";"),
            Some("MUS_MAGIC")
        );
        assert_eq!(
            defined_const("const _: () = assert!(true, \"why\");"),
            None,
            "an anonymous const block is not a named definition"
        );
        assert_eq!(
            defined_const("pub(crate) const fn midi_control(number: u8) -> Option<u8> {"),
            None,
            "a const fn is read by the function side of the walk"
        );
        assert_eq!(
            defined_name("    let value = MUS_MAGIC;"),
            None,
            "a use is not a definition"
        );
        assert_eq!(
            defined_name("pub(crate) const fn midi_control(n: u8) -> u8 {"),
            Some("midi_control"),
            "the function side reads a const fn"
        );
        assert_eq!(defined_fn("fn other() {}"), Some("other"));
    }
}
