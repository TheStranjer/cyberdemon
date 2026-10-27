//! Reading `UDMF` source: every value kind, both comment styles, the
//! lowercase-identifier rule, the specification's example and each grammar
//! failure with the `TEXTMAP line:column` it reports.

use super::{error_of, global, parse};
use crate::error::CyberError;
use crate::wad::udmf::{
    MAX_UDMF_TEXT, UdmfBlock, UdmfDoc, UdmfValue, leading_namespace, parse_udmf,
};

#[test]
fn reads_every_value_kind() {
    assert_eq!(global("x = -12;"), ("x".to_owned(), UdmfValue::Int(-12)));
    assert_eq!(
        global("x = 64.000;"),
        ("x".to_owned(), UdmfValue::Float("64.000".to_owned()))
    );
    assert_eq!(
        global("x = \"hello\";"),
        ("x".to_owned(), UdmfValue::Str("hello".to_owned()))
    );
    assert_eq!(global("x = true;"), ("x".to_owned(), UdmfValue::Bool(true)));
    assert_eq!(
        global("x = false;"),
        ("x".to_owned(), UdmfValue::Bool(false))
    );
    assert_eq!(
        global("x = bottom;"),
        ("x".to_owned(), UdmfValue::Keyword("bottom".to_owned()))
    );
    assert_eq!(
        global("x = Sky1;"),
        ("x".to_owned(), UdmfValue::Keyword("Sky1".to_owned()))
    );
}

#[test]
fn reads_hexadecimal_and_octal_integers() {
    assert_eq!(global("x = 0;").1, UdmfValue::Int(0));
    assert_eq!(global("x = 0x1f;").1, UdmfValue::Int(31));
    assert_eq!(global("x = 0X10;").1, UdmfValue::Int(16));
    assert_eq!(global("x = 0xFF;").1, UdmfValue::Int(255));
    assert_eq!(global("x = 017;").1, UdmfValue::Int(15));
    assert_eq!(global("x = -0x10;").1, UdmfValue::Int(-16));
    assert_eq!(global("x = +07;").1, UdmfValue::Int(7));
    assert_eq!(global("x = 000;").1, UdmfValue::Int(0));
}

#[test]
fn reads_negative_and_exponent_numbers() {
    assert_eq!(global("x = -42;").1, UdmfValue::Int(-42));
    assert_eq!(global("x = -0.5;").1, UdmfValue::Float("-0.5".to_owned()));
    assert_eq!(
        global("x = 1.25e2;").1,
        UdmfValue::Float("1.25e2".to_owned())
    );
    assert_eq!(
        global("x = -3.5E-2;").1,
        UdmfValue::Float("-3.5E-2".to_owned())
    );
    assert_eq!(global("x = 64.;").1, UdmfValue::Float("64.".to_owned()));
}

#[test]
fn keeps_punctuation_inside_a_bare_word() {
    assert_eq!(
        global("x = a/b;").1,
        UdmfValue::Keyword("a/b".to_owned()),
        "a slash only starts a comment between tokens"
    );
    assert_eq!(
        global("x = -weird;").1,
        UdmfValue::Keyword("-weird".to_owned()),
        "a sign only begins a number before a digit"
    );
}

#[test]
fn reads_cr_lf_lines_and_raw_tabs_in_strings() {
    let doc = parse("x = \"a\tb\";\r\ny = 2;");
    assert_eq!(
        doc.globals[0].1,
        UdmfValue::Str("a\tb".to_owned()),
        "a raw tab is string content, an escape is its letter"
    );
    assert_eq!(doc.globals[1].0, "y", "CR LF separates lines");
}

