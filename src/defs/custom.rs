//! Session definitions: the built-in game tables overlaid with the custom
//! thing definitions of the loaded archives.
//!
//! [`session_definitions`] starts from [`crate::defs::registry::definitions_for`]
//! and applies, in precedence order (built-in < DEHACKED < DECORATE/`ZScript`,
//! with MAPINFO `DoomEdNums` completing the `ZScript` side):
//!
//! - [`crate::defs::dehacked`]: every `Thing <n>` block of a patch, read
//!   through the shared `crate::defs::dehacked_edit::parse`, moves or
//!   edits the built-in row of its mobj;
//! - [`crate::defs::decorate`] and [`crate::defs::zscript`]: actors with a
//!   doomednum become things; every class is collected first and parents
//!   resolved after the last text, so a parent may stand before or after
//!   its child, and a stock parent names a built-in row whose sprite,
//!   frame and radius pass down; with nothing to inherit from, a class
//!   defaults to an empty sprite and radius 20;
//! - [`crate::defs::mapinfo`]: every `DoomEdNums` number becomes (or is
//!   replaced by) a thing named after its class; an entry for a class
//!   nothing defined places a name-only row labelled `mapinfo`.
//!
//! Every parser is tolerant: unparseable text produces warnings on the
//! result, never an error. The only error is the built-in table lookup's
//! [`crate::error::CyberError::Internal`] for malformed embedded data.

use alloc::collections::BTreeMap;

use crate::defs::registry::{Definitions, definitions_for, game_label};
use crate::defs::table::{Category, ThingDef};
use crate::defs::{decorate, dehacked, mapinfo, zscript};
use crate::error::CyberError;
use crate::wad::game::Game;

/// The radius of a class that names none and inherits none.
const DEFAULT_RADIUS: u32 = 20;

/// The frame of a class that names none and inherits none.
const DEFAULT_FRAME: char = 'A';

/// The category of things defined by a DECORATE actor number.
const CATEGORY_DECORATE: &str = "Custom (DECORATE)";

/// The category of things attached to a doomednum by MAPINFO.
const CATEGORY_MAPINFO: &str = "Custom (MAPINFO)";

/// The [`ThingDef::source`] label of DECORATE classes.
const SOURCE_DECORATE: &str = "decorate";

/// The [`ThingDef::source`] label of `ZScript` classes.
const SOURCE_ZSCRIPT: &str = "zscript";

/// The [`ThingDef::source`] label of the name-only row a `DoomEdNums`
/// entry synthesizes for a class nothing defined.
const SOURCE_MAPINFO: &str = "mapinfo";

/// The [`ThingDef::source`] label of a built-in row read as a stock
/// parent; a child still reports the language that defined it.
const SOURCE_BUILTIN: &str = "builtin";

/// The prefix of warnings about MAPINFO entries.
const MAPINFO_LABEL: &str = "mapinfo";

/// The custom definition lump texts of one session, each stream in load
/// order (the merged view's directory order, included lumps appended).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CustomSources {
    /// The text of every `DEHACKED` lump.
    pub dehacked: Vec<String>,
    /// The text of every `DECORATE` lump and the lumps it includes.
    pub decorate: Vec<String>,
    /// The text of every `ZSCRIPT` lump and the lumps it includes.
    pub zscript: Vec<String>,
    /// The text of every `MAPINFO` and `ZMAPINFO` lump.
    pub mapinfo: Vec<String>,
}

/// A parsed class after inheritance resolution.
#[derive(Clone, Debug)]
struct ResolvedClass {
    /// The class name as written.
    name: String,
    /// The four-letter sprite, empty when none is known.
    sprite: String,
    /// The default spawn frame.
    frame: char,
    /// The blocking radius in map units.
    radius: u32,
    /// `"decorate"` or `"zscript"`, the language that defined it.
    source: &'static str,
}

/// One parsed class, collected before the inheritance pass.
#[derive(Clone, Debug)]
struct Pending {
    /// The raw parse result.
    raw: decorate::RawClass,
    /// `"decorate"` or `"zscript"`, the language that defined it.
    source: &'static str,
    /// The category a placed row carries.
    category: &'static str,
}

