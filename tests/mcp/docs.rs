//! Documentation-layout checks: the `skill/` tree and `README.md` keep the
//! cross-references and size limits the contributor rules promise.

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;
    use std::path::{Path, PathBuf};

    /// The guides directory at the repository root.
    fn guides_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("skill/guides")
    }

    /// Reads a repository file relative to the crate root.
    fn read(relative: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
        fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!("cannot read {}: {error}", path.display());
        })
    }

    /// Whether a file name or link target carries the markdown extension.
    fn is_markdown(name: &str) -> bool {
        Path::new(name).extension() == Some(OsStr::new("md"))
    }

    /// Extracts the markdown link targets that point at `.md` files:
    /// `[text](some/page.md)` yields `some/page.md`, anchors dropped.
    fn md_link_targets(markdown: &str) -> Vec<String> {
        markdown
            .split("](")
            .skip(1)
            .filter_map(|rest| rest.split_once(')'))
            .map(|(target, _)| target.split_once('#').map_or(target, |(head, _)| head))
            .filter(|target| is_markdown(target))
            .map(str::to_owned)
            .collect()
    }

    /// The markdown files in the guides directory, excluding the index.
    fn guide_file_names() -> Vec<String> {
        fs::read_dir(guides_dir())
            .expect("read the guides directory")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| is_markdown(name) && name != "README.md")
            .collect()
    }

    #[test]
    fn every_guide_linked_in_the_index_exists() {
        let links = md_link_targets(&read("skill/guides/README.md"));
        assert!(!links.is_empty(), "the guides index links no files");
        for target in links {
            let path = guides_dir().join(&target);
            assert!(path.is_file(), "index links a missing guide: {target}");
        }
    }

    #[test]
    fn every_guide_file_is_linked_from_the_index() {
        let links = md_link_targets(&read("skill/guides/README.md"));
        for name in guide_file_names() {
            assert!(
                links.contains(&name),
                "guide missing from the index: {name}"
            );
        }
    }

    #[test]
    fn skill_entry_point_links_the_guides_index() {
        let skill = read("skill/SKILL.md");
        assert!(
            md_link_targets(&skill)
                .iter()
                .any(|target| target == "guides/README.md"),
            "skill/SKILL.md must link the guides index at guides/README.md"
        );
    }

    #[test]
    fn root_readme_stays_within_100_lines() {
        let lines = read("README.md").lines().count();
        assert!(lines <= 100, "README.md is {lines} lines, the limit is 100");
    }
}
