//! The shared-math guard: the separable box window and the bilinear tap
//! blend live once, in `src/gfx/filter.rs`, and the integer `Rec. 601`
//! luminance weights are spelled in one file only
//! (`src/gfx/ops/colour.rs`). No other Rust file under `src/` may redefine a
//! shared helper or spell a second copy of the weighting, which is the
//! duplication CD-317 folded away.

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;
    use std::path::{Path, PathBuf};

    /// The one directory name the repository walk never enters (build
    /// products, and any `.hidden` directory, are not this rule's business).
    const SKIPPED_DIRECTORY: &str = "target";

    /// The module the shared window and sampler belong to.
    const FILTER_FILE: &str = "filter.rs";

    /// Every helper `src/gfx/filter.rs` owns: a `fn` definition of one of
    /// these names anywhere else under `src/` is a second copy of the
    /// separable window or the bilinear tap blend. Three names the module
    /// also exports are left out because they are plain words other owners
    /// use for their own work: `blend` (`render::canvas`), `write_column`
    /// (`gfx::patch`) and `window_span` (`plan::connectors`).
    const SHARED_FILTER_FNS: [&str; 8] = [
        "box_filter",
        "box_sums",
        "bilinear",
        "bilinear_kernel",
        "column_of",
        "finish_pixel",
        "last_centre",
        "window_line",
    ];

    /// The file name the `Rec. 601` weights belong to: `gfx/ops/colour.rs`,
    /// which defines `luminance`, and its own test file, which quotes the
    /// weights in its messages.
    const LUMA_FILE: &str = "colour.rs";
    /// The integer `Rec. 601` luminance weights. The file that spells at
    /// least two of them is the one computing a luminance; there is exactly
    /// one, `src/gfx/ops/colour.rs`.
    const LUMA_WEIGHTS: [&str; 3] = ["299", "587", "114"];

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

    /// Whether `path` (a file other than the filter module) redefines one of
    /// the shared helpers, as report rows.
    fn shared_redefinition(path: &Path, text: &str) -> Vec<String> {
        if path.file_name().is_some_and(|name| name == FILTER_FILE) {
            return Vec::new();
        }
        text.lines()
            .enumerate()
            .filter_map(|(number, line)| {
                let duplicated =
                    defined_fn(line).is_some_and(|name| SHARED_FILTER_FNS.contains(&name));
                duplicated.then(|| offender(path, number, line))
            })
            .collect()
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

    /// Whether `text` spells at least two of the [`LUMA_WEIGHTS`], the mark
    /// of a second `Rec. 601` weighting.
    fn weights_the_luma_recipe(text: &str) -> bool {
        let spelled = LUMA_WEIGHTS
            .iter()
            .filter(|weight| text.contains(**weight))
            .count();
        spelled > 1
    }

    /// Appends one offender row when `path` is a `*.rs` file redefining a
    /// shared helper or spelling the luminance weighting again.
    fn check_file(entry: &fs::DirEntry, offenders: &mut Vec<String>) {
        let path = entry.path();
        if path.is_dir() || path.extension() != Some(OsStr::new("rs")) {
            return;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            return;
        };
        offenders.extend(shared_redefinition(&path, &text));
        let owns_weights = path.file_name().is_some_and(|name| name == LUMA_FILE);
        if !owns_weights && weights_the_luma_recipe(&text) {
            offenders.push(format!(
                "{}: spells the luminance weights again",
                path.display()
            ));
        }
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

    /// Appends every shared-helper redefinition and second luminance
    /// weighting under `dir`.
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

    /// One offender of a report, as the failing assert names it.
    fn rows(offenders: &[String]) -> String {
        offenders.join("\n")
    }

    #[test]
    fn the_shared_window_and_luminance_weights_are_defined_once() {
        let mut offenders: Vec<String> = Vec::new();
        collect_offenders(&src_dir(), &mut offenders);
        offenders.sort();
        assert!(
            offenders.is_empty(),
            "the box window and tap blend live in src/gfx/filter.rs and the \
             luminance weights in src/gfx/ops/colour.rs;\nthese files carry a \
             second copy:\n{}",
            rows(&offenders),
        );
    }

    /// Writes one file into the walk fixture.
    fn write(dir: &Path, name: &str, text: &str) {
        fs::write(dir.join(name), text).unwrap_or_else(|error| panic!("write {name}: {error}"));
    }

    #[test]
    fn the_walk_reports_redefinitions_and_skips_the_rest() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path();
        write(
            root,
            "copy.rs",
            "fn window_line(line: &[u64]) -> Vec<u64> {\n        Vec::new()\n    }\n",
        );
        write(
            root,
            "methods.rs",
            "impl Grid {\n    fn column_of(&self) -> Vec<u64> {\n        Vec::new()\n    }\n}\n",
        );
        write(
            root,
            "commented.rs",
            "// fn bilinear(x: i64) {}\nfn other() {}\n",
        );
        write(
            root,
            "filter.rs",
            "pub fn box_sums<S>(values: &[S]) -> Vec<u64> {}\n",
        );
        write(
            root,
            "plain.rs",
            "fn totals(line: &[u64]) -> u64 { line.len() as u64 }\n",
        );
        write(root, "luma.rs", "let weighted = red * 299 + green * 587;\n");
        write(
            root,
            "colour.rs",
            "let weighted = red * 299 + green * 587;\n",
        );
        write(root, "single.md", "the weights 299 and 587 in prose\n");
        let mut offenders: Vec<String> = Vec::new();
        collect_offenders(root, &mut offenders);
        let found = rows(&offenders);
        assert!(
            found.contains("copy.rs:1") && found.contains("methods.rs:2"),
            "both redefinitions are reported: {found}"
        );
        assert!(
            found.contains("luma.rs"),
            "the second weighting is reported: {found}"
        );
        assert!(
            !found.contains("commented.rs")
                && !found.contains("filter.rs")
                && !found.contains("plain.rs")
                && !found.contains("colour.rs")
                && !found.contains("single.md"),
            "comments, the filter module, unrelated names and non-Rust files pass: {found}"
        );
    }

    #[test]
    fn a_definition_is_a_line_defining_a_function() {
        assert_eq!(
            defined_fn("  pub(crate) fn blend(img: &RgbaImage) {}"),
            Some("blend")
        );
        assert_eq!(defined_fn("const fn some(ink: u64) -> bool"), Some("some"));
        assert_eq!(
            defined_fn("pub use crate::gfx::filter::blend;"),
            None,
            "a re-export is not a definition"
        );
        assert_eq!(
            defined_fn("let blend = true;"),
            None,
            "a binding is not a definition"
        );
        assert_eq!(
            defined_fn("// fn blend() {}"),
            None,
            "a comment is not a definition"
        );
    }
}
