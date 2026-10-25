//! The parse and I/O topic: every header and entry refusal, the two
//! directory-width checks, the raw entry's window and the lookups, then the
//! trips through a real file with its staging behaviour.

use super::{build_file, fs_entries, name, raw_entry, sample};
use crate::wad::archive::directory::{DirectoryEntry, directory_start};
use crate::wad::archive::{MAX_LUMPS, Wad, WadKind, offset_field, write_file_atomic};

#[test]
fn parse_rejects_a_short_header() {
    let error = Wad::parse(b"PWA").expect_err("three bytes are no WAD");
    assert_eq!(error.code(), "format_error", "short header code");
    assert!(
        error.to_string().contains("shorter than"),
        "short header message: {error}"
    );
}

#[test]
fn parse_rejects_an_unknown_magic() {
    let error =
        Wad::parse(b"SLAD\0\0\0\0\x0c\0\0\0").expect_err("an upside-down magic is not a WAD");
    assert_eq!(error.code(), "format_error", "bad magic code");
    assert!(
        error.to_string().contains("unknown WAD magic"),
        "bad magic message: {error}"
    );
}

#[test]
fn parse_rejects_a_negative_lump_count() {
    let file = build_file(WadKind::Pwad, -1, 12, &[]);
    let error = Wad::parse(&file).expect_err("negative counts fail");
    assert_eq!(error.code(), "format_error", "negative count code");
    assert!(
        error.to_string().contains("-1"),
        "the message carries the number: {error}"
    );
}

#[test]
fn parse_rejects_a_negative_directory_offset() {
    let file = build_file(WadKind::Pwad, 0, -16, &[]);
    let error = Wad::parse(&file).expect_err("negative offsets fail");
    assert_eq!(error.code(), "format_error", "negative offset code");
    assert!(
        error.to_string().contains("negative directory offset"),
        "offset message: {error}"
    );
}

#[test]
fn parse_rejects_a_directory_past_the_end() {
    let file = build_file(WadKind::Pwad, 4, 1_000, &[]);
    let error = Wad::parse(&file).expect_err("the directory must fit");
    assert_eq!(error.code(), "format_error", "directory bound code");
    let message = error.to_string();
    assert!(
        message.contains("directory") && message.contains("1000") && message.contains('4'),
        "the message carries the numbers: {message}"
    );
}

#[test]
fn parse_rejects_an_oversized_lump_count() {
    let file = build_file(WadKind::Pwad, MAX_LUMPS + 1, 12, &[]);
    let error = Wad::parse(&file).expect_err("huge counts are refused");
    assert_eq!(error.code(), "limit_exceeded", "count limit code");
    assert!(
        error.to_string().contains("limit"),
        "the message names the limit: {error}"
    );
}

#[test]
fn parse_rejects_a_lump_running_past_the_end() {
    let directory = raw_entry(8, 100, *b"TOOBIG\0\0");
    let file = build_file(WadKind::Pwad, 1, 16, &[b"data", &directory]);
    let error = Wad::parse(&file).expect_err("ranges must fit the file");
    assert_eq!(error.code(), "format_error", "lump bound code");
    let message = error.to_string();
    assert!(
        message.contains("TOOBIG") && message.contains("lump 0"),
        "the message names the index and name: {message}"
    );
}

#[test]
fn parse_rejects_a_negative_size() {
    let directory = raw_entry(0, -5, *b"NEGSIZE\0");
    let file = build_file(WadKind::Pwad, 1, 13, &[b"x", &directory]);
    let error = Wad::parse(&file).expect_err("sizes cannot be negative with data");
    assert_eq!(error.code(), "format_error", "negative size code");
    assert!(
        error.to_string().contains("negative size"),
        "negative size message: {error}"
    );
}

#[test]
fn parse_rejects_a_negative_filepos_on_a_data_lump() {
    let directory = raw_entry(-4, 3, *b"NEGPOS\0\0");
    let file = build_file(WadKind::Pwad, 1, 15, &[b"abc", &directory]);
    let error = Wad::parse(&file).expect_err("data cannot start before the header");
    assert_eq!(error.code(), "format_error", "negative position code");
    let message = error.to_string();
    assert!(
        message.contains("NEGPOS") && message.contains("negative file position"),
        "the message names the lump: {message}"
    );
}

#[test]
fn directory_width_checks_are_exercised_directly() {
    let file = build_file(WadKind::Pwad, 0, 12, &[]);
    let wide = usize::MAX.checked_div(8).expect("divides");
    let overflow = directory_start(&file, wide).expect_err("16 times this count overflows");
    assert_eq!(overflow.code(), "format_error", "overflowing width code");
    assert!(
        overflow.to_string().contains("too wide"),
        "wide directory message: {overflow}"
    );
    let huge = usize::MAX.checked_div(16).expect("divides");
    let far = build_file(WadKind::Pwad, 0, i32::MAX, &[]);
    let sum = directory_start(&far, huge).expect_err("i32::MAX plus this width overflows");
    assert!(
        sum.to_string().contains("overflows the address space"),
        "address space message: {sum}"
    );
    let past = usize::try_from(i32::MAX)
        .expect("fits")
        .checked_mul(4)
        .expect("test count fits");
    let error = directory_start(&file, past).expect_err("the directory must fit the file");
    assert!(
        error.to_string().contains("past the end"),
        "past-the-end message: {error}"
    );
}

