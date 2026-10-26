//! The imports-group duplication guard for the CD-324 fold: the theme-role
//! step, the automatic-ops fold and the one working-archive write with its
//! overwrite rule are defined once, in `src/tools/imports.rs`; the landing
//! helpers beside them are defined once inside the group; and no file of the
//! group reads a palette with the two-pass form the single quantization
//! replaced. A second copy is a report that can fall out of step with the
//! lump it describes, which is what this keeps from happening again.

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;
    use std::path::{Path, PathBuf};

    /// The one directory name the repository walk never enters (build
    /// products, and any `.hidden` directory, are not this rule's business).
    const SKIPPED_DIRECTORY: &str = "target";

    /// The file the group's shared helpers belong to: `src/tools/imports.rs`.
    const ROOT_FILE: &str = "imports.rs";

    /// The group's own directory, whose every file the group rules cover:
    /// `src/tools/imports/`.
    const GROUP_DIRECTORY: &str = "imports";

    /// Every helper the whole tree may define only once, in the group's root
    /// file: the theme-role step both creates end with, the fold every
    /// automatic image step runs through, the shared insertion and the
    /// payload constructor that carries the overwrite rule.
    const ROOT_FNS: [&str; 4] = ["apply_extra", "apply_role", "put_lump", "region_write"];

    /// Every helper that may be defined only once *inside* the group: the
    /// landing block that moved to the root with them. These names belong to
    /// another group's work too (`lumps::edit` reads a region, a working
    /// match and an overwrite its own way), so this rule stops at the group
    /// boundary.
    const LANDING_FNS: [&str; 4] = [
        "exists_message",
        "overwrite",
        "region_text",
        "working_match",
    ];

    /// The two-pass palette reads the single quantization replaced. The group
    /// quantizes once, so it calls only the measured forms; the plain walk
    /// and the dither walk stay where the fusion is defined.
    const TWO_PASS_CALLS: [&str; 2] = ["quantize_stats(", "quantize_dithered("];

    /// The name of a `fn NAME(` definition a line carries, past any
    /// visibility and `const`/`unsafe`/`async` qualifiers.
    fn defined_fn(line: &str) -> Option<&str> {
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
        let mut words = unprefixed.trim_start();
        for qualifier in ["const ", "unsafe ", "async ", "extern "] {
            words = words.strip_prefix(qualifier).unwrap_or(words);
        }
        let body = words.strip_prefix("fn ")?;
        let name_end = body.find('(')?;
        let name = body.get(..name_end)?;
        (!name.is_empty()).then_some(name)
    }

    /// One report row: `path:line: the line there`.
    fn offender(path: &Path, number: usize, line: &str) -> String {
        format!(
            "{}:{}: {}",
            path.display(),
            number.saturating_add(1),
            line.trim()
        )
    }

    /// Whether `path` is the file these helpers belong to.
    fn is_root_file(path: &Path) -> bool {
        path.file_name().is_some_and(|name| name == ROOT_FILE)
    }

    /// The row for one line that defines a name the file may not hold: the
    /// whole-tree rule for [`ROOT_FNS`], the group-boundary rule for
    /// [`LANDING_FNS`].
    fn duplicate_row(path: &Path, in_group: bool, number: usize, line: &str) -> Option<String> {
        if is_root_file(path) {
            return None;
        }
        let name = defined_fn(line)?;
        let owned = ROOT_FNS.contains(&name) || (in_group && LANDING_FNS.contains(&name));
        owned.then(|| offender(path, number, line))
    }

    /// The rows for one file's two-pass palette reads.
    fn two_pass_reads(path: &Path, text: &str) -> Vec<String> {
        text.lines()
            .enumerate()
            .filter_map(|(number, line)| {
                let twice = TWO_PASS_CALLS.iter().any(|call| line.contains(*call));
                twice.then(|| offender(path, number, line))
            })
            .collect()
    }

    /// Appends one offender row when `path` is a `*.rs` file the rules
    /// cover: a duplicate helper anywhere, or, inside the group, a palette
    /// read that walks the picture twice.
    fn check_file(path: &Path, in_group: bool, offenders: &mut Vec<String>) {
        if path.is_dir() || path.extension() != Some(OsStr::new("rs")) {
            return;
        }
        let Ok(text) = fs::read_to_string(path) else {
            return;
        };
        let rows = text
            .lines()
            .enumerate()
            .filter_map(|(number, line)| duplicate_row(path, in_group, number, line));
        offenders.extend(rows);
        if in_group {
            offenders.extend(two_pass_reads(path, &text));
        }
    }

    /// Whether one directory entry's name is walked: `target/` and every
    /// hidden directory are not.
    fn is_walked_dir(name: &OsStr) -> bool {
        let text = name.to_string_lossy();
        text != SKIPPED_DIRECTORY && !text.starts_with('.')
    }

    /// Recurses into one entry when it is a directory the walk keeps, carrying
    /// the group flag into `imports/` and everything below it.
    fn follow_dir(entry: &fs::DirEntry, in_group: bool, offenders: &mut Vec<String>) {
        let path = entry.path();
        if !path.is_dir() || !is_walked_dir(&entry.file_name()) {
            return;
        }
        let deeper = in_group || entry.file_name() == GROUP_DIRECTORY;
        collect_offenders(&path, deeper, offenders);
    }

    /// Appends every duplicate and two-pass read under `dir`; `in_group` says
    /// whether `dir` is the imports group's own directory or one under it.
    fn collect_offenders(dir: &Path, in_group: bool, offenders: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            check_file(&entry.path(), in_group, offenders);
            follow_dir(&entry, in_group, offenders);
        }
    }

    /// The repository's `src/` tree.
    fn src_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
    }

    /// The offenders of the whole `src/` tree, sorted for one readable report.
    fn src_offenders() -> Vec<String> {
        let mut offenders: Vec<String> = Vec::new();
        collect_offenders(&src_dir(), false, &mut offenders);
        offenders.sort();
        offenders
    }

    /// Writes one file into the walk fixture.
    fn write(dir: &Path, name: &str, text: &str) {
        fs::write(dir.join(name), text).unwrap_or_else(|error| panic!("write {name}: {error}"));
    }

    /// The imports directory of one fixture root, made for the group rules.
    fn group_dir(root: &Path) -> PathBuf {
        let nested = root.join(GROUP_DIRECTORY).join("nested");
        fs::create_dir_all(&nested).expect("make the group directories");
        nested
    }

    #[test]
    fn the_imports_helpers_are_defined_once() {
        let offenders = src_offenders();
        assert!(
            offenders.is_empty(),
            "the theme-role step, the automatic-ops fold and the one \
             working-archive write live in src/tools/imports.rs, their landing \
             helpers once each inside the group, and the group quantizes with \
             the measured forms only;\nthese files carry a second copy:\n{}",
            offenders.join("\n"),
        );
    }

    #[test]
    fn the_guard_reads_the_helpers_the_group_actually_holds() {
        let text = fs::read_to_string(src_dir().join("tools").join(ROOT_FILE))
            .expect("the group root file");
        for name in ROOT_FNS.iter().chain(LANDING_FNS.iter()) {
            assert!(
                text.contains(&format!("fn {name}(")),
                "src/tools/imports.rs no longer defines {name}: the guard's \
                 list and the group have fallen out of step"
            );
        }
    }

    #[test]
    fn the_walk_reports_duplicates_and_two_pass_reads() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path();
        let group = group_dir(root);
        write(
            root,
            "elsewhere.rs",
            "pub(in crate::tools) fn apply_role(session: &mut Session) {}\n",
        );
        write(
            root,
            "other.rs",
            "pub(super) fn region_text(what: Option<Namespace>) -> String {\n    String::new()\n}\n",
        );
        write(
            &group,
            "second.rs",
            "fn region_text(what: Option<Namespace>) -> String {\n    String::new()\n}\n",
        );
        write(
            &group,
            "again.rs",
            "let (used, error) = quantize_stats(&picture, &palette, &options);\n",
        );
        write(
            &group,
            "clean.rs",
            "let measured = quantize_dithered_measured(&picture, &palette, &options, mode);\n",
        );
        write(
            &group,
            "commented.rs",
            "// fn put_lump(wad: &mut Wad) {}\nfn unrelated() {}\n",
        );
        write(
            root,
            ROOT_FILE,
            "pub(in crate::tools) fn put_lump(wad: &mut Wad) -> usize { 0_usize }\n",
        );
        let mut offenders: Vec<String> = Vec::new();
        collect_offenders(root, false, &mut offenders);
        let found = offenders.join("\n");
        assert!(
            found.contains("elsewhere.rs:1")
                && found.contains("second.rs:1")
                && found.contains("again.rs:1"),
            "a root duplicate, a group duplicate and a two-pass read are all \
             reported: {found}"
        );
        assert!(
            !found.contains("clean.rs")
                && !found.contains("commented.rs")
                && !found.contains("other.rs"),
            "the measured call, a comment line and another group's own helper \
             pass: {found}"
        );
    }

    #[test]
    fn a_definition_is_a_line_defining_a_function() {
        assert_eq!(
            defined_fn("  pub(in crate::tools) const fn region_write(name: LumpName) {"),
            Some("region_write")
        );
        assert_eq!(
            defined_fn("    let apply_role = true;"),
            None,
            "a binding is not a definition"
        );
        assert_eq!(
            defined_fn("/// [`apply_extra`]' errors"),
            None,
            "a doc line is not a definition"
        );
        assert_eq!(defined_fn("fn other() {}"), Some("other"));
    }
}
