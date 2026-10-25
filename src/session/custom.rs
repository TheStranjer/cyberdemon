//! Session building block: collecting the custom definition lump texts
//! from the session's merged view, behind [`crate::session::Session`]’s
//! [`custom_sources`](Session::custom_sources) and
//! [`definitions`](Session::definitions) reads.
//!
//! [`custom_sources`] gathers the texts [`crate::defs::session_definitions`]
//! overlays: every `DEHACKED`, `DECORATE` and `ZSCRIPT` lump and every
//! `MAPINFO`/`ZMAPINFO` lump of the merged view, in directory order. For
//! DECORATE and ZSCRIPT it follows `#include "NAME"` lines by lump name
//! (the merged view's last match wins), bounded to [`MAX_INCLUDE_DEPTH`]
//! nesting with every lump taken at most once. Include problems come back
//! as notes, never as errors.

use alloc::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::defs::CustomSources;
use crate::error::CyberError;
use crate::session::{MergedLump, Scope, Session};
use crate::wad::lumps;
use crate::wad::name::LumpName;

/// Deepest `#include` chain followed below a direct DECORATE/ZSCRIPT lump.
const MAX_INCLUDE_DEPTH: usize = 8;

/// The include directive, matched case-insensitively at a line's start.
const INCLUDE_DIRECTIVE: &str = "#include";

/// The line comment marker an include scan stops at.
const LINE_COMMENT: &str = "//";

/// Which custom source stream a lump name selects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stream {
    /// The `DEHACKED` text stream.
    Dehacked,
    /// The `DECORATE` text stream, includes joined.
    Decorate,
    /// The `ZSCRIPT` text stream, includes joined.
    Zscript,
    /// The `MAPINFO`/`ZMAPINFO` text stream.
    Mapinfo,
}

impl Stream {
    /// The stream's name in warning text.
    const fn label(self) -> &'static str {
        match self {
            Self::Dehacked => "DEHACKED",
            Self::Decorate => "DECORATE",
            Self::Zscript => "ZSCRIPT",
            Self::Mapinfo => "MAPINFO",
        }
    }
}

/// The stream a merged-view lump name selects, `None` for other names.
/// [`LumpName`] stores uppercase, so a constant equality is the same
/// case-insensitive word test the raw bytes went through.
fn stream_of(name: LumpName) -> Option<Stream> {
    if name == lumps::DEHACKED {
        return Some(Stream::Dehacked);
    }
    if name == lumps::DECORATE {
        return Some(Stream::Decorate);
    }
    if name == lumps::ZSCRIPT {
        return Some(Stream::Zscript);
    }
    if name == lumps::MAPINFO || name == lumps::ZMAPINFO {
        return Some(Stream::Mapinfo);
    }
    None
}

/// One queued include-following source lump.
struct Queued {
    /// The stream the lump feeds.
    stream: Stream,
    /// The lump's name.
    name: LumpName,
    /// Its include depth (`0` for a direct lump).
    depth: usize,
    /// The lump text, decoded lossily.
    text: String,
}

/// The merged view plus the collection running over it.
struct Collector {
    /// The merged view, in directory order.
    view: Vec<MergedLump>,
    /// The last position of every name: include lookups land here.
    lookup: BTreeMap<LumpName, usize>,
    /// Lumps already taken, so a cycle collects nothing twice.
    seen: BTreeSet<LumpName>,
    /// The queue of decorates/zscripts still to scan for includes.
    queue: VecDeque<Queued>,
    /// The texts collected so far.
    texts: CustomSources,
    /// Notes about includes that could not be followed.
    notes: Vec<String>,
}

impl Collector {
    /// A collector over one merged view.
    fn new(view: Vec<MergedLump>) -> Self {
        let mut lookup: BTreeMap<LumpName, usize> = BTreeMap::new();
        for (index, entry) in view.iter().enumerate() {
            lookup.insert(entry.lump.name, index);
        }
        Self {
            view,
            lookup,
            seen: BTreeSet::new(),
            queue: VecDeque::new(),
            texts: CustomSources::default(),
            notes: Vec::new(),
        }
    }

    /// The lump at `index` decoded as text.
    fn body(&self, index: usize) -> String {
        self.view.get(index).map_or_else(String::new, |entry| {
            String::from_utf8_lossy(&entry.lump.data).into_owned()
        })
    }

    /// Takes one source lump: `DEHACKED`/`MAPINFO` texts land directly in
    /// their streams, DECORATE/ZSCRIPT also queue for include following.
    fn take_root(&mut self, index: usize) {
        let Some(entry) = self.view.get(index) else {
            return;
        };
        let Some(stream) = stream_of(entry.lump.name) else {
            return;
        };
        let name = entry.lump.name;
        let body = self.body(index);
        match stream {
            Stream::Dehacked => self.texts.dehacked.push(body),
            Stream::Mapinfo => self.texts.mapinfo.push(body),
            Stream::Decorate | Stream::Zscript => {
                self.seen.insert(name);
                self.queue.push_back(Queued {
                    stream,
                    name,
                    depth: 0,
                    text: body,
                });
            }
        }
    }

