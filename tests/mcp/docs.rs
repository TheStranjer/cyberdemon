//! Documentation-layout checks: the `skill/` tree and `README.md` keep the
//! cross-references and size limits the contributor rules promise, every
//! guide's `Tools:` line names real tools the guide also uses, and every
//! `json plan-example` block in the guides compiles into a playable map.

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;
    use std::path::{Path, PathBuf};

    use serde_json::{Value, json};

    use crate::fixtures::iwad_features;
    use crate::support::{call, connect};

    /// The prefix every guide's tool-list line starts with.
    const TOOLS_PREFIX: &str = "Tools:";

    /// The fence info string a buildable plan example carries.
    const PLAN_EXAMPLE_FENCE: &str = "```json plan-example";

    /// The name `check_playability` must answer `ok: true` for in the
    /// example builds.
    const EXAMPLE_MAP: &str = "MAP02";

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

    /// Reads one guide file.
    fn guide_text(name: &str) -> String {
        read(&format!("skill/guides/{name}"))
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

    /// The names on a guide's `Tools:` line, split on commas and trimmed;
    /// `None` when the guide has no such line.
    fn tools_line_names(markdown: &str) -> Option<Vec<String>> {
        let line = markdown
            .lines()
            .map(str::trim_start)
            .find(|line| line.starts_with(TOOLS_PREFIX))?;
        let list = line.strip_prefix(TOOLS_PREFIX).unwrap_or_default();
        Some(
            list.split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .collect(),
        )
    }

    /// The `(guide, names)` pairs of every guide's `Tools:` line, empty
    /// for a guide without one.
    fn listed_tools_by_guide() -> Vec<(String, Vec<String>)> {
        guide_file_names()
            .into_iter()
            .map(|file| {
                let names = tools_line_names(&guide_text(&file)).unwrap_or_default();
                (file, names)
            })
            .collect()
    }

    /// Whether `name` appears as a whole word in `text`.
    fn mentions_name(text: &str, name: &str) -> bool {
        text.split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .any(|word| word == name)
    }

    /// The guide's markdown without its `Tools:` line: the body a listed
    /// name must appear in.
    fn body_without_tools_line(markdown: &str) -> String {
        markdown
            .lines()
            .filter(|line| !line.trim_start().starts_with(TOOLS_PREFIX))
            .collect::<Vec<&str>>()
            .join("\n")
    }

    /// The bodies of every `json plan-example` fenced block in `markdown`.
    fn plan_examples(markdown: &str) -> Vec<Value> {
        markdown
            .split(PLAN_EXAMPLE_FENCE)
            .skip(1)
            .filter_map(|rest| rest.split("\n```").next())
            .map(|body| {
                serde_json::from_str(body).expect("every json plan-example block is valid JSON")
            })
            .collect()
    }

    /// The `(guide, plan)` pairs of every example block in every guide.
    fn all_plan_examples() -> Vec<(String, Value)> {
        guide_file_names()
            .into_iter()
            .flat_map(|file| {
                plan_examples(&guide_text(&file))
                    .into_iter()
                    .map(move |block| (file.clone(), block))
            })
            .collect()
    }

    /// Builds one plan example into a fresh session and requires
    /// `check_playability` to answer `ok: true`.
    async fn build_and_play(file: &str, block: Value) {
        let dir = tempfile::tempdir().expect("temp dir");
        let client = connect().await;
        let base = iwad_features(dir.path());
        call(
            &client,
            "open_iwad",
            json!({"path": base.display().to_string()}),
        )
        .await;
        call(&client, "new_wad", json!({})).await;
        let mut args = block;
        args["map"] = json!(EXAMPLE_MAP);
        args["finalize"] = json!(true);
        let built = call(&client, "build_map_from_plan", args).await;
        assert_eq!(
            built["map"],
            json!(EXAMPLE_MAP),
            "{file}: the example builds its block"
        );
        let report = call(&client, "check_playability", json!({"map": EXAMPLE_MAP})).await;
        assert_eq!(
            report["ok"],
            json!(true),
            "{file}: the example plays: {}",
            report["summary"]
        );
    }

    /// Requires one guide's `Tools:` names to be listed tools the guide
    /// also uses in its body.
    fn tools_line_is_honest(file: &str, names: &[String], known: &[&str]) {
        let body = body_without_tools_line(&guide_text(file));
        for name in names {
            assert!(
                known.contains(&name.as_str()),
                "{file} lists the unknown tool {name}",
            );
            assert!(
                mentions_name(&body, name),
                "{file} lists {name} on its Tools line but never uses it",
            );
        }
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

    #[test]
    fn every_guide_lists_its_tools() {
        for file in guide_file_names() {
            let has_line = tools_line_names(&guide_text(&file)).is_some();
            assert!(has_line, "{file} has no `Tools:` line after its title");
        }
    }

    #[tokio::test]
    async fn every_listed_tool_exists_and_is_used() {
        let client = connect().await;
        let listed = client.list_all_tools().await.expect("list tools");
        let known: Vec<&str> = listed.iter().map(|tool| tool.name.as_ref()).collect();
        check_every_tools_line(&known);
    }

    /// Applies [`tools_line_is_honest`] to every guide's `Tools:` line.
    fn check_every_tools_line(known: &[&str]) {
        for (file, names) in listed_tools_by_guide() {
            tools_line_is_honest(&file, &names, known);
        }
    }

    #[tokio::test]
    async fn every_plan_example_builds_a_playable_map() {
        let examples = all_plan_examples();
        assert!(
            !examples.is_empty(),
            "no `json plan-example` block in any guide",
        );
        build_and_play_all(examples).await;
    }

    /// Builds every extracted example, each in its own session.
    async fn build_and_play_all(examples: Vec<(String, Value)>) {
        for (file, block) in examples {
            build_and_play(&file, block).await;
        }
    }

    #[test]
    fn sketch_guide_stays_within_its_line_budget() {
        let lines = read("skill/guides/sketch-to-map.md").lines().count();
        assert!(
            lines <= 350,
            "sketch-to-map.md is {lines} lines, the limit is 350"
        );
    }

    #[test]
    fn images_guide_stays_within_its_line_budget() {
        let lines = read("skill/guides/images-to-assets.md").lines().count();
        assert!(
            lines <= 250,
            "images-to-assets.md is {lines} lines, the limit is 250"
        );
    }
}
