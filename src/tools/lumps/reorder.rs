//! Moving a contiguous run of lumps inside the working archive:
//! `reorder_lumps`.
//!
//! The step takes the run out of the directory and puts it back where the
//! shared [`crate::tools::lumps::place`] helper says, the destination resolved
//! against the directory the run has *left*: `index` counts positions without
//! the moved lumps, `before`/`after` name the last remaining lump of that name
//! and `namespace` is the region as it stands once the run is gone. Anchoring
//! a run on one of its own lumps is refused, because placing something beside
//! itself has no meaning. Unlike the inserting tools this tool has no append:
//! a destination is required.
//!
//! Selection follows the other editing tools: it reads the **working archive
//! only** (a lump a read-only layer owns cannot move, only be overridden), by
//! name with the `from_namespace` and `occurrence` refinements or by
//! `from_index`, plus `count` for a longer run (e.g. a whole map block).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CyberError;
use crate::session::Session;
use crate::tools::lumps::edit::{EditSelector, edit_selector, region_text, working_match};
use crate::tools::lumps::place::{
    Fallback, Placement, PlacementParams, insert_run, parse_placement,
};
use crate::wad::archive::{Lump, Wad};
use crate::wad::name::LumpName;

/// Arguments for [`Cyberdemon::reorder_lumps`](crate::server::Cyberdemon):
/// exactly one of `name` and `from_index` says what moves, the shared
/// placement fields say where it goes and one of them is required.
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ReorderLumpsParams {
    /// First lump of the run to move, resolved in the **working** archive
    /// only: 1 to 8 printable `ASCII` characters, matched case-insensitively.
    /// Give exactly one of `name` and `from_index`.
    #[serde(default)]
    pub(super) name: Option<String>,
    /// Keep only `name` matches inside this namespace region (`sprites`,
    /// `flats`, ...) when picking the run; without it any region and the loose
    /// lumps count. This refines `name` only: the destination region is
    /// `namespace`.
    #[serde(default)]
    pub(super) from_namespace: Option<String>,
    /// 0-based index among the working archive's `name` matches; the default
    /// is the last one. Needs `name`.
    #[serde(default)]
    pub(super) occurrence: Option<usize>,
    /// Directory position in the working archive the run starts at. Give
    /// exactly one of `from_index` and `name`.
    #[serde(default)]
    pub(super) from_index: Option<usize>,
    /// How many contiguous lumps to move, starting at the selected one
    /// (default 1): a whole map block is `name` `MAP01` with `count` 11.
    #[serde(default)]
    pub(super) count: Option<usize>,
    /// Where the run goes: exactly one of `index`, `before`, `after` or
    /// `namespace`, each resolved **after** the run has been taken out. There
    /// is no append default here.
    #[serde(flatten)]
    pub(super) placement: PlacementParams,
}

/// Where the moved run starts, as the caller named it.
#[derive(Clone, Copy, Debug)]
enum Source {
    /// The working-archive lump this selector picks starts the run.
    Named(EditSelector),
    /// The run starts at this directory position.
    At(usize),
}

/// A validated `reorder_lumps` call: the run it names and where it goes.
#[derive(Debug)]
struct Plan {
    /// What picks the run's first lump.
    source: Source,
    /// How many contiguous lumps move, at least one.
    count: usize,
    /// Where the run goes once it is out.
    placement: Placement,
}

/// The run as the working archive holds it: where it starts and what it
/// carries, resolved before anything moves.
#[derive(Debug)]
struct Located {
    /// Directory position of the run's first lump.
    start: usize,
    /// The run's names, in directory order.
    names: Vec<LumpName>,
}

/// Structured result of [`Cyberdemon::reorder_lumps`](crate::server::Cyberdemon).
#[derive(Debug, Serialize)]
pub(super) struct ReorderReport {
    /// The moved lumps' names, in directory order.
    moved: Vec<LumpName>,
    /// The index the run started at before it moved.
    from_index: usize,
    /// The index the run's first lump sits at now.
    to_index: usize,
}

