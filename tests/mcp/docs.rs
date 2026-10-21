//! Documentation-layout checks: the `skill/` tree and `README.md` keep the
//! cross-references and size limits the contributor rules promise, every
//! guide's `Tools:` line names real tools the guide also uses, every
//! `json plan-example` block in the guides compiles into a playable map,
//! and every `json recipe` block is a well-formed `run_batch` payload whose
//! fixture-runnable steps actually succeed on the features fixture session.

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;
    use std::path::{Path, PathBuf};

    use serde_json::{Value, json};

    use crate::fixtures::iwad_features;
    use crate::support::{call, connect, midi_bytes, png_bytes, wav_bytes};

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

    #[test]
    fn editing_guide_stays_within_its_line_budget() {
        let lines = read("skill/guides/editing-existing-maps.md")
            .lines()
            .count();
        assert!(
            lines <= 250,
            "editing-existing-maps.md is {lines} lines, the limit is 250"
        );
    }

    #[test]
    fn recipes_guide_stays_within_its_line_budget() {
        let lines = read("skill/guides/recipes.md").lines().count();
        assert!(
            lines <= 400,
            "recipes.md is {lines} lines, the limit is 400"
        );
    }

    /// The fence info string a runnable recipe carries.
    const RECIPE_FENCE: &str = "```json recipe";

    /// The placeholder a recipe uses for a `PNG` file source.
    const IMAGE_PLACEHOLDER: &str = "<IMAGE>";

    /// The placeholder a recipe uses for a MIDI file source.
    const MIDI_PLACEHOLDER: &str = "<MIDI>";

    /// The placeholder a recipe uses for a `WAV` file source.
    const WAV_PLACEHOLDER: &str = "<WAV>";

    /// The floors for the recipe checks: a minimum count of
    /// fixture-runnable recipes.
    const MIN_FIXTURE_RECIPES: usize = 15;

    /// The bodies of every `json recipe` fenced block in `markdown`.
    fn recipe_blocks(markdown: &str) -> Vec<Value> {
        markdown
            .split(RECIPE_FENCE)
            .skip(1)
            .filter_map(|rest| rest.split("\n```").next())
            .map(|body| serde_json::from_str(body).expect("every json recipe block is valid JSON"))
            .collect()
    }

    /// The `(guide, recipe)` pairs of every recipe block in every guide.
    fn all_recipes() -> Vec<(String, Value)> {
        guide_file_names()
            .into_iter()
            .flat_map(|file| {
                recipe_blocks(&guide_text(&file))
                    .into_iter()
                    .map(move |block| (file.clone(), block))
            })
            .collect()
    }

    /// Requires one recipe to be `{fixture: bool, steps: [...]}` with
    /// every step naming a listed tool and carrying object arguments.
    fn check_recipe_shape(file: &str, recipe: &Value, known: &[&str]) {
        let Some(object) = recipe.as_object() else {
            panic!("{file}: a recipe block must hold a JSON object");
        };
        assert!(
            !object.keys().any(|key| key != "fixture" && key != "steps"),
            "{file}: a recipe holds more than the fixture and steps keys",
        );
        assert!(
            recipe.get("fixture").and_then(Value::as_bool).is_some(),
            "{file}: a recipe needs a boolean fixture flag",
        );
        let Some(steps) = recipe.get("steps").and_then(Value::as_array) else {
            panic!("{file}: a recipe needs a steps array");
        };
        assert!(!steps.is_empty(), "{file}: a recipe has no steps");
        for step in steps {
            check_recipe_step(file, step, known);
        }
    }

    /// Requires one recipe step to name a listed tool, with object
    /// arguments when it carries any.
    fn check_recipe_step(file: &str, step: &Value, known: &[&str]) {
        let Some(tool) = step.get("tool").and_then(Value::as_str) else {
            panic!("{file}: a recipe step needs a tool name");
        };
        assert!(
            known.contains(&tool),
            "{file}: a recipe step calls the unknown tool {tool}",
        );
        assert!(
            step.get("arguments").is_none_or(Value::is_object),
            "{file}: step {tool} arguments must be an object",
        );
    }

    /// Checks every recipe block in every guide against `known`.
    fn check_all_recipe_shapes(known: &[&str]) {
        let recipes = all_recipes();
        assert!(!recipes.is_empty(), "no `json recipe` block in any guide");
        for entry in &recipes {
            check_recipe_shape(&entry.0, &entry.1, known);
        }
    }

    #[tokio::test]
    async fn every_recipe_is_well_formed_with_real_tools() {
        let client = connect().await;
        let listed = client.list_all_tools().await.expect("list tools");
        let known: Vec<&str> = listed.iter().map(|tool| tool.name.as_ref()).collect();
        check_all_recipe_shapes(&known);
    }

    /// Writes the three media files a recipe placeholder can name into
    /// `dir` and returns the placeholder -> path lookup as a JSON object.
    fn write_media(dir: &Path) -> Value {
        let png = dir.join("recipe-media.png");
        let midi = dir.join("recipe-media.mid");
        let wav = dir.join("recipe-media.wav");
        fs::write(&png, png_bytes()).expect("write the test PNG");
        fs::write(&midi, midi_bytes()).expect("write the test MIDI");
        fs::write(&wav, wav_bytes()).expect("write the test WAV");
        json!({
            IMAGE_PLACEHOLDER: png.display().to_string(),
            MIDI_PLACEHOLDER: midi.display().to_string(),
            WAV_PLACEHOLDER: wav.display().to_string(),
        })
    }

    /// The media path a placeholder string names, if `value` is one.
    fn placeholder_path(value: &Value, media: &Value) -> Option<String> {
        let text = value.as_str()?;
        let found = media.get(text).and_then(Value::as_str)?;
        Some(found.to_owned())
    }

    /// Replaces every placeholder string inside `value` with its media
    /// path from the `media` lookup.
    fn substitute(value: &mut Value, media: &Value) {
        if let Some(path) = placeholder_path(value, media) {
            *value = json!(path);
            return;
        }
        substitute_children(value, media);
    }

    /// Recurses into the children of a `value` that is no string.
    fn substitute_children(value: &mut Value, media: &Value) {
        if let Some(entries) = value.as_object_mut() {
            let items: Vec<&mut Value> = entries.values_mut().collect();
            substitute_all(items, media);
            return;
        }
        if let Some(items) = value.as_array_mut() {
            let references: Vec<&mut Value> = items.iter_mut().collect();
            substitute_all(references, media);
        }
    }

    /// Substitutes through every child.
    fn substitute_all(items: Vec<&mut Value>, media: &Value) {
        for item in items {
            substitute(item, media);
        }
    }

    /// Whether one recipe is flagged fixture-runnable.
    fn is_fixture_recipe(recipe: &Value) -> bool {
        recipe.get("fixture") == Some(&json!(true))
    }

    /// Runs one fixture recipe through `run_batch` on a fresh fixture
    /// session and requires the batch to succeed.
    async fn run_fixture_recipe(dir: &Path, media: &Value, file: &str, recipe: &Value) {
        let client = connect().await;
        let base = iwad_features(dir);
        call(
            &client,
            "open_iwad",
            json!({"path": base.display().to_string()}),
        )
        .await;
        call(&client, "new_wad", json!({})).await;
        let mut args = json!({"steps": recipe["steps"].clone(), "label": file});
        substitute(&mut args, media);
        let report = call(&client, "run_batch", args).await;
        assert_eq!(
            report.get("ok"),
            Some(&json!(true)),
            "{file}: the fixture recipe runs clean: {report}",
        );
    }

    /// Runs every fixture recipe, each in its own session.
    async fn run_all_fixture_recipes(dir: &Path, media: &Value, recipes: Vec<(String, Value)>) {
        let fixture_count = recipes
            .iter()
            .filter(|entry| is_fixture_recipe(&entry.1))
            .count();
        assert!(
            fixture_count >= MIN_FIXTURE_RECIPES,
            "only {fixture_count} fixture recipes, the floor is {MIN_FIXTURE_RECIPES}",
        );
        let runnable: Vec<(String, Value)> = recipes
            .into_iter()
            .filter(|entry| is_fixture_recipe(&entry.1))
            .collect();
        for (file, recipe) in runnable {
            run_fixture_recipe(dir, media, &file, &recipe).await;
        }
    }

    #[tokio::test]
    async fn every_fixture_recipe_runs_through_run_batch() {
        let dir = tempfile::tempdir().expect("temp dir");
        let media = write_media(dir.path());
        run_all_fixture_recipes(dir.path(), &media, all_recipes()).await;
    }
}