#[test]
fn resolves_string_escapes() {
    assert_eq!(
        global(r#"x = "a\"b";"#).1,
        UdmfValue::Str("a\"b".to_owned())
    );
    assert_eq!(
        global(r#"x = "a\\b";"#).1,
        UdmfValue::Str("a\\b".to_owned())
    );
    assert_eq!(
        global(r#"x = "a\nb";"#).1,
        UdmfValue::Str("a\nb".to_owned())
    );
    assert_eq!(global(r#"x = "a\zb";"#).1, UdmfValue::Str("azb".to_owned()));
    assert_eq!(global(r#"x = "a\tb";"#).1, UdmfValue::Str("atb".to_owned()));
}

#[test]
fn skips_both_comment_styles() {
    let doc =
        parse("// leading\nvertex /* a * b */ { /* two */ x = 1; // three\n } /* four */\n// five");
    assert_eq!(doc.blocks.len(), 1, "one block survives the comments");
    assert_eq!(doc.blocks.first().expect("block").kind, "vertex");
    assert_eq!(doc.blocks.first().expect("block").fields.len(), 1);
}

#[test]
fn lowercases_identifiers_only() {
    let doc = parse("LineDef { V1 = true; MyKey = Sky1; Other = True; }");
    let block = doc.blocks.first().expect("the block");
    assert_eq!(block.kind, "linedef", "the kind is lowercased");
    assert_eq!(block.fields[0].0, "v1", "so is the field name");
    assert_eq!(
        block.fields[0].1,
        UdmfValue::Bool(true),
        "lowercase true is a boolean"
    );
    assert_eq!(
        block.fields[1].1,
        UdmfValue::Keyword("Sky1".to_owned()),
        "a keyword keeps its case"
    );
    assert_eq!(
        block.fields[2].1,
        UdmfValue::Keyword("True".to_owned()),
        "only the exact spelling is a boolean"
    );
}

#[test]
fn reads_globals_after_blocks() {
    let doc = parse("a = 1; v { x = 2; } namespace = \"zdoom\"; b = 3;");
    let names: Vec<&str> = doc.globals.iter().map(|field| field.0.as_str()).collect();
    assert_eq!(names, ["a", "namespace", "b"], "globals keep their order");
    assert_eq!(doc.blocks.len(), 1, "the block stays where it was");
}

#[test]
fn leading_namespace_reads_only_the_opening_statement() {
    assert_eq!(
        leading_namespace("namespace = \"zdoom\"; vertex { x = 1; broken"),
        Some("zdoom".to_owned()),
        "a body the full parser would reject still answers"
    );
    assert_eq!(
        leading_namespace("  // a comment\n/* and a block */\nNAMESPACE = \"glbdoom\";"),
        Some("glbdoom".to_owned()),
        "trivia before it, and the identifier lowercases like the parser's"
    );
    assert_eq!(
        leading_namespace("namespace = \"a\\\"b\";"),
        Some("a\"b".to_owned()),
        "string escapes resolve"
    );
    assert_eq!(
        leading_namespace("namespace = \"zdoom\""),
        None,
        "the statement needs its semicolon"
    );
    assert_eq!(leading_namespace(""), None, "empty text declares nothing");
    assert_eq!(
        leading_namespace("vertex {} namespace = \"zdoom\";"),
        None,
        "a block first is not a leading namespace"
    );
    assert_eq!(
        leading_namespace("skypear = 1; namespace = \"zdoom\";"),
        None,
        "a later global is not leading"
    );
    assert_eq!(
        leading_namespace("namespace = 3;"),
        None,
        "a non-string value declares nothing"
    );
    assert_eq!(
        leading_namespace("namespace { x = 1; }"),
        None,
        "a block named namespace is no global"
    );
    assert_eq!(
        leading_namespace("// /* unterminated"),
        None,
        "broken trivia answers nothing"
    );
}

#[test]
fn reads_an_empty_block_and_an_empty_document() {
    let doc = parse("thing {}");
    assert_eq!(
        doc.blocks,
        vec![UdmfBlock {
            kind: "thing".to_owned(),
            fields: Vec::new(),
        }]
    );
    assert_eq!(parse(""), UdmfDoc::default(), "nothing parses to nothing");
    assert_eq!(parse("  // just a comment\n"), UdmfDoc::default());
}

#[test]
fn reads_the_specification_example() {
    let doc = parse(
        "namespace = \"zdoom\";\n\
         vertex // 0\n\
         {\n\
         x = 64.000;\n\
         y = -128.000;\n\
         }\n\
         linedef { v1 = 0; v2 = 1; sidefront = 0; blocking = true; }\n",
    );
    assert_eq!(
        doc.globals,
        vec![("namespace".to_owned(), UdmfValue::Str("zdoom".to_owned()))]
    );
    assert_eq!(doc.blocks.len(), 2, "a vertex and a linedef");
    assert_eq!(doc.blocks[1].fields.len(), 4, "four linedef fields");
    assert_eq!(doc.blocks[1].fields[3].1, UdmfValue::Bool(true));
}

#[test]
fn reports_a_missing_identifier() {
    assert_eq!(
        error_of("= 5;"),
        "TEXTMAP 1:1: expected an identifier, found '='"
    );
}

#[test]
fn reports_a_missing_equals_sign() {
    assert_eq!(
        error_of("a = 1;\nb c = 2;"),
        "TEXTMAP 2:3: expected '=' after identifier 'b'"
    );
}

#[test]
fn reports_a_missing_semicolon() {
    assert_eq!(
        error_of("x = 1 }"),
        "TEXTMAP 1:7: expected ';' after the value of 'x', found '}'"
    );
    assert_eq!(
        error_of("v { x = 1 }"),
        "TEXTMAP 1:11: expected ';' after the value of 'x', found '}'"
    );
}

#[test]
fn reports_a_missing_value() {
    assert_eq!(
        error_of("x = ;"),
        "TEXTMAP 1:5: expected a value, found ';'"
    );
    assert_eq!(
        error_of("x = ),"),
        "TEXTMAP 1:5: expected a value, found ')'"
    );
    assert_eq!(
        error_of("x = 'a';"),
        "TEXTMAP 1:5: expected a value, found '''"
    );
}

#[test]
fn reports_an_unterminated_string() {
    assert_eq!(
        error_of("x = \"abc"),
        "TEXTMAP 1:5: unterminated string: expected a closing quote"
    );
    assert_eq!(
        error_of("x = \"a\nb\";"),
        "TEXTMAP 1:5: unterminated string: a raw newline ends it"
    );
    assert_eq!(
        error_of("x = \"a\\"),
        "TEXTMAP 1:5: unterminated string: an escape ends the input"
    );
    assert_eq!(
        error_of("x = \"a\\\nb\";"),
        "TEXTMAP 1:5: unterminated string: an escape ends the line"
    );
}

#[test]
fn reports_values_cut_off_by_the_end_of_input() {
    assert_eq!(
        error_of("x = "),
        "TEXTMAP 1:5: expected a value, found end of input"
    );
    assert_eq!(
        error_of("x"),
        "TEXTMAP 1:2: expected '=' after identifier 'x'"
    );
    assert_eq!(
        error_of("v { 1 = 2; }"),
        "TEXTMAP 1:5: expected an identifier, found '1'"
    );
}

#[test]
fn reports_an_unterminated_comment() {
    assert_eq!(
        error_of("/* open\nx = 1;"),
        "TEXTMAP 1:1: unterminated block comment: expected '*/'"
    );
    assert_eq!(
        error_of("x = 1; / y"),
        "TEXTMAP 1:8: a '/' must start a comment, found '/ '"
    );
    assert_eq!(
        error_of("x = 1; /"),
        "TEXTMAP 1:8: a '/' must start a comment"
    );
}

#[test]
fn reports_an_unterminated_block() {
    assert_eq!(
        error_of("linedef { x = 1;"),
        "TEXTMAP 1:17: unterminated block: expected '}'"
    );
}

#[test]
fn reports_invalid_numbers() {
    assert_eq!(
        error_of("x = 09;"),
        "TEXTMAP 1:5: invalid octal digit '9' in integer '09'"
    );
    assert_eq!(
        error_of("x = -0o7;"),
        "TEXTMAP 1:5: invalid octal digit 'o' in integer '-0o7'"
    );
    assert_eq!(
        error_of("x = 0xg;"),
        "TEXTMAP 1:5: invalid hexadecimal digit 'g' in integer '0xg'"
    );
    assert_eq!(
        error_of("x = 1e5;"),
        "TEXTMAP 1:5: invalid decimal digit 'e' in integer '1e5'"
    );
    assert_eq!(error_of("x = 0x;"), "TEXTMAP 1:5: '0x' is not an integer");
    assert_eq!(
        error_of("x = 1.2.3;"),
        "TEXTMAP 1:5: '1.2.3' is not a valid float"
    );
    assert_eq!(
        error_of("x = .5;"),
        "TEXTMAP 1:5: '.5' is not a valid float"
    );
    assert_eq!(
        error_of("x = 1.5e;"),
        "TEXTMAP 1:5: '1.5e' is not a valid float"
    );
    assert_eq!(
        error_of("x = 1.5e+;"),
        "TEXTMAP 1:5: '1.5e+' is not a valid float"
    );
    assert_eq!(
        error_of("x = 1.2e3e4;"),
        "TEXTMAP 1:5: '1.2e3e4' is not a valid float"
    );
}

#[test]
fn reports_integer_overflow() {
    assert_eq!(
        error_of("x = 99999999999999999999;"),
        "TEXTMAP 1:5: the value '99999999999999999999' does not fit into an i64"
    );
    assert_eq!(
        error_of("x = -9223372036854775809;"),
        "TEXTMAP 1:5: the value '-9223372036854775809' does not fit into an i64"
    );
    assert_eq!(
        global("x = -9223372036854775808;").1,
        UdmfValue::Int(i64::MIN),
        "the boundary itself parses"
    );
    assert_eq!(
        error_of(&format!("x = {};", "9".repeat(40))),
        format!(
            "TEXTMAP 1:5: the value '{}' does not fit into an i64",
            "9".repeat(40)
        ),
        "even digits past the i128 accumulator are refused"
    );
}

#[test]
fn rejects_text_beyond_the_size_guard() {
    let too_big = "a".repeat(MAX_UDMF_TEXT.saturating_add(1));
    assert!(
        matches!(parse_udmf(&too_big), Err(CyberError::LimitExceeded(_))),
        "one byte over the limit is refused"
    );
}