/// Validate `reorder_lumps`' arguments: a positive run length, one
/// destination and exactly one way of naming the run.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for `count` 0, both or neither of `name`
/// and `from_index`, `occurrence`/`from_namespace` beside `from_index`, more
/// than one placement field, a bad name or region, or no destination at all.
fn plan_reorder(args: &ReorderLumpsParams) -> Result<Plan, CyberError> {
    let count = args.count.unwrap_or(1_usize);
    if count == 0 {
        return Err(CyberError::InvalidArgument(
            "count 0 moves nothing: pass at least 1 lump (1 is the default)".to_owned(),
        ));
    }
    let placement = parse_placement(&args.placement)?;
    if placement.is_empty() {
        return Err(CyberError::InvalidArgument(
            "reorder_lumps needs a destination: pass one of index, before, after or namespace \
             (it has no append default)"
                .to_owned(),
        ));
    }
    Ok(Plan {
        source: plan_source(args)?,
        count,
        placement,
    })
}

/// The run's source: exactly one of `name`, with its own refinements, and
/// `from_index`.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for both or neither, for the refinements
/// `from_index` cannot use, or for a name or region that does not parse.
fn plan_source(args: &ReorderLumpsParams) -> Result<Source, CyberError> {
    match (args.name.as_deref(), args.from_index) {
        (Some(_), Some(_)) => Err(CyberError::InvalidArgument(
            "reorder_lumps takes exactly one of name and from_index: say what moves by name or \
             by position, not both"
                .to_owned(),
        )),
        (None, None) => Err(CyberError::InvalidArgument(
            "reorder_lumps needs a run to move: pass name (with optional occurrence and \
             from_namespace) or from_index"
                .to_owned(),
        )),
        (Some(text), None) => Ok(Source::Named(edit_selector(
            text,
            args.from_namespace.as_deref(),
            args.occurrence,
        )?)),
        (None, Some(index)) => {
            if args.occurrence.is_some() || args.from_namespace.is_some() {
                return Err(CyberError::InvalidArgument(
                    "occurrence and from_namespace refine name: drop them when the run starts at \
                     from_index"
                        .to_owned(),
                ));
            }
            Ok(Source::At(index))
        }
    }
}

/// The run's start position and names in the working archive.
///
/// # Errors
///
/// [`CyberError::NotFound`] when `name` selects nothing or `occurrence`
/// reaches past the matches, [`CyberError::InvalidArgument`] when `from_index`
/// or the run reaches past the end of the directory.
fn locate(session: &Session, plan: &Plan) -> Result<Located, CyberError> {
    let start = match plan.source {
        Source::Named(selector) => working_match(session, &selector)?
            .ok_or_else(|| CyberError::NotFound(not_movable_message(&selector)))?,
        Source::At(index) => index,
    };
    let wad = session.working();
    let end = run_end(start, plan.count, wad.len())?;
    Ok(Located {
        start,
        names: wad
            .lumps
            .get(start..end)
            .unwrap_or(&[])
            .iter()
            .map(|lump| lump.name)
            .collect(),
    })
}

/// One past the last lump of the run, checked against the directory.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when the run reaches past the end of a
/// `len`-lump archive (which covers a start equal to the length) or when its
/// end cannot be computed.
fn run_end(start: usize, count: usize, len: usize) -> Result<usize, CyberError> {
    let Some(end) = start.checked_add(count) else {
        return Err(CyberError::InvalidArgument(format!(
            "count {count} starting at index {start} overflows: pass a smaller run"
        )));
    };
    if end > len {
        let room = len.saturating_sub(start);
        return Err(CyberError::InvalidArgument(format!(
            "{count} lumps from index {start} run past the end of the {len}-lump working \
             archive: a run from {start} can hold at most {room}"
        )));
    }
    Ok(end)
}

/// Refuse a `before`/`after` destination that names one of the moved lumps.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when the anchor's name stands inside the
/// run, which would make the destination meaningless.
fn refuse_anchor_in_run(placement: &Placement, names: &[LumpName]) -> Result<(), CyberError> {
    let anchor = placement.before.or(placement.after);
    if let Some(named) = anchor {
        if names.contains(&named) {
            return Err(CyberError::InvalidArgument(format!(
                "the destination lump {named} is one of the moved lumps: pick a before/after \
                 anchor the run leaves behind, or place the run with index or namespace"
            )));
        }
    }
    Ok(())
}

