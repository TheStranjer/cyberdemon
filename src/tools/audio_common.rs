//! The machinery the `audio` and `music` groups share: one [`MediaFamily`]
//! rule per vanilla lump family (the engine's name prefix, the prefix-aware
//! derived lump name and the one undoable replace-or-append store of a
//! global media lump) plus [`publish_playable`], the step both previews end
//! with. The format-specific decoders and converters stay in
//! [`crate::tools::audio`] and [`crate::tools::music`].

use crate::assets::fetch::derive_lump_name;
use crate::assets::server::AssetRef;
use crate::assets::store::mime_for_extension;
use crate::error::CyberError;
use crate::session::Session;
use crate::state::AppState;
use crate::wad::archive::{Lump, Wad};
use crate::wad::name::{LumpName, MAX_NAME_LEN};

/// One vanilla lump family's import rule: the prefix its engine looks names
/// up under and the word every undo label and refusal speaks the family
/// under (`import_music` / `import_sound`, "cannot derive a music name").
#[derive(Clone, Copy, Debug)]
pub(super) struct MediaFamily {
    /// The engine's lump-name prefix: `D_` for music, `DS` for sounds.
    prefix: &'static str,
    /// The family word: `music` or `sound`.
    word: &'static str,
}

impl MediaFamily {
    /// One family's rule from its `prefix` and `word`.
    const fn new(prefix: &'static str, word: &'static str) -> Self {
        Self { prefix, word }
    }

    /// Characters a derived stem keeps once the prefix took its share of
    /// the eight-byte lump-name field.
    const fn head(&self) -> usize {
        MAX_NAME_LEN.saturating_sub(self.prefix.len())
    }

    /// The name to store under: the explicit one; else a derived stem that
    /// already carries the prefix, kept whole so `D_RUNNIN.mid` stores as
    /// `D_RUNNIN` rather than doubling into `D_D_RUNN`; else the prefix
    /// plus the stem trimmed to [`Self::head`] characters.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] asking for `name` when nothing usable
    /// can be derived.
    pub(super) fn choose_name(
        &self,
        explicit: Option<LumpName>,
        filename: &str,
    ) -> Result<LumpName, CyberError> {
        if let Some(name) = explicit {
            return Ok(name);
        }
        let stem = derive_lump_name(filename).ok_or_else(|| {
            CyberError::InvalidArgument(format!(
                "cannot derive a {} name from {filename:?}: pass name (1 to {MAX_NAME_LEN} \
                 printable ASCII characters)",
                self.word
            ))
        })?;
        let text = stem.to_string();
        if text.starts_with(self.prefix) {
            return Ok(stem);
        }
        let head: String = text.chars().take(self.head()).collect();
        LumpName::parse(&format!("{}{head}", self.prefix))
    }

    /// One undoable insert (or in-place replace) of a family lump in the
    /// global area of the working archive, labelled `import_<word> NAME`.
    ///
    /// # Errors
    ///
    /// [`CyberError::AlreadyExists`] for a working lump of the same name
    /// when `replace` is `false`.
    pub(super) fn store(
        &self,
        session: &mut Session,
        lump: MediaLump,
        replace: bool,
    ) -> Result<(), CyberError> {
        let word = self.word;
        let label = format!("import_{word} {}", lump.name);
        session.edit(&label, |wad| place_last(wad, lump.name, lump.data, replace))
    }
}

/// The lump one import commits: the chosen name and its payload bytes.
pub(super) struct MediaLump {
    /// The family-prefixed name the payload is stored under.
    pub(super) name: LumpName,
    /// The payload bytes.
    pub(super) data: Vec<u8>,
}

/// The `music` family: vanilla `Doom`'s music player looks every song up as
/// `D_<name>` with no `MAPINFO` entry.
pub(super) const MUSIC: MediaFamily = MediaFamily::new("D_", "music");

