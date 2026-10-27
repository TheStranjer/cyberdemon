//! The CD-316 duplication guard for the `BSP` node writers: the node record,
//! its bounding-box and child-word encoding, the vertex table they all build
//! and the words their refusals answer with are defined once, in
//! `src/bsp/records.rs`. No other Rust file under `src/bsp/` may define one
//! of those names, and the folded spellings (`write_boxes`, `i16_or`,
//! `child_word`, `field_i16` and the rest) stay banned there, so the vanilla,
//! extended and `GL` writers cannot each grow back their own copy of the
//! record they now share.

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;
    use std::path::{Path, PathBuf};

    /// The tree the guard walks: the `BSP` builder and its three writers.
    const ROOT: &str = "bsp";

    /// The file the shared record machinery belongs to.
    const OWNER: &str = "records.rs";

    /// The names only `bsp/records.rs` may define: the `16.16` shift and the
    /// count display form, the refusal maker and the child-width enum, the
    /// vertex table with the spot it answers, and the record writers.
    const OWNED: [&str; 10] = [
        "scaled",
        "shown",
        "Refusals",
        "ChildWidth",
        "VertexTable",
        "Spot",
        "write_box",
        "write_node",
        "write_node_records",
        "write_rows",
    ];

    /// The names no file under `src/bsp/` may define at all: the per-format
    /// copies CD-316 folded into the owned ones. Each writer keeps its own
    /// lump framing (`write_nodes` over them all), which is why only the
    /// record-level spellings below are banned.
    const BANNED: [&str; 8] = [
        "write_boxes",
        "i16_or",
        "word_of",
        "child_word",
        "child_index",
        "field_i16",
        "field_u16",
        "field_u32",
    ];

    /// The line's text past a leading `//` guard and any `pub` or `pub(..)`
    /// visibility, when the line is a definition candidate.
    fn definition_text(line: &str) -> Option<&str> {
        let text = line.trim_start();
        if text.starts_with("//") {
            return None;
        }
        let rest = text.strip_prefix("pub").unwrap_or(text);
        match rest
            .strip_prefix('(')
            .and_then(|inner| inner.split_once(')'))
        {
            Some((_path, tail)) => Some(tail.trim_start()),
            None => Some(rest),
        }
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

    /// Whether `path` is the owner, `bsp/records.rs`.
    fn is_owner(path: &Path) -> bool {
        path.file_name()
            .is_some_and(|name| name == OsStr::new(OWNER))
    }

    /// Whether `name` breaks the rule at `path`: an owned name outside the
    /// owner, or any banned name anywhere.
    fn violates(path: &Path, name: &str) -> bool {
        if BANNED.contains(&name) {
            return true;
        }
        !is_owner(path) && OWNED.contains(&name)
    }

    /// The rows for one file's second copies.
    fn violations(path: &Path, text: &str) -> Vec<String> {
        definitions(text)
            .iter()
            .filter(|name| violates(path, name))
            .map(|name| format!("{}: {name}", path.display()))
            .collect()
    }

    /// Appends every second copy in one `*.rs` file.
    fn check_file(path: &Path, offenders: &mut Vec<String>) {
        if path.is_dir() || path.extension() != Some(OsStr::new("rs")) {
            return;
        }
        let Ok(text) = fs::read_to_string(path) else {
            return;
        };
        offenders.extend(violations(path, &text));
    }

    /// Recurses into one entry when it is a directory.
    fn follow_dir(entry: &fs::DirEntry, offenders: &mut Vec<String>) {
        if entry.path().is_dir() {
            collect_offenders(&entry.path(), offenders);
        }
    }

    /// Appends every second copy of an owned or banned name under `dir`.
    fn collect_offenders(dir: &Path, offenders: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            check_file(&entry.path(), offenders);
            follow_dir(&entry, offenders);
        }
    }

    /// The guarded tree: `src/bsp/`.
    fn bsp_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(ROOT)
    }

    /// Whether the real `bsp/records.rs` still defines `name`.
    fn owner_still_defines(name: &str) -> bool {
        fs::read_to_string(bsp_dir().join(OWNER))
            .is_ok_and(|text| definitions(&text).iter().any(|held| held == name))
    }

    /// The owned names `bsp/records.rs` no longer defines, as report rows.
    fn stale_rows() -> Vec<String> {
        OWNED
            .iter()
            .filter(|name| !owner_still_defines(name))
            .map(|name| format!("{name} is not defined by src/{ROOT}/{OWNER}"))
            .collect()
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
            "writer.rs",
            "fn write_box(out: &mut W, bbox: B) -> R {\n    R\n}\n",
        );
        write(
            root,
            "lumps.rs",
            "pub(super) fn child_word(child: C) -> W {\n    W\n}\n",
        );
        write(
            root,
            "gl/fields.rs",
            "fn field_i16(value: i64, what: &str) -> R {\n    R\n}\n",
        );
        write(root, "gl/table.rs", "struct VertexTable {\n    f: K,\n}\n");
    }

    /// Writes the files the walk must leave alone: the owner, a comment, a
    /// test module and the formats' own wrappers.
    fn write_allowed(root: &Path) {
        write(
            root,
            OWNER,
            "pub(super) fn write_box(out: &mut W, bbox: B) -> R {\n    R\n}\n\
             pub(super) enum Spot {\n    Known(usize),\n}\n",
        );
        write(
            root,
            "commented.rs",
            "// fn write_box() {}\nfn other() {}\n",
        );
        write(
            root,
            "tested.rs",
            "#[cfg(test)]\nmod tests {\n    fn child_word() {}\n}\n",
        );
        write(
            root,
            "vanilla.rs",
            "struct SplitVertices {\n    table: VertexTable<(i32, i32), Vertex>,\n}\n",
        );
        write(
            root,
            "gl/write.rs",
            "pub(super) fn write_vertices(t: &V) -> Vec<u8> {\n    V\n}\n",
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
            rows.contains("writer.rs")
                && rows.contains("lumps.rs")
                && rows.contains("fields.rs")
                && rows.contains("table.rs"),
            "every second copy and every banned spelling is reported: {rows}"
        );
        assert!(
            !rows.contains("commented.rs")
                && !rows.contains("tested.rs")
                && !rows.contains("vanilla.rs")
                && !rows.contains(OWNER),
            "comments, test modules, the wrappers and the owner pass: {rows}"
        );
    }

    #[test]
    fn the_node_record_machinery_is_defined_once_in_the_bsp_tree() {
        let mut offenders: Vec<String> = Vec::new();
        collect_offenders(&bsp_dir(), &mut offenders);
        offenders.sort();
        assert!(
            offenders.is_empty(),
            "the shared BSP node record, its child words, its vertex table \
             and its refusals live in src/bsp/records.rs;\nthese files carry \
             a second copy:\n{}",
            offenders.join("\n"),
        );
    }

    #[test]
    fn every_owned_name_is_still_defined_by_the_owner() {
        let stale = stale_rows();
        assert!(
            stale.is_empty(),
            "these guarded names have moved, so this guard's list is stale:\n{}",
            stale.join("\n"),
        );
    }

    #[test]
    fn a_definition_is_an_fn_struct_enum_or_type_line() {
        assert_eq!(
            defined_name("pub(super) fn write_box(out: &mut W) -> R {"),
            Some("write_box")
        );
        assert_eq!(
            defined_name("const fn scaled(v: i32) -> i64 {"),
            Some("scaled")
        );
        assert_eq!(
            defined_name("struct SplitVertices {"),
            Some("SplitVertices")
        );
        assert_eq!(defined_name("enum ChildWidth {"), Some("ChildWidth"));
        assert_eq!(
            defined_name("type Endpoints = VertexTable<(i64, i64), (i32, i32)>;"),
            Some("Endpoints")
        );
        assert_eq!(defined_name("let table = VertexTable::new();"), None);
        assert_eq!(defined_name("// fn write_node() {}"), None);
    }

    #[test]
    fn the_three_writers_keep_only_their_own_narrowing() {
        // The one rule the fold must not blur: each format still spells its
        // own index space and refusal wording, and only `records.rs` still
        // spells the record layout.
        let read = |file: &str| {
            fs::read_to_string(bsp_dir().join(file))
                .unwrap_or_else(|error| panic!("read {file}: {error}"))
        };
        let classic = read("vanilla.rs");
        let extended = read("extended.rs");
        let gl = read("gl/write.rs");
        assert!(
            classic.contains("MAX_VERTICES") && classic.contains("SplitVertices"),
            "vanilla keeps its own vertex space and table wrapper"
        );
        assert!(
            extended.contains("u32::try_from(total)") && extended.contains("LumpVertices"),
            "extended keeps its own index space and table wrapper"
        );
        assert!(
            gl.contains("GL_VERTEX_FLAG") && gl.contains("VertexWords"),
            "GL keeps its own flagged vertex words"
        );
        assert!(
            !classic.contains("fn write_node(")
                && !extended.contains("fn write_node(")
                && !gl.contains("fn write_node("),
            "the node record is written once, in the shared records module"
        );
    }
}
