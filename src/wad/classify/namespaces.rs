//! The marker names and the open-namespace tracker behind [`super::analyze`].
//!
//! Which marker opens and closes which [`Namespace`], which names count as
//! markers at all, and the `NamespaceState` a directory walk carries to know
//! what is open right now, warning about unclosed starts and stray ends.
//! [`super::analyze`] drives that state's `visit` once per lump and `finish`
//! at the end; `is_marker_name` is the check the map pass and the type-hint
//! pass share so a marker never reads as a map header or as content. The
//! [`Namespace`] enum itself stays in the root.

use crate::wad::name::LumpName;

use super::Namespace;

/// The namespace a start marker opens, `None` for any other name.
pub(crate) const fn namespace_start(name: &[u8]) -> Option<Namespace> {
    match name {
        b"S_START" | b"SS_START" => Some(Namespace::Sprites),
        b"F_START" | b"FF_START" => Some(Namespace::Flats),
        b"P_START" | b"PP_START" => Some(Namespace::Patches),
        b"C_START" => Some(Namespace::Colormaps),
        b"A_START" => Some(Namespace::Acs),
        b"V_START" => Some(Namespace::Voices),
        b"HI_START" => Some(Namespace::Hires),
        b"TX_START" => Some(Namespace::Textures),
        _ => None,
    }
}

/// The namespace an end marker closes, `None` for any other name.
pub(crate) const fn namespace_end(name: &[u8]) -> Option<Namespace> {
    match name {
        b"S_END" | b"SS_END" => Some(Namespace::Sprites),
        b"F_END" | b"FF_END" => Some(Namespace::Flats),
        b"P_END" | b"PP_END" => Some(Namespace::Patches),
        b"C_END" => Some(Namespace::Colormaps),
        b"A_END" => Some(Namespace::Acs),
        b"V_END" => Some(Namespace::Voices),
        b"HI_END" => Some(Namespace::Hires),
        b"TX_END" => Some(Namespace::Textures),
        _ => None,
    }
}

/// Whether `name` is any marker: a namespace boundary or an inner marker
/// like `F1_START` ending in `_START`/`_END`.
pub(super) fn is_marker_name(name: &[u8]) -> bool {
    namespace_start(name).is_some() || namespace_end(name).is_some() || is_inner_marker(name)
}

/// Whether `name` ends in `_START` or `_END` without being a namespace
/// boundary.
fn is_inner_marker(name: &[u8]) -> bool {
    name.ends_with(b"_START") || name.ends_with(b"_END")
}

/// Tracks the namespace currently open while walking the directory.
#[derive(Default)]
pub(super) struct NamespaceState {
    /// The open namespace with its start marker's index and name.
    open: Option<(Namespace, usize, LumpName)>,
}

impl NamespaceState {
    /// Classifies `name` at `index`: returns the namespace it sits in and
    /// updates the open state, warning about unclosed starts and stray ends.
    pub(super) fn visit(
        &mut self,
        name: LumpName,
        index: usize,
        warnings: &mut Vec<String>,
    ) -> Option<Namespace> {
        let bytes = name.as_bytes();
        if let Some(namespace) = namespace_start(bytes) {
            self.close(warnings);
            self.open = Some((namespace, index, name));
            return Some(namespace);
        }
        let Some(namespace) = namespace_end(bytes) else {
            return self.open.map(|(opened, _, _)| opened);
        };
        if self.open.is_some_and(|(opened, _, _)| opened == namespace) {
            self.open = None;
        } else {
            warnings.push(format!(
                "stray end marker {name} at index {index}: no {} namespace is open",
                namespace.as_str()
            ));
        }
        Some(namespace)
    }

    /// Ends the walk: an unclosed start runs to the directory end and warns.
    pub(super) fn finish(&mut self, warnings: &mut Vec<String>) {
        self.close(warnings);
    }

    /// Warns if a namespace is still open and closes it.
    fn close(&mut self, warnings: &mut Vec<String>) {
        if let Some((namespace, at, marker)) = self.open.take() {
            warnings.push(format!(
                "start marker {marker} at index {at} was never closed: the {} namespace \
                 runs to the end of the directory",
                namespace.as_str()
            ));
        }
    }
}
