//! The single-owner guard for the CD-327 duplication fold: `geom::Pt` is
//! the map-unit point, so the folded redefinitions — `describe`'s
//! `type Point = (i64, i64)` tuple alias (the pixel aliases under
//! `src/sketch/` stay, they are another domain), the `[i64; 2]` and
//! `(i64, i64)` point-field spellings of the describer's centre and the
//! plan model's `at`/`pad`/`destination` rows — never come back. The
//! connection's `from->to` label formats in exactly one place
//! (`Connection::label_for` in `src/plan/model.rs`), the folded
//! `derived_label`/`connection_name` helpers stay deleted, and the thing
//! flag-table bit read is defined once, in `src/wad/map/flags.rs`. The
//! `bsp` 16.16 point, the model-coordinate `ops::Point` (`i32`), the
//! serializable `transform::Point` object and the sketch pixels are
//! different domains and are not this rule's business.

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;
    use std::path::{Path, PathBuf};

    /// The one directory name the repository walk never enters (build
    /// products, and any `.hidden` directory, are not this rule's business).
    const SKIPPED_DIRECTORY: &str = "target";

    /// The pixel domain whose `type Point = (i64, i64)` aliases are kept:
    /// sketch works in image pixels, not map units.
    const SKETCH_DIRECTORY: &str = "sketch";

    /// The path past `src/` the connection label formats and the plan
    /// model's point fields belong to.
    const MODEL_PATH: &str = "plan/model.rs";

    /// The path past `src/` the thing flag-table bit read belongs to.
    const FLAGS_PATH: &str = "wad/map/flags.rs";

    /// The path past `src/` whose sector centre is a `Pt`, never a
    /// `[i64; 2]`.
    const DESCRIBE_PATH: &str = "wad/map/describe.rs";

    /// The point-alias spelling the fold removed everywhere but sketch.
    const POINT_ALIAS: &str = "type Point = (i64, i64)";

    /// The arrow spellings of the derived connection label; the plan model
    /// owns the one that remains.
    const ARROW_FORMATS: [&str; 2] = ["{from}->{to}", "{}->{}"];

    /// The folded-away label helpers: none of these may be a `fn`
    /// definition anywhere under `src/` again.
    const BANNED_FNS: [&str; 2] = ["derived_label", "connection_name"];

    /// The line's text past a leading `//` guard, `None` for a comment
    /// line, so no prose counts as a definition or a re-spelling.
    fn code_text(line: &str) -> Option<&str> {
        let text = line.trim_start();
        (!text.starts_with("//")).then_some(text)
    }

    /// The line's text past a leading `//` guard and any `pub` or `pub(..)`
    /// visibility, when the line is a definition candidate.
    fn definition_text(line: &str) -> Option<&str> {
        let text = code_text(line)?;
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

    /// One report row: `path:line: the offending line`.
    fn offender(path: &Path, number: usize, line: &str) -> String {
        format!(
            "{}:{}: {}",
            path.display(),
            number.saturating_add(1),
            line.trim(),
        )
    }

    /// Whether `path` lives under the sketch pixel directory: its point
    /// aliases are kept.
    fn lives_in_sketch(path: &Path) -> bool {
        path.parent().is_some_and(|parent| {
            parent
                .components()
                .any(|part| part.as_os_str() == OsStr::new(SKETCH_DIRECTORY))
        })
    }

    /// Whether `path` is the owner file the rule names, matched by the
    /// path's tail so the check holds both for the real `src/` walk and
    /// for the fixture tree.
    fn is_owner(path: &Path, owner_tail: &str) -> bool {
        path.display().to_string().ends_with(owner_tail)
    }

    /// The rows for one line: the pixel-kept point alias outside sketch, a
    /// second connection-label arrow, a revived folded helper, a second
    /// thing-bit read, and the `[i64; 2]`/`(i64, i64)` point-field
    /// spellings in the two folded files.
    fn offender_rows(path: &Path, number: usize, line: &str) -> Vec<String> {
        let Some(text) = code_text(line) else {
            return Vec::new();
        };
        let alias = text.contains(POINT_ALIAS) && !lives_in_sketch(path);
        let arrow = text.contains("format!(")
            && ARROW_FORMATS.iter().any(|spelling| text.contains(spelling))
            && !is_owner(path, MODEL_PATH);
        let centre = text.contains("Option<[i64; 2]>") && is_owner(path, DESCRIBE_PATH);
        let field = text.contains("Option<(i64, i64)>") && is_owner(path, MODEL_PATH);
        let revived = defined_fn(line)
            .is_some_and(|name| BANNED_FNS.contains(&name) || is_second_thing_bit(name, path));
        let copies = [alias, arrow, centre, field, revived]
            .into_iter()
            .filter(|copy| *copy)
            .count();
        (0..copies).map(|_| offender(path, number, line)).collect()
    }

    /// Whether a `thing_bit` definition on `line` sits outside the flags
    /// module that owns the thing flag-table read.
    fn is_second_thing_bit(name: &str, path: &Path) -> bool {
        name == "thing_bit" && !is_owner(path, FLAGS_PATH)
    }

    /// Appends every folded-away spelling one `*.rs` file carries.
    fn check_file(entry: &fs::DirEntry, offenders: &mut Vec<String>) {
        let path = entry.path();
        if path.is_dir() || path.extension() != Some(OsStr::new("rs")) {
            return;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            return;
        };
        offenders.extend(
            text.lines()
                .enumerate()
                .flat_map(|(number, line)| offender_rows(&path, number, line)),
        );
    }

    /// Whether one directory entry's name is walked: `target/` and every
    /// hidden directory are not.
    fn is_walked_dir(name: &OsStr) -> bool {
        let text = name.to_string_lossy();
        text != SKIPPED_DIRECTORY && !text.starts_with('.')
    }

    /// Recurses into one entry when it is a directory the walk keeps.
    fn follow_dir(entry: &fs::DirEntry, offenders: &mut Vec<String>) {
        if !entry.path().is_dir() {
            return;
        }
        if is_walked_dir(&entry.file_name()) {
            collect_offenders(&entry.path(), offenders);
        }
    }

    /// Appends every offender in the tree under `dir`.
    fn collect_offenders(dir: &Path, offenders: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            check_file(&entry, offenders);
            follow_dir(&entry, offenders);
        }
    }

    /// The `src/` tree of this repository.
    fn src_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
    }

    /// One report of offenders, as the failing assert names them.
    fn rows(offenders: &[String]) -> String {
        offenders.join("\n")
    }

    #[test]
    fn the_point_aliases_label_format_and_thing_bit_are_defined_once() {
        let mut offenders: Vec<String> = Vec::new();
        collect_offenders(&src_dir(), &mut offenders);
        offenders.sort();
        assert!(
            offenders.is_empty(),
            "geom::Pt is the map-unit point, the connection arrow label \
             formats in src/plan/model.rs, and the thing-bit table read \
             lives in src/wad/map/flags.rs;\nthese files carry a second \
             copy:\n{}",
            rows(&offenders),
        );
    }

    /// Writes one file into the walk fixture.
    fn write(dir: &Path, name: &str, text: &str) {
        fs::write(dir.join(name), text).unwrap_or_else(|error| panic!("write {name}: {error}"));
    }

    /// Builds the walk fixture: the offending redefinitions, the kept
    /// sketch alias, the three owner files and an innocent line.
    fn build_fixture(root: &Path) {
        write(root, "alias.rs", "type Point = (i64, i64);\n");
        write(
            root,
            "arrow.rs",
            "fn label(from: &str, to: &str) -> String {\n    format!(\"{}->{}\", from, to)\n}\n",
        );
        write(
            root,
            "revived.rs",
            "fn derived_label(a: &str) -> String {\n    a.to_owned()\n}\n",
        );
        write(
            root,
            "bit.rs",
            "fn thing_bit(family: u8, wanted: &str) -> u32 {\n    0_u32\n}\n",
        );
        write(
            root,
            "innocent.rs",
            "// type Point = (i64, i64);\nfn other() {}\nlet span = (1_i64, 2_i64);\n",
        );
        let sketch = root.join(SKETCH_DIRECTORY);
        fs::create_dir_all(&sketch).expect("sketch dir");
        write(&sketch, "weld.rs", "type Point = (i64, i64);\n");
    }

    /// Builds the three owner files that must pass: the plan model with
    /// its arrow, the flags module with its `thing_bit` and the describer
    /// with its `Pt` centre.
    fn build_owners(root: &Path) {
        let plan = root.join("plan");
        let wad = root.join("wad").join("map");
        fs::create_dir_all(&plan).expect("plan dir");
        fs::create_dir_all(&wad).expect("wad dir");
        write(
            &plan,
            "model.rs",
            "id.map_or_else(|| format!(\"{from}->{to}\"), str::to_owned)\npub at: Option<Pt>,\n",
        );
        write(
            &wad,
            "flags.rs",
            "pub fn thing_bit(family: u8, wanted: &str) -> u32 {\n    0_u32\n}\n",
        );
        write(&wad, "describe.rs", "pub center: Option<Pt>,\n");
    }

    #[test]
    fn the_walk_reports_every_folded_spelling_and_skips_the_rest() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path();
        build_fixture(root);
        build_owners(root);
        let mut offenders: Vec<String> = Vec::new();
        collect_offenders(root, &mut offenders);
        let spelled = rows(&offenders);
        for wanted in ["alias.rs", "arrow.rs", "revived.rs", "bit.rs"] {
            assert!(spelled.contains(wanted), "{wanted} is reported: {spelled}");
        }
        assert_eq!(
            spelled.matches(".rs:").count(),
            4,
            "exactly the four offenders are reported: {spelled}"
        );
        assert!(
            !spelled.contains("innocent.rs")
                && !spelled.contains("weld.rs")
                && !spelled.contains("flags.rs")
                && !spelled.contains("model.rs")
                && !spelled.contains("describe.rs"),
            "comments, other names, plain tuples, the sketch pixel alias \
             and the three owner files pass: {spelled}"
        );
    }
}
