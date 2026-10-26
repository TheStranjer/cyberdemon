//! Shared builders for the `UDMF` map tests: the one-text [`read_udmf_map`]
//! readers, the `TEXTMAP` payload puller out of written lumps, the
//! two-room fixture in a chosen namespace and the decoded `zdoom` linedef
//! flags of a list of trigger booleans.
//!
//! The topics below round trip the two-room map in both families, decode
//! every linedef and thing boolean of each spelling and the activation
//! field, keep fractions and unmodelled fields verbatim until the element
//! moves, carry global assignments and unknown block kinds verbatim, and
//! refuse a missing namespace, a missing required field, an absent
//! `TEXTMAP` lump and non-`UTF-8` text.
use crate::error::CyberError;
use crate::wad::archive::Lump;
use crate::wad::classify::MapFormat;
use crate::wad::map::Map;
use crate::wad::map::common::fixtures::{lump, marker, name, two_room_map};
use crate::wad::map::flags::FlagFamily;
use crate::wad::map::udmf_map::{read_udmf_map, write_udmf_map};
use crate::wad::udmf::UdmfValue;

/// The model built from one `TEXTMAP` text plus a closing `ENDMAP`.
fn read_ok(text: &str) -> Map {
    let members = [lump("TEXTMAP", text.as_bytes()), marker("ENDMAP")];
    read_udmf_map(name("MAP01"), &members).expect("the test TEXTMAP decodes")
}

/// The `Format` message one `TEXTMAP` text fails with.
fn error_of(text: &str) -> String {
    let members = [lump("TEXTMAP", text.as_bytes()), marker("ENDMAP")];
    match read_udmf_map(name("MAP01"), &members) {
        Err(CyberError::Format(message)) => message,
        Err(other) => panic!("expected Format, got {other:?}"),
        Ok(map) => panic!("expected a refusal, read {} vertices", map.vertices.len()),
    }
}

/// The `TEXTMAP` payload of written lumps, as text.
fn textmap_of(lumps: &[Lump]) -> String {
    let lump = lumps
        .iter()
        .find(|lump| lump.name.as_bytes() == b"TEXTMAP")
        .expect("the writer emits TEXTMAP");
    String::from_utf8(lump.data.to_vec()).expect("the writer emits UTF-8")
}

/// The two-room fixture as a `UDMF` map in `namespace`.
fn udmf_two_rooms(namespace: &str) -> Map {
    let mut map = two_room_map();
    map.format = MapFormat::Udmf;
    map.udmf_namespace = Some(namespace.to_owned());
    map.other = vec![
        lump("ZNODES", b"gz"),
        lump("BEHAVIOR", b"bh"),
        lump("REJECT", b"rj"),
        lump("BLOCKMAP", b"bm"),
    ];
    map
}

/// The decoded `flags` of a `zdoom` linedef carrying `booleans`.
fn linedef_bits(booleans: &[&str]) -> u32 {
    let flags: Vec<String> = booleans
        .iter()
        .map(|name| format!(" {name} = true;"))
        .collect();
    let text = format!(
        "namespace = \"zdoom\"; linedef {{ v1 = 0; v2 = 0; sidefront = 0;{} }}",
        flags.join("")
    );
    read_ok(&text).linedefs.first().expect("the linedef").flags
}

#[test]
fn the_two_room_map_round_trips_as_udmf_in_both_families() {
    for (namespace, spelling) in [("doom", "skill1 = true;"), ("zdoom", "easy = true;")] {
        let map = udmf_two_rooms(namespace);
        let lumps = write_udmf_map(&map).expect("the fixture is writable");
        assert_eq!(
            lumps.first().map(|lump| lump.name),
            Some(name("MAP01")),
            "the header leads"
        );
        assert_eq!(
            lumps.get(1).map(|lump| lump.name),
            Some(name("TEXTMAP")),
            "then TEXTMAP"
        );
        assert_eq!(
            lumps.last().map(|lump| lump.name),
            Some(name("ENDMAP")),
            "and a fresh ENDMAP closes the block"
        );
        assert_eq!(lumps.len(), 7, "header, TEXTMAP, four others, ENDMAP");
        let text = textmap_of(&lumps);
        assert!(
            text.contains(spelling),
            "the {namespace} family spells thing presence as {spelling}: {text}"
        );
        assert!(
            text.contains("twosided = true;"),
            "the step line keeps its flag"
        );
        assert!(
            text.contains("heightceiling = 128;"),
            "sector heights are written"
        );
        assert!(!text.contains("heightfloor = 0;"), "defaults stay away");
        assert!(
            text.contains("x = 64.000;"),
            "coordinates are written as floats"
        );
        let read = read_udmf_map(map.name, lumps.get(1..).expect("members"))
            .expect("the written block reads back");
        assert_eq!(read, map, "read(write(map)) == map in {namespace}");
    }
}

