//! The CD-326 duplication guard: the binary map block passes and the record
//! codecs both formats share live once, in `src/wad/map/common.rs`; the two
//! binary formats keep only their own `THINGS` and `LINEDEFS` codecs, in
//! `src/wad/map/doom.rs` and `src/wad/map/hexen.rs`; and the `WAD` position
//! check, payload placement and directory writing live once, in
//! `src/wad/archive.rs`, whose two serializers differ only by the `Placement`
//! strategy they hand the one planner. No other production file under `src/`
//! may define one of those names, and the folded spellings (`dedup_plan` and
//! its `DedupPlan`) stay banned everywhere, so neither the map lump run nor
//! the archive layout can grow a second copy that falls out of step.

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;
    use std::path::{Path, PathBuf};

    /// The one directory name the repository walk never enters (build
    /// products, and any `.hidden` directory, are not this rule's business).
    const SKIPPED_DIRECTORY: &str = "target";

    /// One set of names, the files that own them and the folded spellings no
    /// file may bring back.
    struct Owner {
        /// The word the report names the set by.
        word: &'static str,
        /// The path suffixes allowed to define the owned names.
        files: &'static [&'static str],
        /// The names only those files define.
        names: &'static [&'static str],
        /// The names no file at all may define again.
        banned: &'static [&'static str],
    }

    /// The whole-block passes and the record codecs both binary map formats
    /// share, which belong to `wad/map/common.rs`.
    const BLOCK: Owner = Owner {
        word: "shared binary map block passes",
        files: &["wad/map/common.rs"],
        names: &[
            "read_binary_map",
            "write_binary_map",
            "BinaryLayout",
            "no_tail",
            "member_data",
            "other_lumps",
            "is_editable",
            "records",
            "read_vertices",
            "write_vertexes",
            "read_sidedefs",
            "write_sidedefs",
            "read_sectors",
            "write_sectors",
        ],
        banned: &[],
    };

    /// The `THINGS` and `LINEDEFS` codecs, which belong to the two binary
    /// formats and nowhere else: `Doom` and `Hexen` each spell their own
    /// record widths, which is the difference the shared block passes take
    /// through a `BinaryLayout`.
    const CODECS: Owner = Owner {
        word: "binary THINGS and LINEDEFS codecs",
        files: &["wad/map/doom.rs", "wad/map/hexen.rs"],
        names: &[
            "read_things",
            "write_things",
            "read_linedefs",
            "write_linedefs",
        ],
        banned: &[],
    };

    /// The archive writer: the position check and the payload-sharing lookup
    /// both serializers run through, which belong to `wad/archive.rs`. Its
    /// banned names are the second writer's plan that CD-326 folded into the
    /// one `Placement`-driven pair.
    const ARCHIVE: Owner = Owner {
        word: "WAD archive writer",
        files: &["wad/archive.rs"],
        names: &["offset_field", "shared_at"],
        banned: &["dedup_plan", "DedupPlan"],
    };

    /// The three ownership rules this guard holds.
    const OWNERS: [Owner; 3] = [BLOCK, CODECS, ARCHIVE];

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

    /// The name an `fn`, `struct`, `enum` or `type` definition line carries,
    /// past any visibility and `const`/`unsafe`/`async`/`extern` qualifier.
    fn defined_name(line: &str) -> Option<&str> {
        let mut words = definition_text(line)?;
        for qualifier in ["const ", "unsafe ", "async ", "extern "] {
            words = words.strip_prefix(qualifier).unwrap_or(words);
        }
        let body = ["fn ", "struct ", "enum ", "type "]
            .iter()
            .find_map(|keyword| words.strip_prefix(keyword))?;
        let end = body
            .find(['(', ':', '=', ' ', '<', '{'])
            .unwrap_or(body.len());
        let name = body.get(..end)?;
        (!name.is_empty()).then_some(name)
    }

    /// The names one file defines above its own test module: a test helper
    /// that borrows a shared word is not a second copy of the production item.
    fn definitions(text: &str) -> Vec<String> {
        text.lines()
            .take_while(|line| !line.trim_start().starts_with("#[cfg(test)]"))
            .filter_map(defined_name)
            .map(str::to_owned)
            .collect()
    }

    /// Whether `path` is one of the files `owner` allows these names in.
    fn is_owner(path: &Path, owner: &Owner) -> bool {
        let name = path.to_string_lossy();
        owner.files.iter().any(|file| name.ends_with(file))
    }

    /// Whether `name` is one of `owner`'s owned names defined outside its
    /// files, or one of its banned names defined anywhere at all.
    fn violates(path: &Path, owner: &Owner, name: &str) -> bool {
        if owner.banned.contains(&name) {
            return true;
        }
        !is_owner(path, owner) && owner.names.contains(&name)
    }

    /// The rows for one file's second copies of `owner`'s names.
    fn violations(path: &Path, owner: &Owner, text: &str) -> Vec<String> {
        definitions(text)
            .iter()
            .filter(|name| violates(path, owner, name))
            .map(|name| format!("{}: {name} ({} here)", path.display(), owner.word))
            .collect()
    }

    /// Whether one directory entry's name is walked: `target/` and every
    /// hidden directory are not.
    fn is_walked_dir(name: &OsStr) -> bool {
        let text = name.to_string_lossy();
        text != SKIPPED_DIRECTORY && !text.starts_with('.')
    }

    /// Appends every second copy in one `*.rs` file.
    fn check_file(path: &Path, offenders: &mut Vec<String>) {
        if path.is_dir() || path.extension() != Some(OsStr::new("rs")) {
            return;
        }
        let Ok(text) = fs::read_to_string(path) else {
            return;
        };
        for owner in &OWNERS {
            offenders.extend(violations(path, owner, &text));
        }
    }

    /// Recurses into one entry when it is a directory the walk keeps.
    fn follow_dir(entry: &fs::DirEntry, offenders: &mut Vec<String>) {
        if entry.path().is_dir() && is_walked_dir(&entry.file_name()) {
            collect_offenders(&entry.path(), offenders);
        }
    }

    /// Appends every second copy of an owned name under `dir`.
    fn collect_offenders(dir: &Path, offenders: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            check_file(&entry.path(), offenders);
            follow_dir(&entry, offenders);
        }
    }

    /// The repository's `src/` tree.
    fn src_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
    }

    /// Whether any of `owner`'s files defines `name` in the real tree, read
    /// from it so the guard cannot pass on a group whose owners have quietly
    /// renamed away.
    fn owner_still_defines(owner: &Owner, name: &str) -> bool {
        let root = src_dir();
        owner.files.iter().any(|file| {
            fs::read_to_string(root.join(file))
                .is_ok_and(|text| definitions(&text).iter().any(|held| held == name))
        })
    }

    /// The rows for the names of one owner no owner file defines any more.
    fn stale_rows(owner: &Owner) -> Vec<String> {
        owner
            .names
            .iter()
            .filter(|name| !owner_still_defines(owner, name))
            .map(|name| format!("{name} is not defined by {}", owner.files.join(" or ")))
            .collect()
    }

    /// The guarded names no owner file defines, as report rows.
    fn stale_lists() -> Vec<String> {
        OWNERS.iter().flat_map(stale_rows).collect()
    }

    #[test]
    fn the_block_passes_codecs_and_archive_writer_are_defined_once() {
        let mut offenders: Vec<String> = Vec::new();
        collect_offenders(&src_dir(), &mut offenders);
        offenders.sort();
        assert!(
            offenders.is_empty(),
            "the shared binary map block passes live in src/wad/map/common.rs, \
             the THINGS and LINEDEFS codecs in src/wad/map/doom.rs and \
             src/wad/map/hexen.rs and the archive writer in \
             src/wad/archive.rs;\nthese files carry a second copy:\n{}",
            offenders.join("\n"),
        );
    }

    #[test]
    fn every_owned_name_is_still_defined_by_its_owner() {
        let stale = stale_lists();
        assert!(
            stale.is_empty(),
            "these guarded names have moved, so this guard's lists are \
             stale:\n{}",
            stale.join("\n"),
        );
    }

    #[test]
    fn the_doom_and_hexen_codecs_keep_their_own_widths_under_one_lump_run() {
        // The one rule the block fold must not blur: the shared passes carry
        // both formats' records, so each format still spells its own widths
        // and only `common.rs` still spells the lump run.
        let read = |file: &str| {
            fs::read_to_string(src_dir().join(file))
                .unwrap_or_else(|error| panic!("read {file}: {error}"))
        };
        let doom = read("wad/map/doom.rs");
        let hexen = read("wad/map/hexen.rs");
        let block = read("wad/map/common.rs");
        assert!(
            doom.contains("const THING_LEN: usize = 10")
                && doom.contains("const LINEDEF_LEN: usize = 14"),
            "Doom keeps its own 10- and 14-byte record widths"
        );
        assert!(
            hexen.contains("const THING_LEN: usize = 20")
                && hexen.contains("const LINEDEF_LEN: usize = 16"),
            "Hexen keeps its own 20- and 16-byte record widths"
        );
        assert!(
            block.contains("lumps::CLASSIC_NODE_RUN_LUMPS")
                && !doom.contains("CLASSIC_NODE_RUN_LUMPS")
                && !hexen.contains("CLASSIC_NODE_RUN_LUMPS"),
            "the vanilla lump run is written once, in the shared block passes"
        );
    }

    /// Writes one file into the walk fixture, making its parent directory.
    fn write(dir: &Path, name: &str, text: &str) {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("make the fixture directory");
        }
        fs::write(&path, text).unwrap_or_else(|error| panic!("write {name}: {error}"));
    }

    /// Writes the files a second copy of each guarded name looks like.
    fn write_copies(root: &Path) {
        write(
            root,
            "elsewhere/block.rs",
            "pub(super) fn write_binary_map(layout: &L, map: &M) -> V {\n    V\n}\n",
        );
        write(
            root,
            "elsewhere/codecs.rs",
            "fn read_things(data: &[u8]) -> R {\n    R\n}\n",
        );
        write(
            root,
            "elsewhere/writer.rs",
            "enum DedupPlan {}\nfn offset_field(p: usize, w: &str) -> U {\n    U\n}\n",
        );
    }

    /// Writes the files the walk must leave alone: comments, a test module,
    /// unrelated names, the three owner files and the build tree.
    fn write_allowed(root: &Path) {
        write(
            root,
            "commented.rs",
            "// fn write_binary_map() {}\nfn other() {}\n",
        );
        write(
            root,
            "tested.rs",
            "#[cfg(test)]\nmod tests {\n    fn records() {}\n}\n",
        );
        write(
            root,
            "unrelated.rs",
            "fn totals() -> usize {\n    0_usize\n}\nstruct Layout;\n",
        );
        write(
            root,
            "wad/map/common.rs",
            "pub(super) fn write_binary_map() {}\nfn records() {}\n",
        );
        write(root, "wad/map/doom.rs", "fn read_things() {}\n");
        write(root, "wad/map/hexen.rs", "fn read_things() {}\n");
        write(root, "wad/archive.rs", "fn offset_field() {}\n");
        fs::create_dir_all(root.join(SKIPPED_DIRECTORY)).expect("make the skipped directory");
        write(
            root,
            &format!("{SKIPPED_DIRECTORY}/generated.rs"),
            "fn read_things() {}\n",
        );
    }

    #[test]
    fn the_walk_reports_second_copies_and_skips_the_rest() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path();
        write_copies(root);
        write_allowed(root);
        let mut offenders: Vec<String> = Vec::new();
        collect_offenders(root, &mut offenders);
        let rows: String = offenders.join("\n");
        assert!(
            rows.contains("block.rs") && rows.contains("codecs.rs") && rows.contains("writer.rs"),
            "every second copy and every banned spelling is reported: {rows}"
        );
        assert!(
            !rows.contains("commented.rs")
                && !rows.contains("tested.rs")
                && !rows.contains("unrelated.rs")
                && !rows.contains("common.rs")
                && !rows.contains("doom.rs")
                && !rows.contains("hexen.rs")
                && !rows.contains("archive.rs")
                && !rows.contains(SKIPPED_DIRECTORY),
            "comments, test modules, the owners and the build tree pass: {rows}"
        );
    }

    #[test]
    fn a_definition_is_an_fn_struct_enum_or_type_line() {
        assert_eq!(
            defined_name("pub(super) const fn no_tail(_: &[Lump]) -> Vec<Lump> {"),
            Some("no_tail")
        );
        assert_eq!(defined_name("struct BinaryLayout {"), Some("BinaryLayout"));
        assert_eq!(defined_name("enum Placement {"), Some("Placement"));
        assert_eq!(
            defined_name("type ThingReader = fn(&[u8]) -> R;"),
            Some("ThingReader")
        );
        assert_eq!(
            defined_name("    pub(crate) fn keeps(self) -> bool {"),
            Some("keeps")
        );
        assert_eq!(
            defined_name("let records = data;"),
            None,
            "a use is not a definition"
        );
        assert_eq!(
            defined_name("// fn records() {}"),
            None,
            "a comment is not a definition"
        );
    }
}
