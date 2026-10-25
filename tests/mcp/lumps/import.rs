//! The two import tools sharing one insert: `import_asset` deriving the lump
//! name from the file stem, `import_file_as_lump` taking an explicit name and
//! placement, the underivable name asking for `name`, region placement, the
//! refused twin, the undo labels every import writes and `import_asset`
//! reading back an uploaded asset `URL`.

use std::fs;

use serde_json::{Value, json};

use crate::lumps::{hex_head, source_arg, undo_labels, working_names};
use crate::support::{call, call_err, connect, http_put};

#[tokio::test]
async fn import_asset_derives_names_and_import_file_as_lump_shares_the_behaviour() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = connect().await;
    let png = dir.path().join("TITLEPIC.png");
    fs::write(&png, b"pretend-png-bytes").expect("write the source file");
    let report = call(&client, "import_asset", source_arg(&png)).await;
    assert_eq!(report["name"], json!("TITLEPIC"), "the stem, uppercased");
    assert_eq!(report["index"], json!(0_u64), "appended");
    assert_eq!(report["size"], json!(17_u64), "bytes as-is");
    assert_eq!(report["namespace"], Value::Null, "loose by default");
    assert_eq!(
        report["source"],
        png.display().to_string(),
        "the import tools echo the source"
    );
    let mut named = source_arg(&png);
    named["name"] = json!("MYICON");
    named["after"] = json!("TITLEPIC");
    let explicit = call(&client, "import_file_as_lump", named).await;
    assert_eq!(explicit["name"], json!("MYICON"), "an explicit name wins");
    assert_eq!(
        explicit["index"],
        json!(1_u64),
        "the placement fields apply too"
    );
    let hidden = dir.path().join(".png");
    fs::write(&hidden, b"x").expect("write the underivable file");
    let (code, message, _) = call_err(&client, "import_asset", source_arg(&hidden)).await;
    assert_eq!(code, "invalid_argument", "nothing usable in the file name");
    assert!(
        message.contains("pass name"),
        "the message asks for the name: {message}"
    );
    let mut named_hidden = source_arg(&hidden);
    named_hidden["name"] = json!("STILL_OK");
    named_hidden["namespace"] = json!("patches");
    let saved = call(&client, "import_file_as_lump", named_hidden).await;
    assert_eq!(saved["name"], json!("STILL_OK"), "with a name it imports");
    assert_eq!(saved["namespace"], json!("patches"), "placement applies");
    assert_eq!(
        working_names(&client).await,
        vec!["TITLEPIC", "MYICON", "P_START", "STILL_OK", "P_END"],
        "one region created, nothing else moved"
    );
    let (twin_code, _, _) = call_err(&client, "import_asset", source_arg(&png)).await;
    assert_eq!(twin_code, "already_exists", "the derived twin is refused");
    assert_eq!(
        undo_labels(&client).await,
        vec!["import TITLEPIC", "import MYICON", "import STILL_OK"],
        "every import labels the history"
    );
}

#[tokio::test]
async fn import_asset_reads_an_uploaded_asset_url() {
    let client = connect().await;
    let info = call(&client, "get_asset_server_info", Value::Null).await;
    let base = info["base_url"].as_str().expect("an origin").to_owned();
    let body = b"uploaded-flat-bytes";
    let (status, receipt) = http_put(&base, "/upload/UPFLAT.lmp", body).await;
    assert_eq!(status, 201, "the upload stored");
    let receipt_json: Value = serde_json::from_slice(&receipt).expect("a JSON receipt");
    let url = receipt_json["url"]
        .as_str()
        .expect("the asset url")
        .to_owned();
    let report = call(&client, "import_asset", json!({ "source": url })).await;
    assert_eq!(
        report["name"],
        json!("UPFLAT"),
        "derived from the stored file name"
    );
    let size = u64::try_from(body.len()).expect("a small count");
    assert_eq!(report["size"], json!(size), "the uploaded bytes");
    assert_eq!(report["source"], json!(url), "the url echoes back");
    let length = u64::try_from(body.len()).expect("a small count");
    let read = call(
        &client,
        "get_lump",
        json!({ "name": "UPFLAT", "scope": "working", "format": "hex", "length": length }),
    )
    .await;
    assert_eq!(
        read["hex"],
        json!(hex_head(body, body.len())),
        "the bytes came back unconverted"
    );
    assert_eq!(
        undo_labels(&client).await,
        vec!["import UPFLAT"],
        "the shared undo label"
    );
}
