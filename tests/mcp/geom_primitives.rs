//! The single-owner guard for the CD-285 polygon, segment and ring fold:
//! the shared geometry primitives are defined once, under `src/geom/`,
//! and the copies this ticket folded away — the arrangement's
//! `normalize_polygon`/`polygon_area2`/`segments_cross`, the outline's
//! `has_crossing`/`strict_sides`/`segments_cross`, the validator's
//! `proper_crossing`, the overlay's `s_*` sample helpers and
//! `side_normal`, the gl region builder's `strictly_collinear`, the
//! simplifier's `dist2`, the plan's free `region_label`, and a second
//! union-find `unite` — never come back. The primitive names may only be
//! `fn`-defined under `src/geom/`; the folded spellings may not be
//! `fn`-defined anywhere under `src/`.

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;
    use std::path::{Path, PathBuf};

    /// The one directory name the repository walk never enters (build
    /// products, and any `.hidden` directory, are not this rule's business).
    const SKIPPED_DIRECTORY: &str = "target";

    /// The module directory the shared primitives belong to.
    const GEOM_DIRECTORY: &str = "geom";

    /// Every primitive the fold owns: a `fn` definition of one of these
    /// names outside `src/geom/` is a second copy that can fall out of
    /// step. Thin adapters (the describer's `label_point` reshaping the
    /// anchor for its report) keep their own names; these are the engines.
    const GEOM_FNS: [&str; 16] = [
        "crosses_properly",
        "distance_point_segment",
        "drop_collinear",
        "edge_area2",
        "grid_hypot",
        "integer_length",
        "intersect_x",
        "intersect_y",
        "normalize_ring",
        "region_id",
        "ring_edges",
        "segments_meet",
        "self_crossing",
        "side_samples",
        "squared_length",
        "unite",
    ];

    /// The folded-away spellings: none of these may be a `fn` definition
    /// anywhere under `src/` again.
    const BANNED_FNS: [&str; 14] = [
        "dist2",
        "has_crossing",
        "normalize_polygon",
        "polygon_area2",
        "proper_crossing",
        "region_label",
        "s_cross",
        "s_delta",
        "s_dist2",
        "s_dot",
        "segments_cross",
        "side_normal",
        "strict_sides",
        "strictly_collinear",
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

    /// The name of a `fn NAME(` or `fn NAME<..>(` definition a line
    /// carries, past any visibility and `const`/`unsafe`/`async`
    /// qualifiers, with any generic parameter list trimmed away.
    fn defined_fn(line: &str) -> Option<&str> {
        let mut words = definition_text(line)?;
        for qualifier in ["const ", "unsafe ", "async ", "extern "] {
            words = words.strip_prefix(qualifier).unwrap_or(words);
        }
        let body = words.strip_prefix("fn ")?;
        let name_end = body.find('(')?;
        let spelled = body.get(..name_end)?;
        let name = spelled.split('<').next().unwrap_or(spelled);
        (!name.is_empty()).then_some(name)
    }

    /// Whether one of `path`'s parent directory components is the geom
    /// directory: the guard only accepts a primitive definition there. It
    /// reads the component names, so it holds both for the real `src/`
    /// walk and for the fixture tree.
    fn lives_in_geom(path: &Path) -> bool {
        path.parent().is_some_and(|parent| {
            parent
                .components()
                .any(|part| part.as_os_str() == OsStr::new(GEOM_DIRECTORY))
        })
    }

    /// The row for one line that redefines or revives a shared name,
    /// `None` when the file may hold it.
    fn offender_row(path: &Path, number: usize, line: &str) -> Option<String> {
        let name = defined_fn(line)?;
        let revived = BANNED_FNS.contains(&name);
        let copied = GEOM_FNS.contains(&name) && !lives_in_geom(path);
        if !revived && !copied {
            return None;
        }
        Some(format!(
            "{}:{}: {} is a second copy of a `src/geom/` primitive",
            path.display(),
            number.saturating_add(1),
            line.trim(),
        ))
    }

    /// Appends every offending definition one `*.rs` file carries.
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
                .filter_map(|(number, line)| offender_row(&path, number, line)),
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
    fn the_polygon_segment_and_ring_primitives_are_defined_once() {
        let mut offenders: Vec<String> = Vec::new();
        collect_offenders(&src_dir(), &mut offenders);
        offenders.sort();
        assert!(
            offenders.is_empty(),
            "the shared polygon, segment and ring primitives live under \
             src/geom/ and the folded-away copies stay gone;\nthese files \
             carry a second copy:\n{}",
            rows(&offenders),
        );
    }

    /// Writes one file into the walk fixture.
    fn write(dir: &Path, name: &str, text: &str) {
        fs::write(dir.join(name), text).unwrap_or_else(|error| panic!("write {name}: {error}"));
    }

    #[test]
    fn the_walk_reports_copies_and_revivals_and_skips_the_rest() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path();
        write(
            root,
            "copy.rs",
            "fn ring_edges(vertices: &[(i64, i64)]) -> Vec<(i64, i64)> {\n        Vec::new()\n    }\n",
        );
        write(
            root,
            "revived.rs",
            "pub(super) fn side_normal(start: (i64, i64), end: (i64, i64)) -> (i128, i128) {}\n",
        );
        write(
            root,
            "generic.rs",
            "pub(crate) const fn distance_point_segment(a: i128) -> i128 { a }\n",
        );
        write(
            root,
            "innocent.rs",
            "// fn unite() {}\nfn other() {}\nfn label_point(edges: &[u8]) -> bool { true }\n",
        );
        let geom = root.join("geom");
        fs::create_dir_all(&geom).expect("geom dir");
        write(
            &geom,
            "owner.rs",
            "pub(crate) fn normalize_ring<P: Copy>(ring: &[P]) -> Vec<P> {\n        Vec::new()\n    }\n",
        );
        let mut offenders: Vec<String> = Vec::new();
        collect_offenders(root, &mut offenders);
        let spelled = rows(&offenders);
        assert!(
            spelled.contains("copy.rs") && spelled.contains("ring_edges"),
            "a copy outside geom is reported: {spelled}"
        );
        assert!(
            spelled.contains("revived.rs") && spelled.contains("side_normal"),
            "a revived folded name is reported anywhere: {spelled}"
        );
        assert!(
            spelled.contains("generic.rs") && spelled.contains("distance_point_segment"),
            "a const definition with a trimmed signature is reported: {spelled}"
        );
        assert!(
            !spelled.contains("innocent.rs"),
            "comments, other names and the adapter's label_point stay clean: {spelled}"
        );
        assert!(
            !spelled.contains("owner.rs"),
            "definitions inside a geom directory are the owners: {spelled}"
        );
    }

    #[test]
    fn a_definition_is_a_named_fn_past_visibility_and_generics() {
        assert_eq!(
            defined_fn("  pub(crate) fn ring_edges<P: Point>(x: &[P])"),
            Some("ring_edges")
        );
        assert_eq!(
            defined_fn("pub(super) const fn distance_point_segment(a: i128)"),
            Some("distance_point_segment")
        );
        assert_eq!(
            defined_fn("fn side_offset(start: Pt, end: Pt)"),
            Some("side_offset")
        );
        assert_eq!(defined_fn("// fn self_crossing() {}"), None);
        assert_eq!(defined_fn("let ring_edges = compute();"), None);
        assert_eq!(defined_fn("use crate::geom::ring::ring_edges;"), None);
        assert_eq!(defined_fn("fn f(x: i128)"), Some("f"));
        assert_eq!(defined_fn("const fn (crate::x) weird()"), None);
    }
}