    /// Empties the queue: each lump's text joins its stream, then the
    /// lumps it includes are appended, depth-bounded and deduplicated.
    fn drain(&mut self) {
        while let Some(item) = self.queue.pop_front() {
            self.drain_one(&item);
        }
    }

    /// One queued lump: its text, then the lumps it includes.
    fn drain_one(&mut self, item: &Queued) {
        self.push_text(item);
        let wanted = include_names(&item.text, item.stream, &mut self.notes);
        if wanted.is_empty() {
            return;
        }
        if item.depth >= MAX_INCLUDE_DEPTH {
            self.note_depth_cut(item, wanted.len());
            return;
        }
        for name in wanted {
            self.follow(item, name);
        }
    }

    /// The note that a lump sits too deep for its includes to be read.
    fn note_depth_cut(&mut self, item: &Queued, count: usize) {
        let message = format!(
            "{}: lump {} reaches the include depth limit of {MAX_INCLUDE_DEPTH}; {count} \
             included lump(s) skipped",
            item.stream.label(),
            item.name
        );
        self.notes.push(message);
    }

    /// The texts collected for `stream`.
    const fn list(&mut self, stream: Stream) -> &mut Vec<String> {
        match stream {
            Stream::Dehacked => &mut self.texts.dehacked,
            Stream::Decorate => &mut self.texts.decorate,
            Stream::Zscript => &mut self.texts.zscript,
            Stream::Mapinfo => &mut self.texts.mapinfo,
        }
    }

    /// Appends one queued lump's text to its stream.
    fn push_text(&mut self, item: &Queued) {
        self.list(item.stream).push(item.text.clone());
    }

    /// Queues the included lump `name`, once, if the view holds it.
    fn follow(&mut self, from: &Queued, name: LumpName) {
        if !self.seen.insert(name) {
            return;
        }
        let Some(index) = self.lookup.get(&name).copied() else {
            let message = format!(
                "{}: {} includes lump {name}, which is not in the merged view",
                from.stream.label(),
                from.name
            );
            self.notes.push(message);
            return;
        };
        let text = self.body(index);
        self.queue.push_back(Queued {
            stream: from.stream,
            name,
            depth: from.depth.saturating_add(1),
            text,
        });
    }
}

/// The lump names one text's `#include "NAME"` lines reference; malformed
/// directives and names note themselves.
fn include_names(text: &str, stream: Stream, notes: &mut Vec<String>) -> Vec<LumpName> {
    let mut names: Vec<LumpName> = Vec::new();
    for line in text.lines() {
        let code = line
            .split_once(LINE_COMMENT)
            .map_or(line, |(head, _)| head)
            .trim_start();
        if !code.starts_with('#') {
            continue;
        }
        let lowered = code.to_ascii_lowercase();
        let Some(rest) = lowered.strip_prefix(INCLUDE_DIRECTIVE) else {
            continue;
        };
        if rest.trim().is_empty() {
            continue;
        }
        let Some(name) = rest.split('"').nth(1) else {
            let message = format!(
                "{}: #include without a quoted lump name: {code:?}",
                stream.label()
            );
            notes.push(message);
            continue;
        };
        let Ok(parsed) = LumpName::parse(name) else {
            let message = format!(
                "{}: #include name {name:?} is not a valid lump name",
                stream.label()
            );
            notes.push(message);
            continue;
        };
        names.push(parsed);
    }
    names
}

