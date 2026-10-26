//! The CD-328 layering guard: the render, `WAD`, plan, `BSP`, geom, gfx,
//! audio, music, defs, sketch, themes and session core must never import the
//! MCP tool layer. Shared picture, texture, palette and map-block resolvers
//! live in the session layer (`src/session/resolve.rs`, `src/session/textures.rs`,
//! `src/session/palette.rs`, `src/session/mapdir.rs`) and the tools
//! re-export them, so a core module can reach a resource without a
//! `use crate::tools::` edge that would pull the whole tool layer down into
//! the pure core. Only files under `src/tools/` (and its `tools.rs` root) and
//! the `src/server.rs` entry point that composes every group router may name
//! `crate::tools`.

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;
    use std::path::{Path, PathBuf};

    /// The one directory name the repository walk never enters (build
    /// products, and any `.hidden` directory, are not this rule's business).
    const SKIPPED_DIRECTORY: &str = "target";

    /// The `use`-path spellings that reach the tool layer from outside it.
    const TOOLS_IMPORT_MARKERS: [&str; 3] = ["crate::tools", "super::tools", "self::tools"];

    /// The crate-root file that owns the tool layer, exempt like its
    /// directory.
    const TOOLS_FILE: &str = "tools.rs";

    /// The directory the tool layer lives in.
    const TOOLS_DIR: &str = "tools/";

    /// The crate-root file the guard also exempts: the MCP entry point that
    /// composes every group router and so names the tools by design.
    const SERVER_FILE: &str = "server.rs";

    /// The line's text past a leading `//` comment guard, when it is code
    /// rather than a doc or line comment.
    fn code_text(line: &str) -> Option<&str> {
        let text = line.trim_start();
        (!text.starts_with("//")).then_some(text)
    }

    /// The imported path(s) of a `use` declaration line: the text after the
    /// `use ` keyword, past any `pub`/`pub(..)`/`pub(in ..)` visibility.
    /// `None` when the line is not a `use` declaration or is a comment, so a
    /// doc-comment intra-doc link to the tool layer never reads as an import.
    fn use_clause(line: &str) -> Option<&str> {
        let text = code_text(line)?;
        let past_pub = text.strip_prefix("pub").unwrap_or(text);
        let past_vis = match past_pub
            .strip_prefix('(')
            .and_then(|inner| inner.split_once(')'))
        {
            Some((_vis, tail)) => tail,
            None => past_pub,
        };
        past_vis
            .trim_start()
            .strip_prefix("use ")
            .map(str::trim_start)
    }

    /// Whether a `use` clause imports the tool layer.
    fn names_tools(clause: &str) -> bool {
        TOOLS_IMPORT_MARKERS
            .iter()
            .any(|marker| clause.contains(marker))
    }

    /// The row for one `use` line that imports the tool layer, `None` when
    /// the line is any other statement.
    fn tools_import_row(path: &Path, number: usize, line: &str) -> Option<String> {
        let clause = use_clause(line)?;
        if !names_tools(clause) {
            return None;
        }
        Some(format!(
            "{}:{}: {}",
            path.display(),
            number.saturating_add(1),
            line.trim()
        ))
    }

    /// Whether a `src/`-relative path names the exempt tool layer or the
    /// server entry point (whose files may import the tools they compose).
    fn is_exempt_rel(rel: &Path) -> bool {
        let text = rel.to_string_lossy();
        text == SERVER_FILE || text == TOOLS_FILE || text.starts_with(TOOLS_DIR)
    }

    /// Appends every tool-layer import in one `*.rs` file, `root` the
    /// directory reported paths are taken relative to, unless the file is
    /// exempt.
    fn check_file_for_tools_imports(path: &Path, root: &Path, offenders: &mut Vec<String>) {
        let Ok(rel) = path.strip_prefix(root) else {
            return;
        };
        if path.is_dir() || path.extension() != Some(OsStr::new("rs")) || is_exempt_rel(rel) {
            return;
        }
        let Ok(text) = fs::read_to_string(path) else {
            return;
        };
        let rows = text
            .lines()
            .enumerate()
            .filter_map(|(number, line)| tools_import_row(rel, number, line));
        offenders.extend(rows);
    }

    /// Whether one directory entry's name is walked: `target/` and every
    /// hidden directory are not.
    fn is_walked_source_dir(name: &OsStr) -> bool {
        let text = name.to_string_lossy();
        text != SKIPPED_DIRECTORY && !text.starts_with('.')
    }

    /// Recurses into one entry when it is a directory the walk keeps.
    fn follow_source_dir(
        entry: &fs::DirEntry,
        root: &Path,
        offenders: &mut Vec<String>,
    ) -> std::io::Result<()> {
        let path = entry.path();
        if path.is_dir() && is_walked_source_dir(&entry.file_name()) {
            collect_tool_layer_imports(&path, root, offenders)?;
        }
        Ok(())
    }

    /// Appends every tool-layer import under `dir`, `root` the directory the
    /// reported paths are taken relative to.
    fn collect_tool_layer_imports(
        dir: &Path,
        root: &Path,
        offenders: &mut Vec<String>,
    ) -> std::io::Result<()> {
        for entry in fs::read_dir(dir)?.filter_map(Result::ok) {
            check_file_for_tools_imports(&entry.path(), root, offenders);
            follow_source_dir(&entry, root, offenders)?;
        }
        Ok(())
    }

    /// The repository's `src/` tree.
    fn src_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
    }

    /// Every tool-layer import under `src/` outside the tools directory and
    /// `server.rs`, as `path:line: text` rows relative to `src/`.
    ///
    /// # Panics
    ///
    /// When `src/` cannot be read.
    fn tool_layer_imports_in_src() -> Vec<String> {
        let root = src_dir();
        let mut offenders: Vec<String> = Vec::new();
        collect_tool_layer_imports(&root, &root, &mut offenders).expect("read the src tree");
        offenders.sort();
        offenders
    }

    #[test]
    fn core_modules_do_not_import_the_tool_layer() {
        let offenders = tool_layer_imports_in_src();
        assert!(
            offenders.is_empty(),
            "the render, wad, plan and session core must reach resources through \
             their own modules, not `crate::tools`; move shared resolvers into a \
             session or wad module and re-export them from tools.\nthese files \
             import the tool layer:\n{}",
            offenders.join("\n"),
        );
    }

    /// The shared picture resolvers and the map/palette/texture loaders the
    /// renderers need must live in the session layer, so this guard would
    /// have caught the debt it now prevents: `src/render/scene.rs` once read
    /// them at `crate::tools::graphics::resolve`, `crate::tools::maps::common`
    /// and `crate::tools::palette`.
    #[test]
    fn the_render_scene_reads_the_session_resolvers() {
        let text = fs::read_to_string(src_dir().join("render").join("scene.rs"))
            .expect("read src/render/scene.rs");
        assert!(
            !text.contains("crate::tools"),
            "src/render/scene.rs must not name crate::tools at all",
        );
        for needed in [
            "crate::session::mapdir::load_map",
            "crate::session::palette::load_palette",
            "crate::session::resolve",
        ] {
            assert!(
                text.contains(needed),
                "src/render/scene.rs should read {needed}: {text}"
            );
        }
    }

    #[test]
    fn the_use_clause_reads_only_real_use_declarations() {
        assert_eq!(
            use_clause("use crate::tools::palette::load_palette;"),
            Some("crate::tools::palette::load_palette;")
        );
        assert_eq!(
            use_clause("pub(in crate::tools) use crate::session::mapdir::Directory;"),
            Some("crate::session::mapdir::Directory;"),
            "the visibility path never counts as the import",
        );
        assert_eq!(
            use_clause("pub(crate) use crate::session::resolve::load_flat;"),
            Some("crate::session::resolve::load_flat;")
        );
        assert_eq!(
            use_clause("    let value = crate::tools::fail(&err);"),
            None
        );
        assert_eq!(use_clause("/// [`crate::tools::fail`] reports one."), None);
        assert_eq!(use_clause("//! the crate::tools layer"), None);
    }

    /// Writes one file into the layering-guard walk fixture.
    fn write_source(dir: &Path, rel: &str, text: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("make the fixture directory");
        }
        fs::write(&path, text).expect("write the fixture file");
    }

    #[test]
    fn the_layering_walk_reports_core_imports_and_skips_the_rest() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path();
        write_source(
            root,
            "render.rs",
            "use crate::tools::maps::common::load_map;\n",
        );
        write_source(root, "scene/notes.rs", "use super::tools::fail;\n");
        write_source(
            root,
            "tools/group.rs",
            "use crate::tools::session::run_batch;\n",
        );
        write_source(root, "tools.rs", "use crate::tools::reply;\n");
        write_source(
            root,
            "server.rs",
            "use crate::tools::meta::CyberdemonRouter;\n",
        );
        write_source(
            root,
            "reexport.rs",
            "pub(in crate::tools) use crate::session::mapdir::Directory;\n",
        );
        write_source(
            root,
            "clean.rs",
            "//! [`crate::tools::fail`] is the helper.\npub(crate) use crate::session::resolve;\n",
        );
        fs::create_dir_all(root.join(SKIPPED_DIRECTORY)).expect("make the skipped directory");
        write_source(
            root,
            &format!("{SKIPPED_DIRECTORY}/gen.rs"),
            "use crate::tools::nope;\n",
        );
        let mut offenders: Vec<String> = Vec::new();
        collect_tool_layer_imports(root, root, &mut offenders).expect("read the fixture tree");
        let rows: String = offenders.join("\n");
        assert!(
            rows.contains("render.rs:1") && rows.contains("scene/notes.rs:1"),
            "every core import of the tool layer is reported: {rows}"
        );
        assert!(
            !rows.contains("group.rs")
                && !rows.contains("tools.rs")
                && !rows.contains("server.rs")
                && !rows.contains("reexport.rs")
                && !rows.contains("clean.rs")
                && !rows.contains(SKIPPED_DIRECTORY),
            "the tools layer, the server file, a pub(in) re-export, a doc link \
             and the build tree all pass: {rows}"
        );
    }
}