#[test]
fn a_map_without_a_namespace_writes_as_zdoom() {
    let mut map = udmf_two_rooms("doom");
    map.udmf_namespace = None;
    map.things.clear();
    map.linedefs.clear();
    map.sidedefs.clear();
    map.sectors.clear();
    map.vertices.clear();
    map.other.clear();
    let text = textmap_of(&write_udmf_map(&map).expect("an empty map is writable"));
    assert_eq!(
        text, "namespace = \"zdoom\";\n",
        "the unset namespace falls back and nothing else is emitted"
    );
    let read = read_ok(&text);
    assert_eq!(
        read.udmf_namespace.as_deref(),
        Some("zdoom"),
        "it reads back"
    );
    assert_eq!(
        read.flag_family(),
        FlagFamily::Hexen,
        "and lands in the Hexen family"
    );
}

#[test]
fn the_doom_family_decodes_every_linedef_and_thing_boolean() {
    let map = read_ok(concat!(
        "namespace = \"doom\";",
        "linedef { v1 = 0; v2 = 1; sidefront = 2; sideback = 3; special = 1;",
        " id = 66; arg0 = 66;",
        " blocking = true; blockmonsters = true; twosided = true; dontpegtop = true;",
        " dontpegbottom = true; secret = true; blocksound = true; dontdraw = true;",
        " mapped = true; passuse = true; }",
        "thing { x = 8; y = 16; type = 5; height = 24; angle = 90; id = 7; special = 2;",
        " arg1 = 3; skill1 = true; skill3 = true; skill5 = true; ambush = true;",
        " friend = true; }",
    ));
    let linedef = map.linedefs.first().expect("one linedef");
    assert_eq!(linedef.flags, 0x3FF, "the ten Doom line bits");
    assert_eq!(linedef.tag, 66_i32, "the Doom tag rides on arg0");
    assert_eq!(
        linedef.args.first().copied(),
        Some(66_i32),
        "arg0 keeps the tag"
    );
    assert_eq!(linedef.front, Some(2), "the front side");
    assert_eq!(linedef.back, Some(3), "and the back side");
    assert_eq!(linedef.special, 1_i32, "the special");
    let spawn = map.things.first().expect("one thing");
    assert_eq!(
        spawn.flags, 255_u32,
        "skill1 gives easy, skill3 medium, skill5 hard, ambush and friend set their \
         bits, and absent single, dm and coop invert into notsingle, notdm and notcoop"
    );
    assert_eq!(spawn.tid, 7_i32, "the thing id is the tid");
    assert_eq!(spawn.z, 24_i32, "height is the model z");
    assert_eq!(spawn.args.get(1).copied(), Some(3_i32), "thing args decode");
    let again = read_ok(&textmap_of(&write_udmf_map(&map).expect("writable")));
    assert_eq!(again, map, "the spelled flags survive a write cycle");
}

#[test]
fn the_hexen_family_decodes_every_linedef_and_thing_boolean() {
    let map = read_ok(concat!(
        "namespace = \"zdoom\";",
        "linedef { v1 = 0; v2 = 1; sidefront = 0; blocking = true; blockmonsters = true;",
        " twosided = true; dontpegtop = true; dontpegbottom = true; secret = true;",
        " blocksound = true; dontdraw = true; mapped = true; repeatspecial = true;",
        " playeruse = true; monsteractivate = true; blockplayers = true;",
        " blockeverything = true; }",
        "thing { x = 0; y = 0; type = 1; easy = true; medium = true; hard = true;",
        " ambush = true; dormant = true; class1 = true; class2 = true; class3 = true;",
        " single = true; coop = true; dm = true; translucent = true; invisible = true;",
        " friend = true; standing = true; }",
    ));
    let linedef = map.linedefs.first().expect("one linedef");
    assert_eq!(
        linedef.flags,
        0x1FF_u32 | 0x200 | 0x400 | 0x2000 | 0x4000 | 0x8000,
        "the shared bits, repeatspecial, the playeruse field and the Hexen bits"
    );
    let spawn = map.things.first().expect("one thing");
    assert_eq!(spawn.flags, 0x7FFF, "all fifteen Hexen thing bits");
    let again = read_ok(&textmap_of(&write_udmf_map(&map).expect("writable")));
    assert_eq!(again, map, "the direct spelling survives a write cycle");
}