/// The resolved view of one pending class: its own values, else the
/// parent's, else the defaults.
fn inherit(pending: &Pending, parent: Option<&ResolvedClass>) -> ResolvedClass {
    ResolvedClass {
        name: pending.raw.name.clone(),
        sprite: pending
            .raw
            .sprite
            .clone()
            .or_else(|| parent.map(|base| base.sprite.clone()))
            .unwrap_or_default(),
        frame: pending
            .raw
            .frame
            .or_else(|| parent.map(|base| base.frame))
            .unwrap_or(DEFAULT_FRAME),
        radius: pending
            .raw
            .radius
            .or_else(|| parent.map(|base| base.radius))
            .unwrap_or(DEFAULT_RADIUS),
        source: pending.source,
    }
}

/// The thing-table editor behind the class and `DoomEdNums` overlays.
///
/// Classes are collected first and resolved once the last text was read,
/// so a parent may stand anywhere in the run — in any DECORATE/`ZScript`
/// text, before or after its child — or be a stock class named by a
/// built-in row.
struct Overlay<'defs> {
    /// The definitions being overlaid (things and warnings).
    defs: &'defs mut Definitions,
    /// The resolved classes, keyed by lowercase class name, last wins.
    classes: BTreeMap<String, ResolvedClass>,
    /// Every collected class, keyed by lowercase name, last wins.
    pending: BTreeMap<String, Pending>,
    /// The collected names, lowercase, one entry per occurrence in text
    /// order.
    order: Vec<String>,
    /// The wire label of the game, for synthesized rows.
    game: &'static str,
}

impl<'defs> Overlay<'defs> {
    /// A fresh overlay for `defs` of `game`.
    const fn new(defs: &'defs mut Definitions, game: Game) -> Self {
        Self {
            defs,
            classes: BTreeMap::new(),
            pending: BTreeMap::new(),
            order: Vec::new(),
            game: game_label(game),
        }
    }

    /// Appends parser notes to the report warnings.
    fn warn(&mut self, mut messages: Vec<String>) {
        self.defs.warnings.append(&mut messages);
    }

    /// Collects one parsed class for [`Overlay::finish`]'s resolution pass,
    /// remembering its language and category and the order of its
    /// occurrence.
    fn collect(&mut self, raw: decorate::RawClass, source: &'static str, category: &'static str) {
        let key = raw.name.to_lowercase();
        self.order.push(key.clone());
        self.pending.insert(
            key,
            Pending {
                raw,
                source,
                category,
            },
        );
    }

    /// Resolves every collected class against the whole run and places
    /// the occurrences whose header carries a doomednum, in text order.
    fn finish(&mut self) {
        for key in self.order.clone() {
            self.place_occurrence(&key);
        }
    }

    /// Resolves one collected occurrence and places its row when the
    /// header carried a doomednum.
    fn place_occurrence(&mut self, key: &str) {
        let doomednum = self.pending.get(key).and_then(|entry| entry.raw.doomednum);
        let category = self
            .pending
            .get(key)
            .map_or(CATEGORY_DECORATE, |entry| entry.category);
        let Some(class) = self.resolve(key, &mut Vec::new()) else {
            return;
        };
        if let Some(kind) = doomednum {
            self.place(kind, &class, category);
        }
    }

    /// The resolved class named by the lowercase `key`, memoized once
    /// found. `visiting` holds the chain being resolved and breaks an
    /// inheritance cycle at its second visit; `None` answers only for a
    /// key nothing was collected under.
    fn resolve(&mut self, key: &str, visiting: &mut Vec<String>) -> Option<ResolvedClass> {
        if let Some(done) = self.classes.get(key) {
            return Some(done.clone());
        }
        if visiting.iter().any(|seen| seen == key) {
            return None;
        }
        let pending = self.pending.get(key)?.clone();
        visiting.push(key.to_owned());
        let parent = self.parent_class(pending.raw.parent.as_deref(), visiting);
        visiting.pop();
        let class = inherit(&pending, parent.as_ref());
        self.classes.insert(key.to_owned(), class.clone());
        Some(class)
    }

    /// The values a named parent supplies: another collected class,
    /// resolved wherever it stands, else the built-in row whose name is
    /// the parent's, else nothing.
    fn parent_class(
        &mut self,
        parent: Option<&str>,
        visiting: &mut Vec<String>,
    ) -> Option<ResolvedClass> {
        let name = parent?;
        let lower = name.to_lowercase();
        if self.pending.contains_key(&lower) {
            return self.resolve(&lower, visiting);
        }
        self.builtin_parent(&lower)
    }