/// The `not_found` text for a run the working archive does not hold.
fn not_movable_message(selector: &EditSelector) -> String {
    format!(
        "lump {} is not in {}: the IWAD and resource layers are read-only, so their lumps cannot \
         be moved; add_lump a working copy to move one",
        selector.name,
        region_text(selector.namespace)
    )
}

/// The undo label for a move: `reorder_lumps NAME`, naming the run's first
/// lump. A located run always holds at least one lump because `count` was
/// required positive, so the empty name only guards that promise.
fn reorder_label(names: &[LumpName]) -> String {
    names.first().map_or_else(
        || "reorder_lumps".to_owned(),
        |head| format!("reorder_lumps {head}"),
    )
}

/// The `count` lumps from `start`, taken out of the directory and leaving the
/// rest of it where it stands.
///
/// # Errors
///
/// [`run_end`]'s [`CyberError::InvalidArgument`] re-checked against the
/// archive that is being edited.
fn take_run(wad: &mut Wad, start: usize, count: usize) -> Result<Vec<Lump>, CyberError> {
    run_end(start, count, wad.len())?;
    let mut beyond = wad.lumps.split_off(start);
    let run: Vec<Lump> = beyond.drain(..count).collect();
    wad.lumps.extend(beyond);
    Ok(run)
}

/// The whole directory step: take the run out, then re-insert it where the
/// placement says, the destination resolved against the shortened directory.
///
/// # Errors
///
/// [`take_run`]'s and [`insert_run`]'s refusals.
fn move_run(
    wad: &mut Wad,
    start: usize,
    count: usize,
    placement: &Placement,
) -> Result<usize, CyberError> {
    let run = take_run(wad, start, count)?;
    insert_run(wad, placement, run, Fallback::Required)
}