#[test]
fn the_activation_field_takes_the_first_true_boolean() {
    for (booleans, expected) in [
        (&[][..], 0_u32),
        (&["playercross"][..], 0),
        (&["playeruse"][..], 0x400),
        (&["monstercross"][..], 0x800),
        (&["impact"][..], 0xC00),
        (&["playerpush"][..], 0x1000),
        (&["missilecross"][..], 0x1400),
        (&["monsteruse"][..], 0x2400),
        (&["monsterpush"][..], 0x3000),
        (&["monsterpush", "monsteruse"][..], 0x2400),
        (&["playercross", "impact"][..], 0),
        (&["playeruse", "monstercross"][..], 0x400),
        (&["playercross", "monstercross"][..], 0x2000),
        (&["playeruse", "monsteruse"][..], 0x2400),
        (&["playerpush", "monsterpush"][..], 0x3000),
    ] {
        assert_eq!(
            linedef_bits(booleans),
            expected,
            "activation from {booleans:?}"
        );
    }
}

#[test]
fn a_fractional_vertex_survives_until_it_moves() {
    let map = read_ok("namespace = \"zdoom\"; vertex { x = 64.5; y = -20.25; }");
    let vertex = map.vertices.first().expect("one vertex");
    assert_eq!(vertex.x, 65_i32, "the model keeps the rounded integer");
    assert_eq!(vertex.y, -20_i32, "a fraction rounds half away from zero");
    assert_eq!(vertex.extra.get("x").map(String::as_str), Some("64.5"));
    assert_eq!(vertex.extra.get("y").map(String::as_str), Some("-20.25"));
    let text = textmap_of(&write_udmf_map(&map).expect("writable"));
    assert!(
        text.contains("x = 64.5;") && text.contains("y = -20.25;"),
        "untouched fractions are re-emitted verbatim: {text}"
    );
    assert_eq!(read_ok(&text), map, "and read back identically");
    let mut moved = map.clone();
    let first = moved.vertices.first_mut().expect("one vertex");
    first.x = 70_i32;
    assert!(
        first.extra.remove("x").is_some(),
        "the stale text leaves with the move"
    );
    let moved_text = textmap_of(&write_udmf_map(&moved).expect("writable"));
    assert!(
        moved_text.contains("x = 70.000;") && !moved_text.contains("64.5"),
        "a moved vertex emits the new integer: {moved_text}"
    );
    assert_eq!(read_ok(&moved_text), moved, "the moved vertex round trips");
}

