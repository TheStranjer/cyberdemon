//! Session definitions: the built-in game tables overlaid with the custom
//! thing definitions of the loaded archives.
//!
//! [`session_definitions`] starts from [`crate::defs::registry::definitions_for`]
//! and applies, in precedence order (built-in < DEHACKED < DECORATE/`ZScript`,
//! with MAPINFO `DoomEdNums` completing the `ZScript` side):
//!
//! - [`crate::defs::dehacked`]: every `Thing <n>` block moves or edits the
//!   built-in row of its mobj;
//! - [`crate::defs::decorate`] and [`crate::defs::zscript`]: actors with a
//!   doomednum become things; classes resolve their parent's sprite and
//!   radius, else default to an empty sprite and radius 20;
//! - [`crate::defs::mapinfo`]: every `DoomEdNums` number becomes (or is
//!   replaced by) a thing named after its class.
//!
//! Every parser is tolerant: unparseable text produces warnings on the
//! result, never an error. The only error is the built-in table lookup's
//! [`crate::error::CyberError::Internal`] for malformed embedded data.

use alloc::collections::BTreeMap;

use crate::defs::registry::{Definitions, definitions_for, game_label};
use crate::defs::table::ThingDef;
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

/// The thing-table editor behind the class and `DoomEdNums` overlays.
struct Overlay<'defs> {
    /// The definitions being overlaid (things and warnings).
    defs: &'defs mut Definitions,
    /// The resolved classes, keyed by lowercase class name, last wins.
    classes: BTreeMap<String, ResolvedClass>,
    /// The wire label of the game, for synthesized rows.
    game: &'static str,
}

impl<'defs> Overlay<'defs> {
    /// A fresh overlay for `defs` of `game`.
    const fn new(defs: &'defs mut Definitions, game: Game) -> Self {
        Self {
            defs,
            classes: BTreeMap::new(),
            game: game_label(game),
        }
    }

    /// Appends parser notes to the report warnings.
    fn warn(&mut self, mut messages: Vec<String>) {
        self.defs.warnings.append(&mut messages);
    }

    /// Registers one parsed class: resolves inheritance, remembers it for
    /// MAPINFO and places a thing when the header carries a doomednum.
    fn register(&mut self, raw: decorate::RawClass, source: &'static str, category: &str) {
        let parent = raw
            .parent
            .as_deref()
            .and_then(|name| self.classes.get(&name.to_lowercase()))
            .cloned();
        let class = ResolvedClass {
            sprite: raw
                .sprite
                .or_else(|| parent.as_ref().map(|base| base.sprite.clone()))
                .unwrap_or_default(),
            frame: raw
                .frame
                .or_else(|| parent.as_ref().map(|base| base.frame))
                .unwrap_or(DEFAULT_FRAME),
            radius: raw
                .radius
                .or_else(|| parent.as_ref().map(|base| base.radius))
                .unwrap_or(DEFAULT_RADIUS),
            name: raw.name.clone(),
            source,
        };
        if let Some(kind) = raw.doomednum {
            self.place(kind, &class, category);
        }
        self.classes.insert(raw.name.to_lowercase(), class);
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
                    source: SOURCE_DECORATE,
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
            hanging: false,
            game: self.game.to_owned(),
            source: class.source.to_owned(),
        };
        self.defs.things.retain(|row| row.kind != def.kind);
        self.defs.things.push(def);
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
            overlay.register(class, SOURCE_DECORATE, CATEGORY_DECORATE);
        }
    }
    for text in &texts.zscript {
        let (classes, messages) = zscript::parse(text);
        overlay.warn(messages);
        for class in classes {
            overlay.register(class, SOURCE_ZSCRIPT, CATEGORY_DECORATE);
        }
    }
    for text in &texts.mapinfo {
        let (entries, messages) = mapinfo::doom_ed_nums(text);
        overlay.warn(messages);
        for entry in entries {
            overlay.map_entry(&entry);
        }
    }
    drop(overlay);
    defs.things.sort_by_key(|row| row.kind);
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