/// The custom definition texts of the session's merged view, in load order
/// with DECORATE/`ZScript` `#include` lumps appended, plus notes about
/// includes that could not be followed.
///
/// # Errors
///
/// [`Session::lumps_in`]'s error for the merged scope (a poisoned lock can
/// not reach here; the merged scope itself never fails).
pub(super) fn custom_sources(
    session: &Session,
) -> Result<(CustomSources, Vec<String>), CyberError> {
    let view = session.lumps_in(Scope::Merged)?;
    let mut collector = Collector::new(view);
    let roots: Vec<usize> = collector
        .view
        .iter()
        .enumerate()
        .filter(|pair| stream_of(pair.1.lump.name).is_some())
        .map(|(index, _)| index)
        .collect();
    for index in roots {
        collector.take_root(index);
    }
    collector.drain();
    Ok((collector.texts, collector.notes))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{MAX_INCLUDE_DEPTH, Stream, custom_sources, include_names};
    use crate::defs::CustomSources;
    use crate::session::{Layer, LayerRole, Session};
    use crate::wad::archive::{Lump, Wad, WadKind};
    use crate::wad::name::LumpName;

    /// A parsed test lump name.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// A lump named `text` carrying `text` as its payload.
    fn lump(text: &str) -> Lump {
        lump_named(text, text)
    }

    /// A lump named `label` carrying `body`.
    fn lump_named(label: &str, body: &str) -> Lump {
        Lump::new(name(label), body.as_bytes().to_vec())
    }

    /// A layer of `lumps` at `path`.
    fn layer(path: &str, role: LayerRole, lumps: Vec<Lump>) -> Layer {
        Layer::new(
            PathBuf::from(path),
            role,
            Wad {
                kind: WadKind::Pwad,
                lumps,
            },
        )
    }

    /// A session: the base layer holds `base`, the resource `extra`.
    fn session(base: Vec<Lump>, extra: Vec<Lump>) -> Session {
        let mut session = Session::default();
        session.set_iwad(layer("base.wad", LayerRole::Iwad, base));
        session.add_resource(layer("extra.wad", LayerRole::Resource, extra));
        session
    }

    /// The collected streams of a bare base over `extra`.
    fn collect(extra: Vec<Lump>) -> (CustomSources, Vec<String>) {
        custom_sources(&session(vec![lump("PLAYPAL")], extra)).expect("the merged scope lists")
    }

    #[test]
    fn every_named_lump_joins_its_stream_in_view_order() {
        let (texts, notes) = collect(vec![
            lump("DEHACKED"),
            lump("DECORATE"),
            lump("MAPINFO"),
            lump("ZSCRIPT"),
            lump("ZMAPINFO"),
            lump("OTHERLUM"),
        ]);
        assert_eq!(texts.dehacked.len(), 1, "one dehacked text");
        assert_eq!(texts.decorate.len(), 1, "one decorate text");
        assert_eq!(texts.zscript.len(), 1, "one zscript text");
        assert_eq!(texts.mapinfo.len(), 2, "MAPINFO and ZMAPINFO both");
        assert!(notes.is_empty(), "nothing to note: {notes:?}");
    }

    #[test]
    fn includes_are_followed_by_lump_name_to_the_last_match() {
        let base = vec![lump("PLAYPAL"), lump_named("EXTRA1", "from the base")];
        let extra = vec![
            lump_named("DECORATE", "#include \"EXTRA1\"\nactor Root 1 { }"),
            lump_named("EXTRA1", "from the resource"),
        ];
        let (texts, notes) = custom_sources(&session(base, extra)).expect("the merged scope lists");
        assert!(notes.is_empty(), "nothing to note: {notes:?}");
        assert_eq!(texts.decorate.len(), 2, "the root and the included lump");
        let included = texts.decorate.get(1).expect("the included text");
        assert_eq!(included, "from the resource", "the last merged match won");
    }

    #[test]
    fn an_include_cycle_collects_each_lump_once() {
        let extra = vec![
            lump_named("DECORATE", "#include \"DECB\""),
            lump_named("DECB", "actor B 2 { }\n#include \"DECA\""),
            lump_named("DECA", "actor A 1 { }\n#include \"DECB\""),
        ];
        let (texts, notes) = collect(extra);
        assert_eq!(texts.decorate.len(), 3, "root, DECB and DECA once each");
        assert!(notes.is_empty(), "a cycle ends silently: {notes:?}");
    }

    #[test]
    fn include_depth_is_capped_and_noted() {
        let mut extra: Vec<Lump> = vec![lump_named("DECORATE", "#include \"INC1\"")];
        for step in 1_usize..=MAX_INCLUDE_DEPTH.saturating_add(1) {
            let label = format!("INC{step}");
            let body = format!("#include \"INC{}\"", step.saturating_add(1));
            extra.push(lump_named(label.as_str(), body.as_str()));
        }
        let (texts, notes) = collect(extra);
        let expected = MAX_INCLUDE_DEPTH.saturating_add(1);
        assert_eq!(
            texts.decorate.len(),
            expected,
            "the root plus INC1 to INC8, the limit"
        );
        assert_eq!(notes.len(), 1, "the cut is noted: {notes:?}");
        assert!(
            notes.first().expect("one note").contains("depth limit"),
            "the note names the limit: {notes:?}"
        );
    }

    #[test]
    fn a_missing_include_notes_itself() {
        let (texts, notes) = collect(vec![lump_named("ZSCRIPT", "#include \"NOWAY\"")]);
        assert_eq!(texts.zscript.len(), 1, "only the root itself");
        assert_eq!(notes.len(), 1, "the miss is noted: {notes:?}");
        assert!(
            notes
                .first()
                .expect("one note")
                .contains("not in the merged view"),
            "the note says what is missing: {notes:?}"
        );
    }

    #[test]
    fn the_include_directive_is_line_based_and_case_insensitive() {
        let mut notes: Vec<String> = Vec::new();
        let wanted = include_names(
            "  #INCLUDE \"QUOTED\" text\n#include uncited here\n#include \"BAD NAME\"\n// #include \
             \"SKIPPED\"\n",
            Stream::Decorate,
            &mut notes,
        );
        assert_eq!(wanted, vec![name("QUOTED")], "only the quoted valid name");
        assert_eq!(notes.len(), 2, "both bad directives note: {notes:?}");
    }
}
