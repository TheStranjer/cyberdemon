//! The shared payload picker and byte fetch: `add_lump`'s at-most-one
//! rule, `replace_lump`'s exactly-one rule, both refusal texts, and the
//! bytes each payload form resolves to.

use crate::error::CyberError;
use crate::state::AppState;
use crate::tools::lumps::payload::{
    Payload, PayloadFields, PayloadRule, payload_bytes, pick_payload,
};

use super::code;

/// The `add_lump` reading of three payload fields.
fn optional(
    text: Option<&str>,
    digits: Option<&str>,
    source: Option<&str>,
) -> Result<Payload, CyberError> {
    pick_payload(
        "add_lump",
        PayloadRule::AtMostOne,
        PayloadFields::new(text, digits, source),
    )
}

/// The `replace_lump` reading of three payload fields.
fn required(
    text: Option<&str>,
    digits: Option<&str>,
    source: Option<&str>,
) -> Result<Payload, CyberError> {
    pick_payload(
        "replace_lump",
        PayloadRule::ExactlyOne,
        PayloadFields::new(text, digits, source),
    )
}

#[test]
fn the_picker_takes_at_most_one_field_under_add_lump() {
    assert!(
        matches!(optional(None, None, None).expect("none"), Payload::Empty),
        "no data field is a marker"
    );
    assert!(
        matches!(optional(Some("t"), None, None), Ok(Payload::Text(body)) if body == "t"),
        "text wins alone"
    );
    assert!(
        matches!(optional(None, Some("4D55"), None), Ok(Payload::Hex(_))),
        "hex wins alone"
    );
    assert!(
        matches!(
            optional(None, None, Some("/x/y.bin")),
            Ok(Payload::Source(_))
        ),
        "source wins alone"
    );
    for (text, digits, source) in [
        (Some("t"), Some("4d"), None),
        (Some("t"), None, Some("/x")),
        (None, Some("4d"), Some("/x")),
        (Some("t"), Some("4d"), Some("/x")),
    ] {
        let error = optional(text, digits, source).expect_err("two fields");
        assert_eq!(code(&error), "invalid_argument", "payload conflict code");
        assert_eq!(
            error.to_string(),
            "add_lump takes at most one of text, hex and source: pick one payload form",
            "the documented refusal"
        );
    }
}

#[test]
fn the_picker_takes_exactly_one_field_under_replace_lump() {
    assert!(
        matches!(required(Some("t"), None, None), Ok(Payload::Text(body)) if body == "t"),
        "text alone"
    );
    assert!(
        matches!(required(None, Some("4D55"), None), Ok(Payload::Hex(_))),
        "hex alone"
    );
    assert!(
        matches!(required(None, None, Some("/x/y.bin")), Ok(Payload::Source(raw)) if raw == "/x/y.bin"),
        "source alone"
    );
    let none = required(None, None, None).expect_err("no payload");
    assert_eq!(code(&none), "invalid_argument", "no payload code");
    assert_eq!(
        none.to_string(),
        "replace_lump needs a new payload: pass exactly one of text, hex and source",
        "the documented missing-payload refusal"
    );
    for (text, digits, source) in [
        (Some("t"), Some("4d"), None),
        (Some("t"), None, Some("/x")),
        (None, Some("4d"), Some("/x")),
        (Some("t"), Some("4d"), Some("/x")),
    ] {
        let error = required(text, digits, source).expect_err("two fields");
        assert_eq!(code(&error), "invalid_argument", "payload conflict code");
        assert_eq!(
            error.to_string(),
            "replace_lump takes exactly one of text, hex and source: pick one payload form",
            "the same refusal, only the rule word differs"
        );
    }
}

#[tokio::test]
async fn payload_bytes_resolve_their_forms_and_name_bad_hex() {
    let state = AppState::default();
    assert_eq!(
        payload_bytes(&state, Payload::Empty)
            .await
            .expect("an empty marker"),
        (Vec::new(), None),
        "no payload is no bytes and no file name"
    );
    assert_eq!(
        payload_bytes(&state, Payload::Text("hi".to_owned()))
            .await
            .expect("text is taken as-is"),
        (b"hi".to_vec(), None),
        "the text's bytes"
    );
    assert_eq!(
        payload_bytes(&state, Payload::Hex("4D 55".to_owned()))
            .await
            .expect("hex decodes"),
        (vec![0x4D_u8, 0x55], None),
        "spaces ignored"
    );
    assert_eq!(
        payload_bytes(&state, Payload::Hex(String::new()))
            .await
            .expect("empty hex"),
        (Vec::new(), None),
        "empty hex is an empty payload"
    );
    for digits in ["4D5", "4Z"] {
        let error = payload_bytes(&state, Payload::Hex(digits.to_owned()))
            .await
            .expect_err("not a hex payload");
        assert_eq!(code(&error), "invalid_argument", "{digits} code");
        assert!(
            error.to_string().starts_with("hex "),
            "the message names the field: {error}"
        );
    }
}

#[tokio::test]
async fn a_source_fetch_reports_its_failure_and_carries_its_file_name() {
    let state = AppState::default();
    let error = payload_bytes(
        &state,
        Payload::Source("/definitely-not-here/cyberdemon.bin".to_owned()),
    )
    .await
    .expect_err("no such file");
    assert_eq!(
        code(&error),
        "not_found",
        "the fetch reports the missing file"
    );
    let dir = tempfile::tempdir().expect("a scratch directory");
    let file = dir.path().join("note.txt");
    std::fs::write(&file, b"from a file").expect("write the source file");
    let (data, filename) = payload_bytes(&state, Payload::Source(file.display().to_string()))
        .await
        .expect("the file fetches");
    assert_eq!(data, b"from a file".to_vec(), "the file's bytes");
    assert_eq!(filename.as_deref(), Some("note.txt"), "and its name");
}
