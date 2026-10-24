//! Writing `UDMF` source and the decimal readers: the whole-document render
//! with its per-kind index comments, the namespace-first rule, each value
//! kind's own shape, [`crate::wad::udmf::decimal_to_rounded_int`]'s rounding,
//! [`crate::wad::udmf::has_fraction`] and the typed reads around a value.

use super::{parse, round_trip};
use crate::error::CyberError;
use crate::wad::udmf::{UdmfDoc, UdmfValue, decimal_to_rounded_int, has_fraction, write_udmf};

#[test]
fn writes_a_document_of_every_value_kind() {
    let written = round_trip(concat!(
        "namespace = \"zdoom\"; rate = 1.25e2; flag = false;",
        " name = \"a\\\"b\"; mode = Sky1; n = 0x10;",
        " vertex { x = 64.000; } vertex { x = 0; }",
        " linedef { v1 = 0; blocking = true; }",
    ));
    assert_eq!(
        written,
        concat!(
            "namespace = \"zdoom\";\n",
            "rate = 1.25e2;\n",
            "flag = false;\n",
            "name = \"a\\\"b\";\n",
            "mode = Sky1;\n",
            "n = 16;\n",
            "vertex { // 0\n",
            "\tx = 64.000;\n",
            "}\n",
            "vertex { // 1\n",
            "\tx = 0;\n",
            "}\n",
            "linedef { // 0\n",
            "\tv1 = 0;\n",
            "\tblocking = true;\n",
            "}\n",
        ),
        "globals keep their order after namespace, blocks count per kind"
    );
}

#[test]
fn round_trips_a_typical_textmap() {
    let text = concat!(
        "namespace = \"zdoom\";\r\n",
        "things\r\n{\r\n",
        "\tx = -128.000;\r\n",
        "\ty = 64.000;\r\n",
        "\tangle = 90;\r\n",
        "\ttype = 1;\r\n",
        "\tflags = 7;\r\n",
        "}\r\n",
        "linedef\r\n{\r\n",
        "\tv1 = 0;\r\n",
        "\tv2 = 1;\r\n",
        "\tsidefront = 0;\r\n",
        "\tblocking = true;\r\n",
        "\tspecial = WS_Door_Close;\r\n",
        "}\r\n",
        "sector\r\n{\r\n",
        "\tfloorplane_sky = \"Sky1\";\r\n",
        "\tlight = 128;\r\n",
        "}\r\n",
    );
    let written = round_trip(text);
    assert!(
        written.contains("sector { // 0\n"),
        "the sector block is indexed: {written}"
    );
    assert_eq!(
        written.matches("// ").count(),
        3,
        "one index comment per block: {written}"
    );
}

#[test]
fn writes_an_empty_document_as_nothing() {
    assert_eq!(write_udmf(&UdmfDoc::default()), "");
}

#[test]
fn moves_the_namespace_to_the_front() {
    let doc = parse("a = 1; namespace = \"utf8\";");
    assert_eq!(
        write_udmf(&doc),
        "namespace = \"utf8\";\na = 1;\n",
        "namespace leads, the rest keeps its order"
    );
}

#[test]
fn writes_a_string_newline_as_an_escape() {
    let written = round_trip("x = \"a\\nb\";");
    assert_eq!(written, "x = \"a\\nb\";\n");
}

#[test]
fn rounds_the_text_of_a_decimal() {
    let cases = [
        ("64.000", 64_i64),
        ("-0.5", -1),
        ("1.25e2", 125),
        ("1e-3", 0),
        ("2.5", 3),
        ("-2.5", -3),
        ("0", 0),
        ("-0", 0),
        ("0.4999", 0),
        ("0.5", 1),
        ("-1.5", -2),
        ("1.4", 1),
        ("1.6", 2),
        ("9.999", 10),
        ("-12.34e1", -123),
        ("0.25e-1", 0),
        ("3.", 3),
        ("-3.", -3),
        ("9223372036854775807", i64::MAX),
        ("-9223372036854775808", i64::MIN),
        ("1e-99999999999999999999999", 0),
        ("-1e-99999999999999999999999", 0),
    ];
    for (text, expected) in cases {
        assert_eq!(
            decimal_to_rounded_int(text).expect("a rounded value"),
            expected,
            "'{text}' rounds to {expected}"
        );
    }
}

