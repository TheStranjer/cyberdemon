//! The `ZScript` parser (subset): `class` headers with `Default` radius and
//! `States` spawn sprite.
//!
//! `ZScript` classes share the [`crate::defs::decorate`] scanner: same brace
//! tracking, same comment and string skipping. The differences the parser
//! applies here are the `class` keyword and the absence of doomednums in
//! headers — `ZScript` classes reach an editor's thing list only through a
//! MAPINFO `DoomEdNums` entry (see [`crate::defs::mapinfo`]). The `Default {
//! Radius <n>; }` block and the first `Spawn:` state are read exactly like
//! their DECORATE counterparts.

use crate::defs::decorate::RawClass;

/// Parses the `class` definitions of one `ZScript` lump text.
pub(crate) fn parse(text: &str) -> (Vec<RawClass>, Vec<String>) {
    crate::defs::decorate::parse(text, "class", "zscript", false)
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn a_class_keeps_name_radius_and_spawn_sprite() {
        let text = "class RevenantFlame : Monster \n{\n \
                    Default { Speed 10; Radius 32; Mass 500; }\n \
                    States { Spawn: RFLE A -1 Bright\n Pain: RFLE B 5\n }\n}";
        let (classes, warnings) = parse(text);
        assert!(warnings.is_empty(), "no warnings: {warnings:?}");
        assert_eq!(classes.len(), 1, "one class");
        let flame = classes.first().expect("one class");
        assert_eq!(flame.name, "RevenantFlame", "class name");
        assert_eq!(flame.parent.as_deref(), Some("Monster"), "colon parent");
        assert_eq!(flame.doomednum, None, "ZScript headers carry no doomednum");
        assert_eq!(flame.radius, Some(32), "the Default block radius");
        assert_eq!(flame.sprite.as_deref(), Some("RFLE"), "the spawn sprite");
        assert_eq!(flame.frame, Some('A'), "the first frame letter");
    }

    #[test]
    fn extends_names_the_parent_and_comments_are_skipped() {
        let text = "class Quiet extends Actor { /* Spawn: NOTHING A 1 */ }";
        let (classes, warnings) = parse(text);
        assert!(warnings.is_empty(), "no warnings: {warnings:?}");
        let quiet = classes.first().expect("one class");
        assert_eq!(quiet.parent.as_deref(), Some("Actor"), "extends parent");
        assert_eq!(quiet.sprite, None, "the commented-out state is not read");
    }
}