    /// The built-in row named `name` (case-insensitively), read for the
    /// sprite, frame and radius a stock class would pass down.
    fn builtin_parent(&self, name: &str) -> Option<ResolvedClass> {
        self.defs.thing_by_name(name).map(|row| ResolvedClass {
            name: row.name.clone(),
            sprite: row.sprite.clone(),
            frame: row.frame,
            radius: row.radius,
            source: SOURCE_BUILTIN,
        })
    }

    /// The thing a `DoomEdNums` entry maps `number` to: the class's sprite
    /// and radius, or a name-only row when the class was never parsed.
    fn map_entry(&mut self, entry: &mapinfo::DoomEdEntry) {
        let class = self
            .classes
            .get(&entry.class.to_lowercase())
            .cloned()
            .unwrap_or_else(|| {
                let message = format!(
                    "{MAPINFO_LABEL}: DoomEdNums {} names unknown class {:?}",
                    entry.number, entry.class
                );
                self.defs.warnings.push(message);
                ResolvedClass {
                    name: entry.class.clone(),
                    sprite: String::new(),
                    frame: DEFAULT_FRAME,
                    radius: DEFAULT_RADIUS,
                    source: SOURCE_MAPINFO,
                }
            });
        self.place(entry.number, &class, CATEGORY_MAPINFO);
    }

    /// Inserts the thing row for `class` at type `kind`, replacing any row
    /// that already uses the number.
    fn place(&mut self, kind: u32, class: &ResolvedClass, category: &str) {
        let def = ThingDef {
            kind,
            name: class.name.clone(),
            sprite: class.sprite.clone(),
            frame: class.frame,
            radius: class.radius,
            category: category.to_owned(),
            group: Category::parse(category),
            hanging: false,
            game: self.game.to_owned(),
            source: class.source.to_owned(),
        };
        self.defs.put_thing(def);
    }
}

/// The editing definitions for `game`: the built-in tables with the custom
/// definitions from `texts` overlaid on top.
///
/// Precedence: built-in < DEHACKED < DECORATE/`ZScript` (+ MAPINFO). The
/// result's `warnings` carry the built-in fallback notes plus everything the
/// custom parsers refused to read.
///
/// # Errors
///
/// [`definitions_for`]'s [`CyberError::Internal`] for malformed embedded
/// data and the DEHACKED patcher's, for a malformed embedded mobj table.
#[inline]
pub fn session_definitions(game: Game, texts: &CustomSources) -> Result<Definitions, CyberError> {
    let mut defs = definitions_for(game)?;
    for text in &texts.dehacked {
        dehacked::apply(text, game, &mut defs)?;
    }
    let mut overlay = Overlay::new(&mut defs, game);
    for text in &texts.decorate {
        let (classes, messages) = decorate::parse_decorate(text);
        overlay.warn(messages);
        for class in classes {
            overlay.collect(class, SOURCE_DECORATE, CATEGORY_DECORATE);
        }
    }
    for text in &texts.zscript {
        let (classes, messages) = zscript::parse(text);
        overlay.warn(messages);
        for class in classes {
            overlay.collect(class, SOURCE_ZSCRIPT, CATEGORY_DECORATE);
        }
    }
    overlay.finish();
    for text in &texts.mapinfo {
        let (entries, messages) = mapinfo::doom_ed_nums(text);
        overlay.warn(messages);
        for entry in entries {
            overlay.map_entry(&entry);
        }
    }
    drop(overlay);
    defs.things.sort_by_key(|row| row.kind);
    defs.reindex_things();
    Ok(defs)
}

#[cfg(test)]
mod tests {
    use super::{CustomSources, session_definitions};
    use crate::defs::registry::definitions_for;
    use crate::wad::game::Game;

    /// The definitions for DOOM II overlaid with `texts`.
    fn overlaid(texts: &CustomSources) -> crate::defs::Definitions {
        session_definitions(Game::Doom2, texts).expect("the embedded tables are valid")
    }

    /// The built-in DOOM II table without overlays.
    fn builtin() -> crate::defs::Definitions {
        definitions_for(Game::Doom2).expect("the embedded tables are valid")
    }