#[test]
fn offset_field_refuses_positions_past_the_format() {
    assert_eq!(
        offset_field(12, "lump 0 data").expect("12 fits"),
        12_u32,
        "small positions pass"
    );
    let over_signed = usize::try_from(i32::MAX)
        .expect("fits")
        .checked_add(1)
        .expect("one more");
    let wide_error = offset_field(over_signed, "lump 1 data").expect_err("i32::MAX plus one fails");
    assert_eq!(wide_error.code(), "limit_exceeded", "past-i32 code");
    assert!(
        wide_error.to_string().contains("i32::MAX"),
        "past-i32 message: {wide_error}"
    );
    let over_wide = usize::try_from(u32::MAX)
        .expect("fits")
        .checked_add(1)
        .expect("one more");
    let huge_error = offset_field(over_wide, "directory").expect_err("u32::MAX plus one fails");
    assert_eq!(huge_error.code(), "limit_exceeded", "past-u32 code");
    assert!(
        huge_error.to_string().contains("32 bits"),
        "past-u32 message: {huge_error}"
    );
}

#[test]
fn entry_into_lump_copies_the_bytes_it_names() {
    let raw = DirectoryEntry {
        filepos: 2,
        size: 3,
        name: name("WINDOW"),
    };
    let lump = raw.into_lump(b"0123456789", 7).expect("the window fits");
    assert_eq!(lump.data.as_ref(), b"234", "the addressed window");
    assert_eq!(lump.name, name("WINDOW"), "the name carries over");
}

#[test]
fn lookups_find_duplicates_in_engine_order() {
    let wad = sample(WadKind::Pwad);
    let texture = name("TEXTURE1");
    assert_eq!(wad.find_first(texture), Some(1), "first hit");
    assert_eq!(
        wad.find_last(texture),
        Some(4),
        "the patch shadows the base"
    );
    assert_eq!(wad.find_all(texture), vec![1, 4], "every hit");
    assert_eq!(wad.find_from(texture, 2), Some(4), "from a later start");
    assert_eq!(wad.find_from(texture, 5), None, "past the last hit");
    assert_eq!(wad.find_from(texture, 99), None, "past the end");
    assert_eq!(wad.find_first(name("NOPE")), None, "missing name");
    assert_eq!(wad.len(), 6, "directory length");
    assert!(!wad.is_empty(), "six lumps are not empty");
}

#[test]
fn put_global_overwrites_the_last_copy_in_place() {
    let mut wad = sample(WadKind::Pwad);
    wad.put_global(name("TEXTURE1"), vec![9_u8])
        .expect("the overwrite runs");
    assert_eq!(wad.len(), 6, "no lump grew in");
    assert_eq!(
        wad.lumps.get(4).expect("the last TEXTURE1").data.to_vec(),
        vec![9_u8],
        "the last copy changed"
    );
    assert_eq!(
        wad.lumps.get(1).expect("the first TEXTURE1").data.to_vec(),
        b"dir-bytes".to_vec(),
        "the earlier copy stands"
    );
}

#[test]
fn put_global_appends_a_lump_the_archive_lacks() {
    let mut wad = Wad::default();
    wad.put_global(name("PNAMES"), vec![7_u8])
        .expect("the append runs");
    assert_eq!(wad.lumps.len(), 1, "one lump arrived");
    assert_eq!(
        wad.lumps.first().expect("one lump").name,
        name("PNAMES"),
        "its name"
    );
    assert_eq!(
        wad.lumps.first().expect("one lump").data.to_vec(),
        vec![7_u8],
        "its bytes"
    );
}

#[test]
fn read_file_round_trips_a_written_archive() {
    let dir = tempfile::tempdir().expect("create a scratch directory");
    let path = dir.path().join("sample.wad");
    let wad = sample(WadKind::Iwad);
    let encoded = wad.to_bytes().expect("serializes");
    write_file_atomic(&path, &encoded).expect("the first write succeeds");
    assert_eq!(
        Wad::read_file(&path).expect("read back"),
        wad,
        "file round trip"
    );
    let replacement = Wad::default().to_bytes().expect("serializes");
    write_file_atomic(&path, &replacement).expect("an existing target is replaced");
    assert_eq!(
        Wad::read_file(&path).expect("reread"),
        Wad::default(),
        "the target holds the new archive"
    );
    assert_eq!(
        fs_entries(dir.path()),
        vec!["sample.wad".to_owned()],
        "no staging files remain"
    );
}

#[test]
fn read_file_reports_missing_paths_as_io() {
    let dir = tempfile::tempdir().expect("create a scratch directory");
    let error =
        Wad::read_file(&dir.path().join("absent.wad")).expect_err("absent files cannot be read");
    assert_eq!(error.code(), "io_error", "missing file code");
    assert!(
        error.to_string().contains("absent.wad"),
        "the message names the path: {error}"
    );
}

#[test]
fn write_file_atomic_needs_an_existing_parent() {
    let dir = tempfile::tempdir().expect("create a scratch directory");
    let path = dir.path().join("gone").join("out.wad");
    let error = write_file_atomic(&path, b"PWAD").expect_err("no parent directory");
    assert_eq!(error.code(), "not_found", "missing parent code");
    assert!(
        error.to_string().contains("parent directory"),
        "missing parent message: {error}"
    );
}

#[test]
fn write_file_atomic_removes_its_staging_file_when_rename_fails() {
    let blocker = tempfile::tempdir().expect("a directory cannot be renamed over");
    let target = blocker.path().to_path_buf();
    let error = write_file_atomic(&target, b"PWAD").expect_err("rename onto a directory");
    assert_eq!(error.code(), "io_error", "failed rename code");
    assert_eq!(
        fs_entries(&target),
        Vec::<String>::new(),
        "no staging files remain"
    );
}