#[test]
fn refuses_to_round_what_is_no_decimal() {
    let bad = ["", "x", "abc", "1e", ".5", "1.2.3", "+", "1e+", "e5"];
    for text in bad {
        assert!(
            matches!(decimal_to_rounded_int(text), Err(CyberError::Format(_))),
            "'{text}' is no decimal literal"
        );
    }
    let long = "1".repeat(40);
    for overflow in [
        "1.5e19",
        "9223372036854775808",
        "1e99999999999999999999999",
        "1e9999999999999999999999999999999999999999",
        "-1e9999999999999999999999999999999999999999",
        "9999999999999999999999999999999999999999.5",
        &long,
    ] {
        assert!(
            matches!(decimal_to_rounded_int(overflow), Err(CyberError::Format(_))),
            "'{overflow}' leaves the i64 range"
        );
    }
}

#[test]
fn knows_whether_a_decimal_has_a_fraction() {
    let cases = [
        ("64.000", false),
        ("0", false),
        ("0.0", false),
        ("-3.", false),
        ("1.25e2", false),
        ("10e-1", false),
        ("1.5", true),
        ("-2.5", true),
        ("1e-3", true),
        ("1.0e-1", true),
        ("0.001", true),
        ("x", true),
        ("", true),
    ];
    for (text, expected) in cases {
        assert_eq!(
            has_fraction(text),
            expected,
            "'{text}' fraction is {expected}"
        );
    }
}

#[test]
fn writes_each_value_in_its_own_shape() {
    let doc = parse("i = 0x10; f = 1.25e2; s = \"a\\\\b\"; b = false; k = Sky1; n = -3;");
    assert_eq!(
        write_udmf(&doc),
        concat!(
            "i = 16;\n",
            "f = 1.25e2;\n",
            "s = \"a\\\\b\";\n",
            "b = false;\n",
            "k = Sky1;\n",
            "n = -3;\n",
        ),
        "ints are decimal, floats keep their text, keywords stand bare"
    );
}

#[test]
fn to_source_renders_every_value_kind() {
    assert_eq!(UdmfValue::Int(-3).to_source(), "-3");
    assert_eq!(UdmfValue::Float("64.5".to_owned()).to_source(), "64.5");
    assert_eq!(UdmfValue::Bool(true).to_source(), "true");
    assert_eq!(UdmfValue::Bool(false).to_source(), "false");
    assert_eq!(UdmfValue::Keyword("Sky1".to_owned()).to_source(), "Sky1");
    assert_eq!(
        UdmfValue::Str("a\"b\\c\nd".to_owned()).to_source(),
        r#""a\"b\\c\nd""#,
        "the rendering round trips back to the same value"
    );
    let values = [
        UdmfValue::Int(7),
        UdmfValue::Float("1.25e2".to_owned()),
        UdmfValue::Str("x".to_owned()),
        UdmfValue::Bool(false),
        UdmfValue::Keyword("bottom".to_owned()),
    ];
    for value in values {
        let text = value.to_source();
        let doc = parse(&format!("x = {text};"));
        let rendered = doc.globals.first().expect("the field").1.clone();
        assert_eq!(rendered, value, "'{text}' parses back");
    }
}

#[test]
fn values_read_as_the_types_around_their_own() {
    assert_eq!(UdmfValue::Int(-7).as_i64_rounded(), Some(-7));
    assert_eq!(UdmfValue::Float("2.5".to_owned()).as_i64_rounded(), Some(3));
    assert_eq!(
        UdmfValue::Float("1e-3".to_owned()).as_i64_rounded(),
        Some(0),
        "a text too small to round up is still a whole number"
    );
    assert_eq!(
        UdmfValue::Float("1.5e19".to_owned()).as_i64_rounded(),
        None,
        "a float past i64 has no integer form"
    );
    assert_eq!(UdmfValue::Keyword("x".to_owned()).as_i64_rounded(), None);
    assert_eq!(UdmfValue::Str("x".to_owned()).as_i64_rounded(), None);
    assert_eq!(UdmfValue::Bool(false).as_i64_rounded(), None);

    assert_eq!(UdmfValue::Bool(true).as_bool(), Some(true));
    assert_eq!(UdmfValue::Int(1).as_bool(), None);
    assert_eq!(UdmfValue::Keyword("true".to_owned()).as_bool(), None);

    assert_eq!(
        UdmfValue::Str("a".to_owned()).as_str(),
        Some("a".to_owned())
    );
    assert_eq!(UdmfValue::Keyword("a".to_owned()).as_str(), None);
    assert_eq!(UdmfValue::Int(1).as_str(), None);
}