    #[test]
    fn no_custom_sources_reproduce_the_built_in_table() {
        let defs = overlaid(&CustomSources::default());
        assert_eq!(defs.things.len(), 123, "the DOOM II set");
        assert_eq!(
            defs.thing(3001).expect("imp").source,
            "builtin",
            "untouched"
        );
        assert!(defs.warnings.is_empty(), "no notes: {:?}", defs.warnings);
    }

    #[test]
    fn dehacked_moves_the_imp_and_decorate_overrides_the_new_number() {
        let texts = CustomSources {
            dehacked: vec!["Thing 12\nID # = 20000\n".to_owned()],
            decorate: vec!["actor ImpActor 20000 { radius 30 }".to_owned()],
            ..CustomSources::default()
        };
        let defs = overlaid(&texts);
        assert!(
            defs.thing(3001).is_none(),
            "the built-in Imp number is freed"
        );
        let imp = defs.thing(20_000).expect("20000 exists");
        assert_eq!(imp.name, "ImpActor", "DECORATE outranks DEHACKED");
        assert_eq!(imp.source, "decorate", "and says so");
        assert_eq!(imp.radius, 30, "its own radius");
        assert_eq!(imp.category, "Custom (DECORATE)", "the decorate category");
    }

    #[test]
    fn parents_provide_sprites_and_radii_transitively() {
        let texts = CustomSources {
            decorate: vec![
                "actor Base { radius 24 states { spawn: TONK A 1 } }".to_owned(),
                "actor Middle : Base { } actor Top : Middle 20020 { }".to_owned(),
            ],
            ..CustomSources::default()
        };
        let defs = overlaid(&texts);
        let top = defs.thing(20_020).expect("the grandchild is a thing");
        assert_eq!(top.sprite, "TONK", "the sprite came down the chain");
        assert_eq!(top.frame, 'A', "with the frame");
        assert_eq!(top.radius, 24, "and the radius");
        assert_eq!(top.source, "decorate", "still decorate");
    }

    #[test]
    fn a_parent_defined_later_still_provides_its_values() {
        let texts = CustomSources {
            decorate: vec![
                "actor Descendant : Ancestor 20031 { }".to_owned(),
                "actor Ancestor { radius 44 states { spawn: TONK E 1 } }".to_owned(),
            ],
            ..CustomSources::default()
        };
        let defs = overlaid(&texts);
        let child = defs
            .thing(20_031)
            .expect("the forward reference is a thing");
        assert_eq!(child.sprite, "TONK", "the later parent's sprite");
        assert_eq!(child.frame, 'E', "and its frame");
        assert_eq!(child.radius, 44, "and its radius");
    }

    #[test]
    fn a_stock_parent_supplies_the_built_in_row_s_values() {
        let texts = CustomSources {
            decorate: vec!["actor CacoEcho : Cacodemon 20030 { }".to_owned()],
            ..CustomSources::default()
        };
        let defs = overlaid(&texts);
        let echo = defs.thing(20_030).expect("the actor is a thing");
        assert_eq!(echo.sprite, "HEAD", "the built-in Cacodemon sprite");
        assert_eq!(echo.frame, 'A', "its frame");
        assert_eq!(echo.radius, 32, "and its radius");
        assert_eq!(echo.source, "decorate", "the child keeps its own language");
    }

    #[test]
    fn parents_cross_the_language_and_order_boundaries() {
        let texts = CustomSources {
            decorate: vec![
                "actor Fancy : Echo 20033 { }".to_owned(),
                "actor Base { radius 22 states { spawn: TONK B 1 } }".to_owned(),
                "actor Derived : Base { }".to_owned(),
            ],
            zscript: vec!["class Echo : Derived { Default { Speed 5; } }".to_owned()],
            ..CustomSources::default()
        };
        let defs = overlaid(&texts);
        let fancy = defs.thing(20_033).expect("the actor is a thing");
        assert_eq!(
            fancy.sprite, "TONK",
            "a DECORATE parent may name a ZScript class"
        );
        assert_eq!(fancy.frame, 'B', "inherited down the whole chain");
        assert_eq!(fancy.radius, 22, "across the text boundary");
        assert_eq!(fancy.source, "decorate", "the child keeps its own language");
    }