#[test]
fn unmodelled_fields_and_shapes_survive_verbatim() {
    let map = read_ok(concat!(
        "namespace = \"zdoom\";",
        "sector { texturefloor = \"FLOOR1\"; textureceiling = \"CEIL1\";",
        " comment = \"hello\"; lightfloor = 1.5; scalex_top = 2; skyfloor = Sky1; }",
        "linedef { v1 = 0; v2 = 0; sidefront = 0; arg1 = \"nope\"; alpha = 0.5;",
        " blocking = 7; }",
    ));
    let sector = map.sectors.first().expect("one sector");
    assert_eq!(
        sector.light, 160_i32,
        "an unmodelled lightfloor changes nothing"
    );
    assert_eq!(
        sector.extra.get("comment").map(String::as_str),
        Some("\"hello\"")
    );
    assert_eq!(
        sector.extra.get("lightfloor").map(String::as_str),
        Some("1.5")
    );
    assert_eq!(
        sector.extra.get("scalex_top").map(String::as_str),
        Some("2")
    );
    assert_eq!(
        sector.extra.get("skyfloor").map(String::as_str),
        Some("Sky1")
    );
    let linedef = map.linedefs.first().expect("one linedef");
    assert_eq!(linedef.flags, 0, "a non-boolean blocking sets no bit");
    assert_eq!(
        linedef.args.get(1).copied(),
        Some(0_i32),
        "a string arg stays unmodelled"
    );
    assert_eq!(
        linedef.extra.get("arg1").map(String::as_str),
        Some("\"nope\"")
    );
    assert_eq!(linedef.extra.get("alpha").map(String::as_str), Some("0.5"));
    assert_eq!(linedef.extra.get("blocking").map(String::as_str), Some("7"));
    let text = textmap_of(&write_udmf_map(&map).expect("writable"));
    for wanted in [
        "comment = \"hello\";",
        "lightfloor = 1.5;",
        "scalex_top = 2;",
        "skyfloor = Sky1;",
        "arg1 = \"nope\";",
        "alpha = 0.5;",
        "blocking = 7;",
    ] {
        assert!(text.contains(wanted), "the text keeps `{wanted}`: {text}");
    }
    assert_eq!(read_ok(&text), map, "and reads back identically");
}

#[test]
fn globals_and_unknown_blocks_ride_along_verbatim() {
    let map = read_ok(concat!(
        "namespace = \"zdoom\";",
        "ednum = 5;",
        "author = \"A. Demo\";",
        "sector { texturefloor = \"FLOOR1\"; textureceiling = \"CEIL1\"; }",
        "vertex_slope { vertex = 0; angle = 45; }",
        "skybox_plane { height = 1.5; }",
        "thing { x = 0; y = 0; type = 1; }",
    ));
    assert_eq!(
        map.extra_globals,
        vec![
            ("ednum".to_owned(), UdmfValue::Int(5_i64)),
            ("author".to_owned(), UdmfValue::Str("A. Demo".to_owned())),
        ],
        "every global but the namespace is carried, in written order"
    );
    let kinds: Vec<&str> = map
        .extra_blocks
        .iter()
        .map(|block| block.kind.as_str())
        .collect();
    assert_eq!(
        kinds,
        ["vertex_slope", "skybox_plane"],
        "unknown blocks are carried in document order"
    );
    assert!(map.vertices.is_empty(), "a vertex_slope is no vertex");
    assert_eq!(map.sectors.len(), 1, "the element blocks still decode");
    assert_eq!(map.things.len(), 1, "and their indices ignore the extras");
    let text = textmap_of(&write_udmf_map(&map).expect("the map writes"));
    let namespace_at = text.find("namespace =").expect("the namespace leads");
    let ednum_at = text.find("ednum = 5;").expect("the first global");
    let author_at = text
        .find("author = \"A. Demo\";")
        .expect("the second global, quoted as written");
    let thing_at = text.find("thing {").expect("the element blocks");
    let slope_at = text
        .find("vertex_slope {")
        .expect("the first unknown block");
    let slope_angle_at = text.find("angle = 45;").expect("its field");
    let sky_at = text
        .find("skybox_plane {")
        .expect("the second unknown block");
    let sky_height_at = text.find("height = 1.5;").expect("its fraction");
    assert!(
        namespace_at < ednum_at && ednum_at < author_at,
        "the globals follow the namespace declaration, in order: {text}"
    );
    assert!(author_at < thing_at, "the globals lead the blocks: {text}");
    assert!(
        thing_at < slope_at && slope_at < slope_angle_at,
        "unknown blocks come after every element block: {text}"
    );
    assert!(
        sky_height_at > sky_at && slope_angle_at < sky_at,
        "and the unknown blocks keep their order and fields: {text}"
    );
    assert_eq!(read_ok(&text), map, "the whole document round trips");
}

#[test]
fn a_repeated_namespace_global_rides_after_the_first() {
    let map = read_ok(concat!(
        "namespace = \"zdoom\";",
        "vertex { x = 0; y = 0; }",
        "namespace = \"doom\";",
    ));
    assert_eq!(
        map.udmf_namespace.as_deref(),
        Some("zdoom"),
        "the first declaration wins as it used to"
    );
    assert_eq!(
        map.extra_globals,
        vec![("namespace".to_owned(), UdmfValue::Str("doom".to_owned()))],
        "the second is carried like any other global"
    );
    let text = textmap_of(&write_udmf_map(&map).expect("the map writes"));
    assert_eq!(
        text,
        "namespace = \"zdoom\";\nnamespace = \"doom\";\nvertex { // 0\n\tx = 0.000;\n\ty = \
         0.000;\n}\n",
        "both declarations come back in order: {text}"
    );
    assert_eq!(read_ok(&text), map, "and read back identically");
}

