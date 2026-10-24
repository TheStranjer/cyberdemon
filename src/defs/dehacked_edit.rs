//! Line-oriented DeHackEd/BEX patch parsing and editing for the scripts
//! tools.
//!
//! The parser keeps the file's line structure: every block, section and key
//! line remembers its line, so `set_patch` and `remove_patch` rewrite
//! exactly the lines they must and leave the untouched ones as they were
//! (line endings normalise to `\n` and a non-empty text keeps a trailing
//! newline). Parsing is tolerant: unknown headers, `Include` lines and
//! malformed numbers become `Patch::warnings` entries, never errors; only
//! the edit functions fail, with `CyberError::InvalidArgument` for a
//! refused target, key or value and `CyberError::NotFound` for a missing
//! block or key.
//!
//! This module is the patch's data: the constants naming every legal block
//! kind and BEX section, the `Field` key lines, the `Block` and `Section`
//! shapes that carry them, the `Patch` that collects them and the `Opened`
//! region the reader fills. `reader` turns a text into that shape, `edit`
//! rewrites lines of a text through it and `tests` runs both over a sample
//! patch.

pub mod edit;
pub mod reader;

pub(crate) use edit::{check_key, check_value, parse_spec, remove_patch, set_patch};
pub(crate) use reader::parse;

#[cfg(test)]
mod tests;

/// The block kinds a `<Kind> [number]` header may name, canonical spelling.
const KINDS: [&str; 10] = [
    "Thing", "Frame", "Pointer", "Sound", "Ammo", "Weapon", "Misc", "Cheat", "Sprite", "Text",
];

/// The BEX sections a `[NAME]` header may open, canonical spelling.
const SECTIONS: [&str; 7] = [
    "CODEPTR", "STRINGS", "PARS", "SPRITES", "SOUNDS", "MUSIC", "HELPER",
];

/// The `Patch File for DeHackEd v3.0` title line, lowercased for matching.
const TITLE_LINE: &str = "patch file for dehacked";

/// The out-of-block key naming the `Doom` executable version.
const KEY_DOOM_VERSION: &str = "doom version";

/// The out-of-block key naming the DeHackEd patch format level.
const KEY_PATCH_FORMAT: &str = "patch format";

/// The header word of an include line: reported, never followed.
const WORD_INCLUDE: &str = "include";

/// The block kind whose header carries two lengths and an opaque body.
const KIND_TEXT: &str = "Text";

/// The text a created patch starts with: the canonical DeHackEd 3.0 header
/// plus a blank line, so appended blocks stand separated.
pub(crate) const NEW_PATCH_HEADER: &str =
    "Patch File for DeHackEd v3.0\nDoom version = 21\nPatch format = 6\n\n";

/// One `Key = Value` line of a block or section.
pub(crate) struct Field {
    /// The key as written (outer space trimmed).
    pub(crate) key: String,
    /// The value as written (outer space trimmed).
    pub(crate) value: String,
    /// The 0-based line this key stands on.
    pub(crate) line: usize,
}

/// One `<Kind> <number> [(name)]` block with its key lines.
pub(crate) struct Block {
    /// The canonical kind word (`Thing`, `Frame`, ...).
    pub(crate) kind: &'static str,
    /// The header number, `None` when the header carries none; a `Text`
    /// block stores its first length here.
    pub(crate) number: Option<i64>,
    /// The parenthesised name, when the header gives one; for a `Text`
    /// block, the second length as text.
    pub(crate) name: Option<String>,
    /// The 0-based line of the header.
    pub(crate) line: usize,
    /// The exclusive line the block ends before: the blank line, the next
    /// header or the file end.
    pub(crate) end: usize,
    /// The `Key = Value` lines, in file order.
    pub(crate) fields: Vec<Field>,
}

/// One `[NAME]` BEX section with its entry lines.
pub(crate) struct Section {
    /// The canonical section name (`STRINGS`, `CODEPTR`, ...).
    pub(crate) name: &'static str,
    /// The 0-based line of the header.
    pub(crate) line: usize,
    /// The exclusive line the section ends before: the next header or the
    /// file end (blank lines do not close a section).
    pub(crate) end: usize,
    /// The `KEY = VALUE` lines, in file order.
    pub(crate) entries: Vec<Field>,
}

/// A parsed DeHackEd/BEX patch text.
#[derive(Default)]
pub(crate) struct Patch {
    /// The `Doom version` header value, when present and numeric.
    pub(crate) doom_version: Option<i64>,
    /// The `Patch format` header value, when present and numeric.
    pub(crate) format: Option<i64>,
    /// The blocks, in file order.
    pub(crate) blocks: Vec<Block>,
    /// The BEX sections, in file order.
    pub(crate) sections: Vec<Section>,
    /// Notes about lines the tolerant parser refused to read (1-based).
    pub(crate) warnings: Vec<String>,
}

/// The block or section whose lines are being read.
enum Opened {
    /// A `<Kind> [number]` block.
    Block(Block),
    /// A `[NAME]` BEX section.
    Section(Section),
}