    #[test]
    fn an_inheritance_cycle_breaks_at_its_second_visit() {
        let texts = CustomSources {
            decorate: vec![
                "actor LoopA : LoopB 20033 { } actor LoopB : LoopA { radius 9 }".to_owned(),
            ],
            ..CustomSources::default()
        };
        let defs = overlaid(&texts);
        let looped = defs.thing(20_033).expect("the cyclic actor is placed");
        assert_eq!(looped.radius, 9, "the chain still carried what resolved");
        assert_eq!(looped.sprite, "", "the loop itself supplies nothing");
    }

    #[test]
    fn unknown_parents_default_to_the_ticket_values() {
        let texts = CustomSources {
            decorate: vec!["actor Orphan : Nowhere 20021 { }".to_owned()],
            ..CustomSources::default()
        };
        let defs = overlaid(&texts);
        let lone = defs.thing(20_021).expect("the actor is a thing");
        assert_eq!(lone.sprite, "", "no sprite to inherit");
        assert_eq!(lone.frame, 'A', "the default frame");
        assert_eq!(lone.radius, 20, "the default radius");
    }

    #[test]
    fn mapinfo_turns_zscript_classes_into_things() {
        let texts = CustomSources {
            zscript: vec![
                "class Torch : Actor { Default { Radius 10; } States { Spawn: TORC D 3 } }"
                    .to_owned(),
            ],
            mapinfo: vec!["DoomEdNums {\n 12345 = Torch\n 12346 = Missing\n}".to_owned()],
            ..CustomSources::default()
        };
        let defs = overlaid(&texts);
        let torch = defs.thing(12_345).expect("the class is mapped");
        assert_eq!(torch.name, "Torch", "class name");
        assert_eq!(torch.sprite, "TORC", "class sprite");
        assert_eq!(torch.frame, 'D', "class frame");
        assert_eq!(torch.radius, 10, "class radius");
        assert_eq!(torch.source, "zscript", "the defining language");
        assert_eq!(torch.category, "Custom (MAPINFO)", "placed by MAPINFO");
        let missing = defs
            .thing(12_346)
            .expect("the unknown class is placed anyway");
        assert_eq!(missing.name, "Missing", "name only");
        assert_eq!(missing.sprite, "", "nothing else is known");
        assert_eq!(missing.source, "mapinfo", "the row names its own stream");
        assert!(
            defs.warnings
                .iter()
                .any(|warning| warning.contains("unknown class")),
            "the unknown class warns: {:?}",
            defs.warnings
        );
    }

    #[test]
    fn things_stay_sorted_and_replace_built_in_numbers() {
        let texts = CustomSources {
            decorate: vec!["actor NewDoor 5 { }".to_owned()],
            ..CustomSources::default()
        };
        let defs = overlaid(&texts);
        assert_eq!(
            defs.things.len(),
            builtin().things.len(),
            "5 was replaced, not added"
        );
        let door = defs.thing(5).expect("type 5 exists");
        assert_eq!(door.name, "NewDoor", "the decorate actor holds the number");
        assert_eq!(door.source, "decorate", "over the Blue Keycard");
        let kinds: Vec<u32> = defs.things.iter().map(|row| row.kind).collect();
        let mut sorted = kinds.clone();
        sorted.sort_unstable();
        assert_eq!(kinds, sorted, "the table arrives sorted");
    }

    #[test]
    fn parser_warnings_travel_with_the_report() {
        let texts = CustomSources {
            decorate: vec!["actor { radius }".to_owned()],
            mapinfo: vec!["DoomEdNums {\n broken line\n}".to_owned()],
            ..CustomSources::default()
        };
        let defs = overlaid(&texts);
        assert_eq!(
            defs.warnings.len(),
            2,
            "both parsers warn: {:?}",
            defs.warnings
        );
        assert!(
            defs.warnings
                .iter()
                .all(|warning| warning.starts_with("decorate:") || warning.starts_with("mapinfo:")),
            "warnings name their parser: {:?}",
            defs.warnings
        );
    }

    #[test]
    fn fallback_games_keep_their_note_under_the_overlay() {
        let texts = CustomSources {
            decorate: vec!["actor Lamp 99999 { radius 6 }".to_owned()],
            ..CustomSources::default()
        };
        let defs = session_definitions(Game::Strife, &texts).expect("strife falls back");
        assert!(
            defs.warnings.first().is_some_and(|w| w.contains("DOOM II")),
            "the fallback note stays: {:?}",
            defs.warnings
        );
        assert!(
            defs.thing(99_999).is_some(),
            "and the custom thing is there"
        );
    }
}
