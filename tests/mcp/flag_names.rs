//! The flag-name glossary guard: every linedef and thing flag-bit `const`
//! lives once in `src/wad/map/flags.rs`, and no other Rust file under
//! `src/` may redefine one of those names (or an older local alias the
//! vocabulary replaced) with a raw numeric literal.

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;
    use std::path::Path;

    /// The one directory name the repository walk never enters (build
    /// products, and any `.hidden` directory, are not this rule's
    /// business).
    const SKIPPED_DIRECTORY: &str = "target";

    /// The flags module the names below belong to.
    const FLAGS_FILE: &str = "flags.rs";

    /// Every linedef and thing flag-bit name: the `const` definitions of
    /// these (and of the older local aliases this vocabulary replaced)
    /// live once, in `src/wad/map/flags.rs`. No other Rust file under
    /// `src/` may define one, so no bit is ever spelled twice.
    const FLAG_BIT_NAMES: &[&str] = &[
        // The names flags.rs owns.
        "ACTIVATION_MASK",
        "ACTIVATION_SHIFT",
        "BLOCKING",
        "BLOCK_MONSTERS",
        "BLOCK_SOUND",
        "DONT_DRAW",
        "DONT_PEG_BOTTOM",
        "DONT_PEG_TOP",
        "HEXEN_BLOCK_BITS",
        "HEXEN_BLOCK_EVERYTHING",
        "HEXEN_BLOCK_PLAYERS",
        "IMPACT",
        "MAPPED",
        "MONSTER_ACTIVATE",
        "MONSTER_CROSS",
        "MISSILE_CROSS",
        "PASS_USE",
        "PLAYER_CROSS",
        "PLAYER_CROSS_BITS",
        "PLAYER_PUSH",
        "PLAYER_USE",
        "PLAYER_USE_BITS",
        "REPEAT_SPECIAL",
        "SECRET",
        "SHARED_LINEDEF_BITS",
        "THING_AMBUSH",
        "THING_DOOM_FRIEND",
        "THING_DOOM_NOT_COOP",
        "THING_DOOM_NOT_DM",
        "THING_DOOM_NOT_SINGLE",
        "THING_EASY",
        "THING_HARD",
        "THING_HEXEN_CLASS1",
        "THING_HEXEN_CLASS2",
        "THING_HEXEN_CLASS3",
        "THING_HEXEN_CLASSES",
        "THING_HEXEN_COOP",
        "THING_HEXEN_DORMANT",
        "THING_HEXEN_DM",
        "THING_HEXEN_FRIEND",
        "THING_HEXEN_INVISIBLE",
        "THING_HEXEN_SINGLE",
        "THING_HEXEN_STANDING",
        "THING_HEXEN_TRANSLUCENT",
        "THING_MEDIUM",
        "THING_SHARED_BITS",
        "THING_SKILL_BITS",
        "TWO_SIDED",
        // The local spellings CD-280 folded into the list above.
        "BLOCKING_BIT",
        "BLOCKING_BITS",
        "BLOCKING_FLAG",
        "BLOCK_EVERYTHING",
        "BLOCK_PLAYERS",
        "CROSS",
        "DOOM_DEFAULT_FLAGS",
        "DOOM_FLAG_BITS",
        "DOOM_PASSUSE",
        "DONTPEG_BOTTOM_BIT",
        "DONTPEG_TOP_BIT",
        "EASY",
        "FLAG_BLOCKING",
        "FLAG_DONT_DRAW",
        "FLAG_LOWER_UNPEGGED",
        "FLAG_SECRET",
        "FLAG_TWOSIDED",
        "HARD",
        "HEXEN_DEFAULT_FLAGS",
        "HEXEN_REPEAT",
        "MEDIUM",
        "MONSTER",
        "NOT_SINGLE",
        "PLAYER_USE_FIELD",
        "REPEAT",
        "REPEAT_SPECIAL_BIT",
        "SINGLE_PLAYER",
        "TWOSIDED",
        "TWO_SIDED_BIT",
        "TWO_SIDED_BITS",
        "TWO_SIDED_FLAG",
        "USE",
        "VISIBLE_FLAGS",
    ];

    /// The line's text past a leading `//` guard and any `pub` or
    /// `pub(..)` visibility, when the line is a definition candidate.
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
        unprefixed
            .trim_start()
            .strip_prefix("const ")
            .filter(|after| !after.starts_with("fn "))
    }

    /// Whether `name` is an all-uppercase constant name.
    fn is_upper_name(name: &str) -> bool {
        !name.is_empty()
            && name
                .chars()
                .all(|letter| letter.is_ascii_uppercase() || letter == '_')
    }

    /// The flag-bit constant a source line defines: the name of a
    /// `const NAME: u32 = <numeric literal>` declaration, past any `pub`
    /// or `pub(..)` prefix. Comments, `const fn`s, non-`u32` types and
    /// values that are not a plain numeric literal (a recipe such as
    /// `THING_SKILL_BITS | THING_HEXEN_SINGLE`) are not definitions.
    fn defined_flag_const(line: &str) -> Option<&str> {
        let after = definition_text(line)?;
        let (name, typed) = after.split_once(':')?;
        if !is_upper_name(name) {
            return None;
        }
        let assigned = typed.trim_start().strip_prefix("u32")?.trim_start();
        let value = assigned.strip_prefix('=')?.trim_start();
        value
            .chars()
            .next()
            .is_some_and(|digit| digit.is_ascii_digit())
            .then_some(name)
    }

    /// Appends one `path:line` offender when `line` redefines a name of
    /// [`FLAG_BIT_NAMES`] in a file other than the flags module.
    fn push_flag_offender(path: &Path, number: usize, line: &str, offenders: &mut Vec<String>) {
        let banned = defined_flag_const(line).is_some_and(|name| FLAG_BIT_NAMES.contains(&name));
        if !banned {
            return;
        }
        offenders.push(format!(
            "{}:{}: {}",
            path.display(),
            number.saturating_add(1),
            line.trim()
        ));
    }

    /// Appends every `FLAG_BIT_NAMES` const defined in `path` (a file
    /// other than the flags module) to `offenders`.
    fn collect_redefined_flags(path: &Path, offenders: &mut Vec<String>) {
        if path.file_name().is_some_and(|name| name == FLAGS_FILE) {
            return;
        }
        let Ok(text) = fs::read_to_string(path) else {
            return;
        };
        for (number, line) in text.lines().enumerate() {
            push_flag_offender(path, number, line, offenders);
        }
    }

    /// Whether one directory entry's name is walked: `target/` and every
    /// hidden directory are not.
    fn is_walked_dir(name: &OsStr) -> bool {
        let text = name.to_string_lossy();
        text != SKIPPED_DIRECTORY && !text.starts_with('.')
    }

    /// Whether one path names Rust source, the way `cargo` itself does.
    fn is_rust_file(path: &Path) -> bool {
        path.extension() == Some(OsStr::new("rs"))
    }

    /// Recurses into one entry when it is a directory the flag walk keeps.
    fn follow_flag_dir(entry: &fs::DirEntry, offenders: &mut Vec<String>) {
        if !entry.path().is_dir() {
            return;
        }
        if is_walked_dir(&entry.file_name()) {
            collect_flag_redefinitions(&entry.path(), offenders);
        }
    }

    /// Runs [`collect_redefined_flags`] on one entry that is a `*.rs` file.
    fn check_flag_file(entry: &fs::DirEntry, offenders: &mut Vec<String>) {
        let path = entry.path();
        if path.is_dir() {
            return;
        }
        if is_rust_file(&path) {
            collect_redefined_flags(&path, offenders);
        }
    }

    /// Appends every flag-bit const redefined under `dir`, recursing into
    /// the directories the walk keeps.
    fn collect_flag_redefinitions(dir: &Path, offenders: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            follow_flag_dir(&entry, offenders);
            check_flag_file(&entry, offenders);
        }
    }

    #[test]
    fn flag_bit_constants_are_defined_only_in_the_flags_module() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders: Vec<String> = Vec::new();
        collect_flag_redefinitions(&src, &mut offenders);
        offenders.sort();
        assert!(
            offenders.is_empty(),
            "linedef/thing flag-bit consts belong in src/wad/map/flags.rs;\n\
             these files redefine one:\n{}",
            offenders.join("\n"),
        );
    }

    #[test]
    fn the_flag_name_walk_reads_definitions_and_skips_the_rest() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path();
        fs::write(
            root.join("redef.rs"),
            "pub(crate) const BLOCKING: u32 = 1_u32;\n",
        )
        .expect("write a redefinition");
        fs::write(
            root.join("importer.rs"),
            "use crate::wad::map::flags::TWO_SIDED;\n",
        )
        .expect("write the importing file");
        fs::write(
            root.join("commented.rs"),
            "/// The `SECRET` bit.\n// const SECRET: u32 = 32;\nfn body() {}\n",
        )
        .expect("write the commenting file");
        fs::write(
            root.join("associated.rs"),
            "impl Kind {\n    pub const USE: Self = Self(2_u32);\n}\n",
        )
        .expect("write the associated-const file");
        fs::write(
            root.join("funcs.rs"),
            "pub(crate) const fn blocks_players(bits: u32) -> bool {\n    bits & 1_u32 != 0\n}\n",
        )
        .expect("write the const-fn file");
        fs::write(
            root.join(FLAGS_FILE),
            "pub(crate) const BLOCKING: u32 = 1_u32;\n",
        )
        .expect("write the flags file");
        fs::write(
            root.join("recipe.rs"),
            "const HEXEN_DEFAULT_FLAGS: u32 = THING_SKILL_BITS | THING_HEXEN_SINGLE;\n\
             const MAPPED_COUNT: u32 = rows.len();\n",
        )
        .expect("write the recipe file");
        fs::write(root.join("decimal.rs"), "pub const EASY: u32 = 1;")
            .expect("write the unsuffixed literal");
        let mut offenders: Vec<String> = Vec::new();
        collect_flag_redefinitions(root, &mut offenders);
        let rows: String = offenders.join("\n");
        assert!(
            offenders.len() == 2 && rows.contains("redef.rs:1") && rows.contains("decimal.rs:1"),
            "only the raw-literal redefinitions are reported: {rows}"
        );
    }
}