/// Runs `reorder_lumps`: one undoable move inside the working archive.
///
/// # Errors
///
/// Whatever [`plan_reorder`], [`locate`] or [`move_run`] reports; a refusal
/// leaves the directory untouched.
pub(super) fn reorder_lumps(
    session: &mut Session,
    args: &ReorderLumpsParams,
) -> Result<ReorderReport, CyberError> {
    let plan = plan_reorder(args)?;
    let Located { start, names } = locate(session, &plan)?;
    refuse_anchor_in_run(&plan.placement, &names)?;
    let label = reorder_label(&names);
    let count = plan.count;
    let placement = plan.placement;
    let to_index = session.edit(&label, |wad| move_run(wad, start, count, &placement))?;
    Ok(ReorderReport {
        moved: names,
        from_index: start,
        to_index,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        ReorderLumpsParams, ReorderReport, locate, move_run, plan_reorder, reorder_label,
        reorder_lumps, run_end, take_run,
    };
    use crate::error::CyberError;
    use crate::session::Session;
    use crate::tools::lumps::place::{Placement, PlacementParams, inspect, parse_placement};
    use crate::wad::archive::{Lump, Wad, WadKind};
    use crate::wad::classify::Namespace;
    use crate::wad::name::LumpName;

    /// A valid parsed name for test data.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// A lump for a fixture entry: an empty marker when `text` starts with
    /// `@`, otherwise a lump named `text` carrying `text` as its payload.
    fn lump(text: &str) -> Lump {
        text.strip_prefix('@').map_or_else(
            || Lump::new(name(text), text.as_bytes().to_vec()),
            |marker| Lump::marker(name(marker)),
        )
    }

    /// A `PWAD` holding `texts` as lumps.
    fn pwad(texts: &[&str]) -> Wad {
        Wad {
            kind: WadKind::Pwad,
            lumps: texts.iter().map(|text| lump(text)).collect(),
        }
    }

    /// The directory names of an archive, in order.
    fn names(wad: &Wad) -> Vec<String> {
        wad.lumps.iter().map(|lump| lump.name.to_string()).collect()
    }

    /// The payloads of an archive as `NAME(payload)`, in order.
    fn with_payloads(wad: &Wad) -> Vec<String> {
        wad.lumps
            .iter()
            .map(|entry| {
                let body = String::from_utf8(entry.data.to_vec()).expect("test payloads are text");
                format!("{}({body})", entry.name)
            })
            .collect()
    }

    /// The error code of a [`CyberError`], for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    /// The names a report says moved, as texts.
    fn moved(report: &ReorderReport) -> Vec<String> {
        let value = serde_json::to_value(report).expect("a report is serializable");
        value["moved"]
            .as_array()
            .expect("moved is an array")
            .iter()
            .map(|named| named.as_str().expect("names are strings").to_owned())
            .collect()
    }

    /// The destination `index` in its wire form.
    fn at(index: usize) -> PlacementParams {
        PlacementParams {
            index: Some(index),
            ..PlacementParams::default()
        }
    }

    /// The destination `before name` in its wire form.
    fn before(text: &str) -> PlacementParams {
        PlacementParams {
            before: Some(text.to_owned()),
            ..PlacementParams::default()
        }
    }

    /// The destination `after name` in its wire form.
    fn after(text: &str) -> PlacementParams {
        PlacementParams {
            after: Some(text.to_owned()),
            ..PlacementParams::default()
        }
    }

    /// The destination `namespace region` in its wire form.
    fn in_region(region: &str) -> PlacementParams {
        PlacementParams {
            namespace: Some(region.to_owned()),
            ..PlacementParams::default()
        }
    }

    /// The parsed form of a wire destination.
    fn destination(args: &PlacementParams) -> Placement {
        parse_placement(args).expect("test destinations are valid")
    }

    /// Arguments moving `count` lumps from the last working lump named
    /// `text` to a destination.
    fn named_run(
        text: &str,
        count: Option<usize>,
        where_to: PlacementParams,
    ) -> ReorderLumpsParams {
        ReorderLumpsParams {
            name: Some(text.to_owned()),
            from_namespace: None,
            occurrence: None,
            from_index: None,
            count,
            placement: where_to,
        }
    }

    /// Arguments moving `text` inside the working copies of `region`.
    fn run_in_region(text: &str, region: &str, where_to: PlacementParams) -> ReorderLumpsParams {
        ReorderLumpsParams {
            from_namespace: Some(region.to_owned()),
            ..named_run(text, None, where_to)
        }
    }

    /// Arguments moving `text` by its `occurrence` among the working copies.
    fn run_by_occurrence(
        text: &str,
        occurrence: usize,
        where_to: PlacementParams,
    ) -> ReorderLumpsParams {
        ReorderLumpsParams {
            occurrence: Some(occurrence),
            ..named_run(text, None, where_to)
        }
    }

    /// Arguments moving `count` lumps from directory position `index` to a
    /// destination.
    fn positioned_run(
        index: usize,
        count: Option<usize>,
        where_to: PlacementParams,
    ) -> ReorderLumpsParams {
        ReorderLumpsParams {
            name: None,
            from_namespace: None,
            occurrence: None,
            from_index: Some(index),
            count,
            placement: where_to,
        }
    }

    /// A session whose working archive holds `texts`, in order, as one seeded
    /// edit.
    fn seeded(texts: &[&str]) -> Session {
        let mut session = Session::default();
        let lumps: Vec<Lump> = texts.iter().map(|text| lump(text)).collect();
        session
            .edit("seed", move |wad| {
                wad.lumps.extend(lumps);
                Ok(())
            })
            .expect("plain appends");
        session
    }

    #[test]
    fn a_run_comes_out_leaving_the_rest_in_order() {
        let mut wad = pwad(&["A", "B", "C", "D", "E"]);
        let run = take_run(&mut wad, 1, 2).expect("B and C are there");
        assert_eq!(
            run.iter()
                .map(|entry| entry.name.to_string())
                .collect::<Vec<String>>(),
            vec!["B", "C"],
            "the run keeps its order"
        );
        assert_eq!(
            run.iter()
                .map(|entry| String::from_utf8(entry.data.to_vec()).expect("test payloads"))
                .collect::<Vec<String>>(),
            vec!["B", "C"],
            "and its payloads"
        );
        assert_eq!(names(&wad), vec!["A", "D", "E"], "the rest stays in order");
        let past = take_run(&mut wad, 2, 2).expect_err("one lump is not two");
        assert_eq!(code(&past), "invalid_argument", "run past the end code");
        assert!(
            past.to_string().contains("can hold at most 1"),
            "the message says how much room is left: {past}"
        );
        assert_eq!(
            names(&wad),
            vec!["A", "D", "E"],
            "the refusal changed nothing"
        );
    }

    #[test]
    fn a_move_reinserts_the_run_at_an_index_counted_after_removal() {
        let mut wad = pwad(&["A", "B", "C", "D", "E"]);
        let front = move_run(&mut wad, 2, 1, &destination(&at(0))).expect("C to the front");
        assert_eq!(front, 0, "the reported index is the destination");
        assert_eq!(
            names(&wad),
            vec!["C", "A", "B", "D", "E"],
            "C travelled over two lumps"
        );
        let back = move_run(&mut wad, 0, 1, &destination(&at(4))).expect("index 4 of four");
        assert_eq!(back, 4, "the index counts the directory without the run");
        assert_eq!(
            names(&wad),
            vec!["A", "B", "D", "E", "C"],
            "so it lands one past where C stood"
        );
        let tail = move_run(&mut wad, 0, 2, &destination(&at(3))).expect("A and B to the end");
        assert_eq!(tail, 3, "the run of two lands last");
        assert_eq!(
            with_payloads(&wad),
            vec!["D(D)", "E(E)", "C(C)", "A(A)", "B(B)"],
            "the run stayed together and kept its bytes"
        );
    }

    #[test]
    fn an_index_past_the_shortened_directory_is_refused_and_rolled_back() {
        let mut wad = pwad(&["A", "B", "C"]);
        let error = move_run(&mut wad, 0, 2, &destination(&at(2)))
            .expect_err("one lump is left to index into");
        assert_eq!(code(&error), "invalid_argument", "out of range code");
        assert!(
            error.to_string().contains("between 0 and 1"),
            "the range is the directory the run has left: {error}"
        );
        assert_eq!(
            names(&wad),
            vec!["C"],
            "the step stopped with the run still out"
        );
        let mut session = seeded(&["A", "B", "C"]);
        let refused = reorder_lumps(&mut session, &positioned_run(0, Some(2), at(2)))
            .expect_err("the same refusal through the tool");
        assert_eq!(code(&refused), "invalid_argument", "tool-level code");
        assert_eq!(
            names(session.working()),
            vec!["A", "B", "C"],
            "the session edit discards the half-finished step"
        );
        assert_eq!(
            session.undo_labels(),
            vec!["seed"],
            "and records no history for it"
        );
    }

    #[test]
    fn an_anchor_is_the_last_lump_the_run_leaves_behind() {
        let mut wad = pwad(&["A", "B", "E", "C", "E"]);
        let moved =
            move_run(&mut wad, 1, 1, &destination(&after("E"))).expect("B after the last E");
        assert_eq!(moved, 4, "the last E sits at 3 once B has gone");
        assert_eq!(
            names(&wad),
            vec!["A", "E", "C", "E", "B"],
            "anchored on the copy that stayed"
        );
        let back = move_run(&mut wad, 4, 1, &destination(&before("E"))).expect("B before the E");
        assert_eq!(back, 3, "just before the last remaining E");
        assert_eq!(
            names(&wad),
            vec!["A", "E", "C", "B", "E"],
            "moved back inside"
        );
        let missing = move_run(&mut wad, 0, 1, &destination(&after("GHOST")))
            .expect_err("nothing is named GHOST");
        assert_eq!(code(&missing), "not_found", "missing anchor code");
    }

    #[test]
    fn a_namespace_destination_uses_the_region_as_it_stands_after_removal() {
        let mut wad = pwad(&["A", "B", "@F_START", "FLAT", "@F_END"]);
        let joined = move_run(&mut wad, 0, 2, &destination(&in_region("flats")))
            .expect("A and B join flats");
        assert_eq!(
            joined, 2,
            "just before F_END, which sat at 2 once A and B went"
        );
        assert_eq!(
            names(&wad),
            vec!["F_START", "FLAT", "A", "B", "F_END"],
            "the run sits at the region's end"
        );
        let mut alone = pwad(&["A", "B"]);
        let made = move_run(&mut alone, 0, 2, &destination(&in_region("sprites")))
            .expect("the region is created for the run");
        assert_eq!(made, 1, "between the new markers");
        assert_eq!(
            names(&alone),
            vec!["S_START", "A", "B", "S_END"],
            "a missing region grows its canonical markers around the run"
        );
    }

    #[test]
    fn run_end_checks_bounds_and_overflow() {
        assert_eq!(run_end(1, 2, 5).expect("in range"), 3, "one past the run");
        assert_eq!(
            run_end(0, 5, 5).expect("the whole archive"),
            5,
            "an exact fit is allowed"
        );
        assert_eq!(
            run_end(3, 0, 5).expect("no run at all"),
            3,
            "a zero count ends where it starts (the plan refuses it earlier)"
        );
        assert_eq!(
            code(&run_end(5, 1, 5).expect_err("nothing stands at the length")),
            "invalid_argument",
            "a start equal to the length has no lump to move"
        );
        let overflowed = run_end(usize::MAX, 2, 100).expect_err("beyond usize::MAX");
        assert_eq!(code(&overflowed), "invalid_argument", "overflow code");
        assert!(
            overflowed.to_string().contains("overflows"),
            "the message says what overflowed: {overflowed}"
        );
    }

    #[test]
    fn a_label_names_the_run_and_an_empty_run_leaves_only_the_verb() {
        assert_eq!(
            reorder_label(&[name("MAP01"), name("THINGS")]),
            "reorder_lumps MAP01",
            "the first moved lump carries the label"
        );
        assert_eq!(
            reorder_label(&[]),
            "reorder_lumps",
            "an impossible empty run has nothing to name"
        );
    }

    #[test]
    fn plans_need_exactly_one_run_source() {
        let no_source = ReorderLumpsParams {
            name: None,
            from_namespace: None,
            occurrence: None,
            from_index: None,
            count: None,
            placement: at(0),
        };
        let missing = plan_reorder(&no_source).expect_err("nothing to move");
        assert_eq!(code(&missing), "invalid_argument", "no source code");
        assert!(
            missing.to_string().contains("needs a run"),
            "the message asks for one: {missing}"
        );
        let both = ReorderLumpsParams {
            from_index: Some(1),
            ..named_run("A", None, at(0))
        };
        let clash = plan_reorder(&both).expect_err("two sources");
        assert_eq!(code(&clash), "invalid_argument", "both sources code");
        assert!(
            clash.to_string().contains("exactly one"),
            "the message states the rule: {clash}"
        );
        let refined = ReorderLumpsParams {
            occurrence: Some(1),
            ..positioned_run(0, None, at(0))
        };
        let refused = plan_reorder(&refined).expect_err("occurrence without name");
        assert_eq!(code(&refused), "invalid_argument", "refinement clash code");
        assert!(
            refused.to_string().contains("from_index"),
            "the message names the field: {refused}"
        );
        let zero = plan_reorder(&positioned_run(0, Some(0), at(0))).expect_err("count 0");
        assert_eq!(code(&zero), "invalid_argument", "an empty run is refused");
        assert!(
            zero.to_string().contains("at least 1"),
            "the message states the floor: {zero}"
        );
        let huge = plan_reorder(&positioned_run(0, Some(usize::MAX), at(0))).expect("it parses");
        assert_eq!(
            huge.count,
            usize::MAX,
            "the length is checked against the archive later"
        );
    }

    #[test]
    fn plans_need_exactly_one_destination_and_valid_names() {
        let nowhere = plan_reorder(&named_run("A", None, PlacementParams::default()))
            .expect_err("no destination");
        assert_eq!(
            code(&nowhere),
            "invalid_argument",
            "missing destination code"
        );
        assert!(
            nowhere
                .to_string()
                .contains("index, before, after or namespace"),
            "the message lists the fields: {nowhere}"
        );
        let two_fields = ReorderLumpsParams {
            placement: PlacementParams {
                index: Some(1),
                ..after("B")
            },
            ..named_run("A", None, PlacementParams::default())
        };
        assert_eq!(
            code(&plan_reorder(&two_fields).expect_err("two destinations")),
            "invalid_argument",
            "placement conflict code"
        );
        let bad_region =
            plan_reorder(&run_in_region("A", "nope", at(0))).expect_err("not a region");
        assert_eq!(
            code(&bad_region),
            "invalid_argument",
            "bad source region code"
        );
        let bad_name =
            plan_reorder(&named_run("TOO LONG", None, at(0))).expect_err("over eight bytes");
        assert_eq!(code(&bad_name), "invalid_argument", "bad name code");
    }

    #[test]
    fn locating_a_run_reports_its_start_and_names() {
        let session = seeded(&["A", "B", "C"]);
        let plan = plan_reorder(&positioned_run(1, Some(2), at(0))).expect("a valid plan");
        let located = locate(&session, &plan).expect("the run is in range");
        assert_eq!(located.start, 1, "the run starts at B");
        assert_eq!(
            located
                .names
                .iter()
                .map(LumpName::to_string)
                .collect::<Vec<String>>(),
            vec!["B", "C"],
            "and carries both names"
        );
        assert_eq!(
            reorder_label(&located.names),
            "reorder_lumps B",
            "labelled by its first lump"
        );
        let oversized = plan_reorder(&positioned_run(2, Some(2), at(0))).expect("a valid plan");
        assert_eq!(
            code(&locate(&session, &oversized).expect_err("C is the last lump")),
            "invalid_argument",
            "the run reaches past the end"
        );
    }

    #[test]
    fn an_absent_name_or_region_is_not_found_and_the_filter_picks_a_copy() {
        let mut session = seeded(&["FLATA", "@F_START", "FLATA", "@F_END"]);
        let ghost = reorder_lumps(&mut session, &named_run("GHOST", None, at(0)))
            .expect_err("nothing is named GHOST");
        assert_eq!(code(&ghost), "not_found", "missing lump code");
        assert!(
            ghost.to_string().contains("cannot be moved"),
            "the message explains the layers: {ghost}"
        );
        let wrong_region = reorder_lumps(&mut session, &run_in_region("FLATA", "sprites", at(0)))
            .expect_err("no sprite FLATA");
        assert_eq!(code(&wrong_region), "not_found", "wrong region code");
        assert!(
            wrong_region.to_string().contains("the sprites region"),
            "the message names the region: {wrong_region}"
        );
        let past = reorder_lumps(&mut session, &run_by_occurrence("FLATA", 2, at(0)))
            .expect_err("only two copies");
        assert_eq!(code(&past), "not_found", "occurrence past the matches");
        assert!(
            past.to_string().contains("0 to 1"),
            "the message names the range: {past}"
        );
        let in_flats = reorder_lumps(&mut session, &run_in_region("FLATA", "flats", at(0)))
            .expect("the region copy moves");
        assert_eq!(in_flats.from_index, 2, "the flats copy, not the loose one");
        assert_eq!(
            names(session.working()),
            vec!["FLATA", "FLATA", "F_START", "F_END"],
            "the moved copy left its region and is loose now"
        );
        assert_eq!(
            session.undo_labels(),
            vec!["reorder_lumps FLATA", "seed"],
            "only the accepted move was recorded"
        );
    }

    #[test]
    fn a_named_run_moves_in_one_undoable_step_that_reverses() {
        let mut session = seeded(&["A", "B", "C", "D", "E"]);
        let report = reorder_lumps(&mut session, &named_run("C", None, at(0)))
            .expect("C moves to the front");
        assert_eq!(
            serde_json::to_value(&report).expect("serializable"),
            json!({
                "moved": ["C"],
                "from_index": 2_u64,
                "to_index": 0_u64
            }),
            "the documented result shape"
        );
        assert_eq!(
            names(session.working()),
            vec!["C", "A", "B", "D", "E"],
            "the order changed"
        );
        assert_eq!(
            session.undo_labels(),
            vec!["reorder_lumps C", "seed"],
            "one undoable step"
        );
        assert!(session.is_dirty(), "an unsaved edit");
        session.undo().expect("undo the move");
        assert_eq!(
            names(session.working()),
            vec!["A", "B", "C", "D", "E"],
            "undo restores the order"
        );
    }

    #[test]
    fn a_map_block_and_its_data_move_as_one_run() {
        let mut session = seeded(&["PLAYPAL", "MAP01", "THINGS", "LINEDEFS"]);
        let block = reorder_lumps(&mut session, &named_run("MAP01", Some(3), at(0)))
            .expect("the block goes to the front");
        assert_eq!(
            moved(&block),
            vec!["MAP01", "THINGS", "LINEDEFS"],
            "the whole run is reported in order"
        );
        assert_eq!(
            names(session.working()),
            vec!["MAP01", "THINGS", "LINEDEFS", "PLAYPAL"],
            "the header leads its members"
        );
        let report = serde_json::to_value(&block).expect("serializable");
        assert_eq!(
            report["from_index"],
            json!(1_u64),
            "the block started after the palette"
        );
        assert_eq!(report["to_index"], json!(0_u64), "and starts at 0 now");
        assert_eq!(
            session.undo_labels(),
            vec!["reorder_lumps MAP01", "seed"],
            "one step for three lumps, labelled by the header"
        );
    }

    #[test]
    fn a_run_from_index_follows_the_position_form() {
        let mut session = seeded(&["A", "B", "C", "D", "E"]);
        let report = reorder_lumps(&mut session, &positioned_run(1, Some(2), after("E")))
            .expect("B and C go after E");
        assert_eq!(moved(&report), vec!["B", "C"], "the same run by position");
        assert_eq!(
            names(session.working()),
            vec!["A", "D", "E", "B", "C"],
            "appended behind the anchor"
        );
        let past = reorder_lumps(&mut session, &positioned_run(5, Some(1), at(0)))
            .expect_err("the directory ends at 4");
        assert_eq!(code(&past), "invalid_argument", "from_index at the length");
        assert_eq!(
            names(session.working()),
            vec!["A", "D", "E", "B", "C"],
            "and the refusal moved nothing"
        );
    }

    #[test]
    fn moving_a_run_onto_one_of_its_own_lumps_is_refused() {
        let mut session = seeded(&["A", "B", "C"]);
        let onto_itself = reorder_lumps(&mut session, &named_run("A", Some(2), after("B")))
            .expect_err("B travels with A");
        assert_eq!(
            code(&onto_itself),
            "invalid_argument",
            "anchor inside the run"
        );
        assert!(
            onto_itself.to_string().contains("one of the moved lumps"),
            "the message says why: {onto_itself}"
        );
        let itself = reorder_lumps(&mut session, &positioned_run(0, Some(2), before("A")))
            .expect_err("A is the run's first lump");
        assert_eq!(
            code(&itself),
            "invalid_argument",
            "before-anchor inside the run"
        );
        assert_eq!(
            names(session.working()),
            vec!["A", "B", "C"],
            "nothing moved"
        );
        assert_eq!(session.undo_labels(), vec!["seed"], "and nothing recorded");
    }

    #[test]
    fn a_moved_lump_is_reclassified_by_where_it_lands() {
        let mut session = seeded(&["SPR", "@F_START", "FLAT", "@F_END"]);
        let report = reorder_lumps(&mut session, &named_run("SPR", None, at(2)))
            .expect("SPR goes where F_END used to sit");
        assert_eq!(report.to_index, 2, "index 2 of the three lumps left");
        assert_eq!(
            names(session.working()),
            vec!["F_START", "FLAT", "SPR", "F_END"],
            "SPR now sits inside the flats region"
        );
        assert_eq!(
            inspect(session.working(), report.to_index).namespace,
            Some(Namespace::Flats),
            "the placement helper sees the new region"
        );
    }
}