/// The `sound` family: vanilla `Doom`'s sound table looks every sound up as
/// `DS<name>` with no `SNDINFO` entry.
pub(super) const SOUND: MediaFamily = MediaFamily::new("DS", "sound");

/// Put the lump at the end of `wad`, replacing the last same-name lump when
/// one exists and `replace` allows it. Media lumps have no namespace.
///
/// # Errors
///
/// [`CyberError::AlreadyExists`] as by [`MediaFamily::store`].
fn place_last(
    wad: &mut Wad,
    name: LumpName,
    data: Vec<u8>,
    replace: bool,
) -> Result<(), CyberError> {
    if wad.find_last(name).is_none() {
        wad.lumps.push(Lump::new(name, data));
        return Ok(());
    }
    if !replace {
        return Err(CyberError::AlreadyExists(format!(
            "lump {name} already exists in the working archive: pass replace true to overwrite \
             it or choose another name"
        )));
    }
    if let Some(slot) = wad.lumps.iter_mut().rev().find(|lump| lump.name == name) {
        *slot = Lump::new(name, data);
    }
    Ok(())
}

/// Publish one preview's file as `NAME.<extension>` on the localhost asset
/// server, its media type taken from the extension.
///
/// # Errors
///
/// [`AppState::publish`]' errors for the asset store.
pub(super) fn publish_playable(
    state: &AppState,
    name: LumpName,
    extension: &str,
    data: Vec<u8>,
) -> Result<AssetRef, CyberError> {
    let filename = format!("{name}.{extension}");
    let mime = mime_for_extension(extension);
    state.publish(&filename, mime, data)
}

#[cfg(test)]
mod tests {
    use super::{MUSIC, SOUND, place_last};
    use crate::wad::archive::{Lump, Wad};
    use crate::wad::name::LumpName;

    /// The name `name`, for the placement tests.
    fn lump_name(name: &str) -> LumpName {
        LumpName::parse(name).expect("a valid test name")
    }

    /// A `wad` holding two same-name lumps: `first` underneath `second`.
    fn wad_with_twins(name: &str, first: &[u8], second: &[u8]) -> Wad {
        let mut wad = Wad::default();
        wad.lumps.push(Lump::new(lump_name(name), first.to_vec()));
        wad.lumps.push(Lump::new(lump_name(name), second.to_vec()));
        wad
    }

    #[test]
    fn the_music_rule_defaults_to_the_prefix_plus_a_trimmed_stem() {
        assert_eq!(
            MUSIC
                .choose_name(None, "runnin.mid")
                .expect("derivable")
                .to_string(),
            "D_RUNNIN",
            "the ticket's example"
        );
        assert_eq!(
            MUSIC
                .choose_name(None, "E1M2 theme.mid")
                .expect("derivable")
                .to_string(),
            "D_E1M2TH",
            "space dropped, stem trimmed to six"
        );
    }

    #[test]
    fn a_stem_already_carrying_its_family_prefix_is_kept_whole() {
        assert_eq!(
            MUSIC
                .choose_name(None, "D_RUNNIN.mid")
                .expect("derivable")
                .to_string(),
            "D_RUNNIN",
            "no doubled D_ prefix"
        );
        assert_eq!(
            MUSIC
                .choose_name(None, "D_RUNNINLONGER.mid")
                .expect("derivable")
                .to_string(),
            "D_RUNNIN",
            "the eight-byte stem, not its first six characters"
        );
        assert_eq!(
            MUSIC
                .choose_name(None, "d_run.mid")
                .expect("derivable")
                .to_string(),
            "D_RUN",
            "a short prefixed stem stays short"
        );
        assert_eq!(
            SOUND
                .choose_name(None, "DSPISTOL.wav")
                .expect("derivable")
                .to_string(),
            "DSPISTOL",
            "no doubled DS prefix"
        );
        assert_eq!(
            SOUND
                .choose_name(None, "dsdoorsw.ogg")
                .expect("derivable")
                .to_string(),
            "DSDOORSW",
            "the prefix is matched case-insensitively because the stem is uppercased first"
        );
    }

