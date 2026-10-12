//! The `assets` tool group as seen by a real MCP client: server discovery,
//! uploads through the asset server, and runtime capacity changes.

#[cfg(test)]
mod tests {
    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use cyberdemon::assets::store::MAX_ASSET_BYTES;

    use crate::support::{call, call_err, connect, http_put};

    /// The port out of a `http://localhost:<port>` base `URL`.
    fn port_of(base_url: &str) -> u16 {
        let text = base_url
            .strip_prefix("http://localhost:")
            .expect("a localhost origin");
        text.parse().expect("a numeric port")
    }

    /// Call `get_asset_server_info` and return its structured content.
    async fn info(client: &RunningService<RoleClient, ()>) -> Value {
        call(client, "get_asset_server_info", Value::Null).await
    }

    /// Upload `body` to the server behind `base_url` and require acceptance.
    async fn upload(base_url: &str, filename: &str, body: &[u8]) {
        let path = format!("/upload/{filename}");
        let (status, receipt) = http_put(base_url, &path, body).await;
        assert_eq!(status, 201, "upload of {filename} rejected: {receipt:?}");
    }

    #[tokio::test]
    async fn asset_server_info_describes_a_fresh_server() {
        let client = connect().await;
        let report = info(&client).await;
        let base_url = report["base_url"].as_str().expect("a base url");
        let port = port_of(base_url);
        assert!(
            (3_000_u16..=60_000_u16).contains(&port),
            "port {port} is outside 3000..=60000"
        );
        assert_eq!(
            report["upload_url_template"].as_str(),
            Some(format!("{base_url}/upload/{{filename}}").as_str()),
            "the upload template sits on the same origin"
        );
        assert_eq!(report["capacity"].as_u64(), Some(50), "default capacity");
        assert_eq!(report["stored"].as_u64(), Some(0), "a fresh store is empty");
        assert_eq!(
            report["stored_bytes"].as_u64(),
            Some(0),
            "an empty store holds no bytes"
        );
        assert_eq!(
            report["max_asset_bytes"].as_u64(),
            Some(u64::try_from(MAX_ASSET_BYTES).expect("a usize fits u64")),
            "the advertised upload cap"
        );
        let again = info(&client).await;
        assert_eq!(
            again["base_url"].as_str(),
            Some(base_url),
            "the second call reuses one server"
        );
    }

    #[tokio::test]
    async fn an_upload_shows_up_in_the_stored_counts() {
        let client = connect().await;
        let report = info(&client).await;
        let base_url = report["base_url"].as_str().expect("a base url");
        let template = report["upload_url_template"]
            .as_str()
            .expect("an upload template");
        let url = template.replace("{filename}", "pic.png");
        let path = url
            .strip_prefix(base_url)
            .expect("the template origin matches");
        let body = b"not really a png".to_vec();
        let expected_bytes = body.len();
        let (status, _receipt) = http_put(base_url, path, &body).await;
        assert_eq!(status, 201, "the upload is stored");
        let after = info(&client).await;
        assert_eq!(after["stored"].as_u64(), Some(1), "one asset stored");
        assert_eq!(
            after["stored_bytes"].as_u64(),
            Some(u64::try_from(expected_bytes).expect("a small body fits u64")),
            "the uploaded bytes are counted"
        );
    }

    #[tokio::test]
    async fn shrinking_the_capacity_evicts_the_oldest() {
        let client = connect().await;
        let report = info(&client).await;
        let base_url = report["base_url"].as_str().expect("a base url").to_owned();
        upload(&base_url, "first.bin", b"1").await;
        upload(&base_url, "second.bin", b"2").await;
        let result = call(
            &client,
            "set_asset_capacity",
            json!({ "capacity": 1_usize }),
        )
        .await;
        assert_eq!(result["capacity"].as_u64(), Some(1), "the new capacity");
        assert_eq!(
            result["evicted"].as_u64(),
            Some(1),
            "the shrink dropped the oldest asset"
        );
        let after = info(&client).await;
        assert_eq!(after["capacity"].as_u64(), Some(1), "the capacity persists");
        assert_eq!(
            after["stored"].as_u64(),
            Some(1),
            "only the newest asset remains"
        );
    }

    #[tokio::test]
    async fn capacity_outside_the_range_is_an_invalid_argument() {
        let client = connect().await;
        let zero = call_err(
            &client,
            "set_asset_capacity",
            json!({ "capacity": 0_usize }),
        )
        .await;
        assert_eq!(zero.0, "invalid_argument", "zero capacity: {}", zero.1);
        let above = call_err(
            &client,
            "set_asset_capacity",
            json!({ "capacity": 10_001_usize }),
        )
        .await;
        assert_eq!(
            above.0, "invalid_argument",
            "above the maximum: {}",
            above.1
        );
        let report = info(&client).await;
        assert_eq!(
            report["capacity"].as_u64(),
            Some(50),
            "rejected calls leave the store untouched"
        );
    }
}