#[test]
fn a_missing_namespace_is_refused() {
    let message = error_of("vertex { x = 0; y = 0; }");
    assert!(
        message.contains("namespace") && message.contains("zdoom"),
        "the error names the missing global and a fix: {message}"
    );
    let shape = error_of("namespace = 5; vertex { x = 0; y = 0; }");
    assert!(
        shape.contains("quoted string"),
        "a bare namespace value is refused: {shape}"
    );
}

#[test]
fn a_missing_required_field_names_the_block_and_index() {
    let cases = [
        ("vertex { x = 0; }", "vertex 0", "y"),
        ("linedef { v1 = 0; v2 = 0; }", "linedef 0", "sidefront"),
        ("sidedef { offsetx = 4; }", "sidedef 0", "sector"),
        (
            "sector { texturefloor = \"A\"; }",
            "sector 0",
            "textureceiling",
        ),
        ("thing { x = 0; y = 0; }", "thing 0", "type"),
    ];
    for (body, block, field) in cases {
        let message = error_of(&format!("namespace = \"doom\"; {body}"));
        assert!(
            message.contains(block) && message.contains(field),
            "'{body}' must name {block} and '{field}': {message}"
        );
    }
    let second = error_of("namespace = \"doom\"; vertex { x = 0; y = 0; } vertex { x = 1; }");
    assert!(
        second.contains("vertex 1") && second.contains("'y'"),
        "the index counts the blocks of the kind: {second}"
    );
    let wrong_shape = error_of("namespace = \"doom\"; vertex { x = \"a\"; y = 0; }");
    assert!(
        wrong_shape.contains("vertex 0") && wrong_shape.contains("'x'"),
        "a required field of the wrong shape is refused: {wrong_shape}"
    );
}

#[test]
fn a_block_without_textmap_and_binary_text_are_refused() {
    let members = [marker("THINGS"), marker("ENDMAP")];
    let CyberError::Format(message) =
        read_udmf_map(name("MAP01"), &members).expect_err("no TEXTMAP here")
    else {
        panic!("expected Format for a block without TEXTMAP");
    };
    assert!(message.contains("TEXTMAP"), "the lump is named: {message}");
    let binary = [
        lump("TEXTMAP", &[0xFF_u8, 0xFE, 0x00, 0x80]),
        marker("ENDMAP"),
    ];
    let CyberError::Format(utf8_message) =
        read_udmf_map(name("MAP01"), &binary).expect_err("no UTF-8 here")
    else {
        panic!("expected Format for a binary TEXTMAP");
    };
    assert!(
        utf8_message.contains("UTF-8"),
        "the reason is named: {utf8_message}"
    );
}

#[test]
fn other_lumps_between_the_markers_are_carried() {
    let members = [
        lump("TEXTMAP", b"namespace = \"zdoom\";"),
        lump("BEHAVIOR", b"acs"),
        lump("ZNODES", b"zn"),
        marker("ENDMAP"),
    ];
    let map = read_udmf_map(name("E1M1"), &members).expect("the block reads");
    let carried: Vec<String> = map.other.iter().map(|lump| lump.name.to_string()).collect();
    assert_eq!(
        carried,
        ["BEHAVIOR", "ZNODES"],
        "TEXTMAP and ENDMAP stay out"
    );
    let written = write_udmf_map(&map).expect("the map writes");
    let order: Vec<String> = written.iter().map(|lump| lump.name.to_string()).collect();
    assert_eq!(
        order,
        ["E1M1", "TEXTMAP", "BEHAVIOR", "ZNODES", "ENDMAP"],
        "the carried lumps sit between the markers, in order"
    );
    let read = read_udmf_map(map.name, written.get(1..).expect("members")).expect("re-read");
    assert_eq!(read, map, "the carried lumps round trip");
}