    #[test]
    fn the_sound_rule_still_prepends_to_a_bare_stem() {
        assert_eq!(
            SOUND
                .choose_name(None, "pistol.wav")
                .expect("derivable")
                .to_string(),
            "DSPISTOL",
            "DS + the stem"
        );
        assert_eq!(
            SOUND
                .choose_name(None, "very long sound name.wav")
                .expect("derivable")
                .to_string(),
            "DSVERYLO",
            "DS plus the stem's first six usable characters"
        );
    }

    #[test]
    fn an_explicit_name_wins_over_derivation() {
        let explicit = LumpName::parse("MYTUNE").ok();
        assert_eq!(
            MUSIC
                .choose_name(explicit, "runnin.mid")
                .expect("explicit")
                .to_string(),
            "MYTUNE",
            "the music rule"
        );
        assert_eq!(
            SOUND
                .choose_name(explicit, "DSPISTOL.wav")
                .expect("explicit")
                .to_string(),
            "MYTUNE",
            "and the sound rule, past its own prefix"
        );
    }

    #[test]
    fn an_underivable_stem_refuses_naming_its_family() {
        let error = MUSIC.choose_name(None, ".mid").expect_err("nothing usable");
        assert_eq!(error.code(), "invalid_argument", "unnameable code");
        assert!(
            error.to_string().contains("music name") && error.to_string().contains("pass name"),
            "the message names the family and the fix: {error}"
        );
        let sound_error = SOUND.choose_name(None, ".wav").expect_err("nothing usable");
        assert!(
            sound_error.to_string().contains("sound name"),
            "the sound family says so: {sound_error}"
        );
    }

    #[test]
    fn placement_appends_a_fresh_lump() {
        let mut wad = Wad::default();
        place_last(&mut wad, lump_name("D_RUNNIN"), b"mus".to_vec(), false).expect("append");
        assert_eq!(wad.lumps.len(), 1, "one lump");
        assert_eq!(wad.lumps[0].data.as_ref(), b"mus".as_slice(), "at the end");
    }

    #[test]
    fn placement_refuses_a_twin_until_replace_allows_it() {
        let mut wad = wad_with_twins("DSPISTOL", b"old", b"new");
        let error = place_last(&mut wad, lump_name("DSPISTOL"), b"newer".to_vec(), false)
            .expect_err("same-name working lumps");
        assert_eq!(error.code(), "already_exists", "twin code");
        assert!(
            error.to_string().contains("replace true"),
            "the message points at the fix: {error}"
        );
        assert_eq!(wad.lumps.len(), 2, "the refusal changed nothing");
        assert_eq!(
            wad.lumps[1].data.as_ref(),
            b"new".as_slice(),
            "and left the payload"
        );
        place_last(&mut wad, lump_name("DSPISTOL"), b"newest".to_vec(), true).expect("replace");
        assert_eq!(wad.lumps.len(), 2, "the replace stays in place");
        assert_eq!(
            wad.lumps[1].data.as_ref(),
            b"newest".as_slice(),
            "the last copy swapped"
        );
        assert_eq!(
            wad.lumps[0].data.as_ref(),
            b"old".as_slice(),
            "the earlier copy untouched"
        );
    }

    #[test]
    fn replacement_of_a_lone_lump_leaves_one_copy() {
        let mut wad = Wad::default();
        place_last(&mut wad, lump_name("D_INTER"), b"old".to_vec(), false).expect("append");
        place_last(&mut wad, lump_name("D_INTER"), b"new".to_vec(), true).expect("replace");
        assert_eq!(
            wad.lumps.len(),
            1,
            "still exactly one copy in the directory"
        );
        assert_eq!(
            wad.lumps[0].data.as_ref(),
            b"new".as_slice(),
            "the payload swapped"
        );
    }
}
