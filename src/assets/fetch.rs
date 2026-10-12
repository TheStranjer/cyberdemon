//! Turning client-supplied `source` strings into bytes.
//!
//! Every import tool starts the same way: the client hands it a `source`
//! string and it needs the bytes behind it. [`AppState::fetch_source`] is
//! that one step. It accepts absolute local paths, `file://` URIs (percent
//! decoded), `http(s)://` `URL`s, and `URL`s of this server's own asset
//! server — the last kind is read straight from the [`AssetStore`] without
//! an HTTP round trip. Relative paths are rejected; everything is capped at
//! [`MAX_ASSET_BYTES`].
//!
//! [`MAX_ASSET_BYTES`]: crate::assets::store::MAX_ASSET_BYTES
//!
//! [`AssetStore`]: crate::assets::store::AssetStore

use core::time::Duration;
use std::io;
use std::path::Path;

use crate::assets::store::mime_for_extension;
use crate::error::CyberError;
use crate::state::AppState;
use crate::wad::name::{LumpName, MAX_NAME_LEN};

/// Whole-request timeout for `HTTP` and `HTTPS` fetches.
const HTTP_TIMEOUT: Duration = Duration::from_secs(60);

/// Maximum number of `HTTP` redirects one fetch follows.
const MAX_REDIRECTS: usize = 5;

/// The bytes and metadata fetched from a client-supplied `source`.
#[derive(Clone, Debug)]
pub struct Fetched {
    /// The file contents.
    pub bytes: Vec<u8>,
    /// The last path segment of the source (percent-decoded for `URL`s);
    /// empty when the source names no file.
    pub filename: String,
    /// The media type when something describes it: the `Content-Type`
    /// header over `HTTP`, the file extension on disk, the stored type for
    /// own assets.
    pub mime: Option<String>,
}

/// What a normalized (trimmed, non-empty) source string names.
enum Target {
    /// An absolute local path, percent-decoded.
    File(String),
    /// A remote or own `HTTP`/`HTTPS` `URL`, still percent-encoded.
    Http(String),
}

/// Turn a client-supplied `source` string into bytes, capped at `limit`.
///
/// This is the body of [`AppState::fetch_source`], which passes the default
/// [`MAX_ASSET_BYTES`] cap; the inherent `impl AppState` block lives in
/// `state.rs` so the type keeps all its methods in one file.
///
/// [`MAX_ASSET_BYTES`]: crate::assets::store::MAX_ASSET_BYTES
///
/// # Errors
///
/// [`AppState::fetch_source`]'s errors: `invalid_argument` for blank,
/// relative, non-absolute or malformed sources, `not_found` for missing
/// files and evicted assets, `io_error` for other filesystem failures,
/// `fetch_error` for HTTP status, connection and stream failures,
/// `unsupported` for other URI schemes and `limit_exceeded` over the cap.
pub(crate) async fn fetch_source_limited(
    state: &AppState,
    source: &str,
    limit: usize,
) -> Result<Fetched, CyberError> {
    let trimmed = source.trim();
    if trimmed.is_empty() {
        return Err(CyberError::InvalidArgument(
            "source must not be empty".to_owned(),
        ));
    }
    match classify(trimmed)? {
        Target::File(path) => read_local_file(&path, limit).await,
        Target::Http(url) => match state.asset_id_from_url(&url) {
            Some(id) => fetch_asset(state, &id, limit),
            None => fetch_http(&url, limit).await,
        },
    }
}

/// Copy `id`'s bytes out of the asset store, refreshing its recency.
fn fetch_asset(state: &AppState, id: &str, limit: usize) -> Result<Fetched, CyberError> {
    let evicted =
        format!("asset {id} was evicted; regenerate the file instead of refetching its URL");
    let asset = state.with_assets(|store| {
        store
            .get(id)
            .ok_or_else(|| CyberError::NotFound(evicted.clone()))
    })?;
    check_size(&format!("asset {id}"), asset.data.len(), limit)?;
    Ok(Fetched {
        bytes: asset.data.to_vec(),
        filename: asset.filename,
        mime: Some(asset.mime),
    })
}

/// Sort a trimmed, non-empty source string into its fetch target.
fn classify(trimmed: &str) -> Result<Target, CyberError> {
    if let Some(rest) = trimmed.strip_prefix("file://") {
        let path = percent_decode(rest)?;
        if path.starts_with('/') {
            return Ok(Target::File(path));
        }
        return Err(CyberError::InvalidArgument(format!(
            "file:// URL {trimmed} does not name an absolute path; use file:///..."
        )));
    }
    if trimmed.starts_with('/') {
        return Ok(Target::File(trimmed.to_owned()));
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return Ok(Target::Http(trimmed.to_owned()));
    }
    Err(scheme_error(trimmed))
}

/// The error for a source that is neither an absolute path nor fetchable.
fn scheme_error(text: &str) -> CyberError {
    text.split_once("://")
        .filter(|pair| is_scheme(pair.0))
        .map_or_else(
            || CyberError::InvalidArgument("source must be an absolute path or a URI".to_owned()),
            |(scheme, _rest)| {
                CyberError::Unsupported(format!(
                    "scheme {scheme} is not supported; pass an absolute path, a file:// \
                     or an http(s):// URL"
                ))
            },
        )
}

/// Whether `candidate` is a syntactically valid URI scheme (RFC 3986).
fn is_scheme(candidate: &str) -> bool {
    let mut chars = candidate.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'))
}

/// Decode `%XX` escapes; every other byte passes through unchanged.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when a `%` does not start a complete hex
/// escape or the decoded bytes are not valid UTF-8.
fn percent_decode(text: &str) -> Result<String, CyberError> {
    let mut bytes: Vec<u8> = Vec::with_capacity(text.len());
    let mut raw = text.as_bytes().iter().copied();
    while let Some(byte) = raw.next() {
        if byte == b'%' {
            let first = raw.next().ok_or_else(|| escape_error(text))?;
            let second = raw.next().ok_or_else(|| escape_error(text))?;
            let value = hex_pair(first, second).ok_or_else(|| escape_error(text))?;
            bytes.push(value);
        } else {
            bytes.push(byte);
        }
    }
    String::from_utf8(bytes).map_err(|error| {
        CyberError::InvalidArgument(format!(
            "{text} is not valid UTF-8 after percent-decoding: {error}"
        ))
    })
}

/// The shared message for a malformed percent-escape.
fn escape_error(text: &str) -> CyberError {
    CyberError::InvalidArgument(format!(
        "invalid percent-escape in {text}; each % must be followed by two hex digits"
    ))
}

/// The byte denoted by two hex digits, if both are hex digits.
fn hex_pair(first: u8, second: u8) -> Option<u8> {
    hex_value(first)?
        .checked_mul(16_u8)?
        .checked_add(hex_value(second)?)
}

/// The nibble a hex digit denotes, if it is a hex digit.
fn hex_value(digit: u8) -> Option<u8> {
    if digit.is_ascii_digit() {
        digit.checked_sub(b'0')
    } else if (b'a'..=b'f').contains(&digit) {
        digit.checked_sub(b'a')?.checked_add(10_u8)
    } else if (b'A'..=b'F').contains(&digit) {
        digit.checked_sub(b'A')?.checked_add(10_u8)
    } else {
        None
    }
}

/// Read an absolute local file, rejecting directories and oversized files.
async fn read_local_file(path: &str, limit: usize) -> Result<Fetched, CyberError> {
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|error| lookup_error(path, &error))?;
    if metadata.is_dir() {
        return Err(CyberError::InvalidArgument(format!(
            "source {path} is a directory; pass a file path"
        )));
    }
    let declared = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
    check_size(&format!("file {path}"), declared, limit)?;
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|error| lookup_error(path, &error))?;
    check_size(&format!("file {path}"), bytes.len(), limit)?;
    Ok(Fetched {
        filename: basename(path),
        mime: mime_from_path(path),
        bytes,
    })
}

/// A failed filesystem lookup: missing is `NotFound`, anything else `Io`.
fn lookup_error(path: &str, error: &io::Error) -> CyberError {
    if error.kind() == io::ErrorKind::NotFound {
        CyberError::NotFound(format!("no file at {path}"))
    } else {
        CyberError::Io(format!("cannot read {path}: {error}"))
    }
}

/// The last path segment of a local path (empty when there is none).
fn basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

/// The media type a local path's extension implies, `None` without one.
fn mime_from_path(path: &str) -> Option<String> {
    Path::new(path)
        .extension()
        .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
        .map(|ext| mime_for_extension(&ext).to_owned())
}

/// GET a remote URL: [`HTTP_TIMEOUT`] per request, at most
/// [`MAX_REDIRECTS`] redirects, at most `limit` body bytes.
async fn fetch_http(url: &str, limit: usize) -> Result<Fetched, CyberError> {
    let filename = url_filename(url)?;
    let client = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
        .build()
        .map_err(|error| CyberError::Fetch(format!("cannot build HTTP client: {error}")))?;
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|error| CyberError::Fetch(format!("fetching {url} failed: {error}")))?;
    let status = response.status();
    if !status.is_success() {
        let code = u16::from(status);
        return Err(CyberError::Fetch(format!(
            "HTTP {code} for {}",
            response.url()
        )));
    }
    let mime = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let declared = response.content_length().map_or(0_usize, |length| {
        usize::try_from(length).unwrap_or(usize::MAX)
    });
    check_size(&format!("response {url}"), declared, limit)?;
    let mut bytes: Vec<u8> = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| CyberError::Fetch(format!("reading {url} failed: {error}")))?
    {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(CyberError::LimitExceeded(format!(
                "response {url} streams more than the fetch limit of {limit} bytes"
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(Fetched {
        bytes,
        filename,
        mime,
    })
}

/// The percent-decoded last path segment of a URL, empty when it has none.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a malformed percent-escape.
fn url_filename(url: &str) -> Result<String, CyberError> {
    let after_authority = url
        .split_once("://")
        .map_or(url, |(_scheme, rest)| rest)
        .split_once('/')
        .map_or("", |(_authority, path)| path);
    let path = after_authority
        .split_once(['?', '#'])
        .map_or(after_authority, |(head, _tail)| head);
    let segment = path.rsplit('/').next().unwrap_or_default();
    percent_decode(segment)
}

/// Reject a payload whose size is above `limit`.
fn check_size(what: &str, size: usize, limit: usize) -> Result<(), CyberError> {
    if size > limit {
        return Err(CyberError::LimitExceeded(format!(
            "{what} is {size} bytes, the fetch limit is {limit}"
        )));
    }
    Ok(())
}

/// Derive a default lump name from a fetched filename.
///
/// Strips the extension, uppercases, keeps only characters valid in a
/// [`LumpName`] and truncates to [`MAX_NAME_LEN`] characters. `None` when
/// nothing usable remains; import tools then require an explicit name.
#[must_use]
#[inline]
pub fn derive_lump_name(filename: &str) -> Option<LumpName> {
    let stem = filename
        .rsplit_once('.')
        .map_or(filename, |(head, _ext)| head);
    let kept: String = stem
        .to_uppercase()
        .chars()
        .filter(|ch| LumpName::is_valid_char(*ch))
        .take(MAX_NAME_LEN)
        .collect();
    LumpName::parse(&kept).ok()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use axum::body::Body;
    use axum::http::{HeaderValue, Response, StatusCode, header};
    use axum::routing::get;
    use axum::{Router, serve};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;

    use super::{
        derive_lump_name, fetch_source_limited, mime_from_path, percent_decode, url_filename,
    };
    use crate::error::CyberError;
    use crate::state::AppState;

    /// Bytes the test server serves at `/ok.png`.
    const OK_BYTES: &[u8] = b"fake-png-bytes-for-tests";

    /// The chunked-body head: a `200` with no `Content-Length`.
    const CHUNKED_HEAD: &[u8] =
        b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nTransfer-Encoding: chunked\r\n\r\n";

    /// One 32-byte (`20` in hex) chunk frame.
    const CHUNK_FRAME: &[u8] = b"20\r\nTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTT\r\n";

    /// The zero-length chunk that ends a chunked body.
    const CHUNK_LAST: &[u8] = b"0\r\n\r\n";

    /// Serve `/ok.png` with a media type.
    async fn ok_handler() -> Response<Body> {
        let mut response = Response::new(Body::from(OK_BYTES.to_vec()));
        *response.status_mut() = StatusCode::OK;
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
        response
    }

    /// Answer 404 with no body.
    async fn missing_handler() -> StatusCode {
        StatusCode::NOT_FOUND
    }

    /// Redirect once to `/ok.png`.
    async fn redirect_handler() -> Response<Body> {
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::FOUND;
        response
            .headers_mut()
            .insert(header::LOCATION, HeaderValue::from_static("/ok.png"));
        response
    }

    /// Serve 64 bytes with an honest `Content-Length`.
    async fn big_handler() -> Response<Body> {
        let mut response = Response::new(Body::from(vec![b'x'; 64]));
        *response.status_mut() = StatusCode::OK;
        response
    }

    /// The throwaway asset origin, serving every test route.
    async fn spawn_server() -> String {
        let app = Router::new()
            .route("/ok.png", get(ok_handler))
            .route("/we%20ird.png", get(ok_handler))
            .route("/missing", get(missing_handler))
            .route("/redirect", get(redirect_handler))
            .route("/big", get(big_handler));
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind an ephemeral port");
        let addr = listener.local_addr().expect("read the bound port");
        let _server: JoinHandle<()> = tokio::spawn(async move {
            match serve(listener, app).await {
                Ok(()) => (),
                Err(_error) => (),
            }
        });
        format!("http://{addr}")
    }

    /// Answer exactly one request with a chunked body of 96 bytes.
    async fn serve_chunked_once(listener: TcpListener) {
        let Ok((mut stream, _peer)) = listener.accept().await else {
            return;
        };
        let mut probe = [0_u8; 1024];
        if stream.read(&mut probe).await.is_err() {
            return;
        }
        for frame in [
            CHUNKED_HEAD,
            CHUNK_FRAME,
            CHUNK_FRAME,
            CHUNK_FRAME,
            CHUNK_LAST,
        ] {
            // The client cuts the connection once its cap trips, so
            // failing writes here are the expected outcome, not a fault.
            let _write: tokio::io::Result<()> = stream.write_all(frame).await;
        }
    }

    /// A one-shot chunked server; the `URL` of its only route.
    async fn spawn_chunked_server() -> String {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind an ephemeral port");
        let addr = listener.local_addr().expect("read the bound port");
        let _task: JoinHandle<()> = tokio::spawn(serve_chunked_once(listener));
        format!("http://{addr}/chunked")
    }

    /// Fetch `source` expecting failure and return the error.
    async fn expect_code(state: &AppState, source: &str, code: &str) -> CyberError {
        let error = state
            .fetch_source(source)
            .await
            .expect_err("the source must be rejected");
        assert_eq!(error.code(), code, "error code for {source}");
        error
    }

    #[tokio::test]
    async fn absolute_path_reads_a_temp_file() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("shot.png");
        fs::write(&path, OK_BYTES).expect("write the file");
        let state = AppState::default();
        let fetched = state
            .fetch_source(path.to_str().expect("utf-8 path"))
            .await
            .expect("read the file");
        assert_eq!(fetched.bytes, OK_BYTES, "identical bytes");
        assert_eq!(fetched.filename, "shot.png", "last path segment");
        assert_eq!(
            fetched.mime.as_deref(),
            Some("image/png"),
            "media type from the extension"
        );
    }

    #[tokio::test]
    async fn missing_file_is_not_found_with_the_path() {
        let state = AppState::default();
        let error = state
            .fetch_source("/definitely-not-here/cyberdemon-test.png")
            .await
            .expect_err("no such file exists");
        assert_eq!(error.code(), "not_found", "missing file code");
        assert!(
            error.to_string().contains("/definitely-not-here"),
            "the message names the path, got {error}"
        );
    }

    #[tokio::test]
    async fn a_directory_is_an_invalid_argument() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let state = AppState::default();
        let error = state
            .fetch_source(dir.path().to_str().expect("utf-8 path"))
            .await
            .expect_err("a directory is not a file");
        assert_eq!(error.code(), "invalid_argument", "directory code");
        assert!(
            error.to_string().contains("directory"),
            "the message says so, got {error}"
        );
    }

    #[tokio::test]
    async fn relative_paths_are_rejected() {
        let state = AppState::default();
        let error = expect_code(&state, "pics/shot.png", "invalid_argument").await;
        assert_eq!(
            error.to_string(),
            "source must be an absolute path or a URI",
            "rejection message"
        );
        expect_code(&state, "shot.png", "invalid_argument").await;
        expect_code(&state, "./shot.png", "invalid_argument").await;
    }

    #[tokio::test]
    async fn empty_and_blank_sources_are_rejected() {
        let state = AppState::default();
        expect_code(&state, "", "invalid_argument").await;
        expect_code(&state, "   ", "invalid_argument").await;
        expect_code(&state, "\t\n", "invalid_argument").await;
    }

    #[tokio::test]
    async fn file_url_percent_escapes_are_decoded() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let weird = dir.path().join("my shots");
        fs::create_dir_all(&weird).expect("create the spaced directory");
        let path = weird.join("my shot.png");
        fs::write(&path, OK_BYTES).expect("write the file");
        let url = format!("file://{}", path.display()).replace(' ', "%20");
        let state = AppState::default();
        let fetched = state
            .fetch_source(&url)
            .await
            .expect("the decoded path reads");
        assert_eq!(fetched.bytes, OK_BYTES, "identical bytes");
        assert_eq!(fetched.filename, "my shot.png", "decoded segment");
    }

    #[tokio::test]
    async fn malformed_percent_escapes_are_rejected() {
        let state = AppState::default();
        expect_code(&state, "file:///tmp/a%zz.png", "invalid_argument").await;
        expect_code(&state, "file:///tmp/a%2.png", "invalid_argument").await;
        expect_code(&state, "file:///tmp/a%.png", "invalid_argument").await;
        expect_code(&state, "http://example.com/a%zz.png", "invalid_argument").await;
    }

    #[tokio::test]
    async fn a_file_url_must_name_an_absolute_path() {
        let state = AppState::default();
        let error = state
            .fetch_source("file://relative/shot.png")
            .await
            .expect_err("no leading slash after the authority");
        assert_eq!(error.code(), "invalid_argument", "non-absolute code");
    }

    #[tokio::test]
    async fn other_schemes_are_unsupported() {
        let state = AppState::default();
        let error = state
            .fetch_source("ftp://example.com/whatever.png")
            .await
            .expect_err("ftp is not fetchable");
        assert_eq!(error.code(), "unsupported", "scheme code");
        assert!(
            error.to_string().contains("ftp"),
            "the message names the scheme, got {error}"
        );
    }

    #[tokio::test]
    async fn local_files_honour_the_size_cap() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("big.bin");
        fs::write(&path, vec![b'x'; 64]).expect("write the file");
        let state = AppState::default();
        let error = fetch_source_limited(&state, path.to_str().expect("utf-8 path"), 8)
            .await
            .expect_err("64 bytes over an 8-byte cap");
        assert_eq!(error.code(), "limit_exceeded", "local cap code");
    }

    #[tokio::test]
    async fn own_asset_urls_read_the_store_without_network() {
        let state = AppState::default();
        let data: Vec<u8> = (0_u8..64_u8).collect();
        let published = state
            .publish("pic.png", "image/png", data.clone())
            .expect("publish succeeds");
        let fetched = state
            .fetch_source(&published.url)
            .await
            .expect("the store holds the asset");
        assert_eq!(fetched.bytes, data, "identical bytes from the store");
        assert_eq!(fetched.filename, "pic.png", "stored filename");
        assert_eq!(
            fetched.mime.as_deref(),
            Some("image/png"),
            "stored media type"
        );
    }

    #[tokio::test]
    async fn evicted_own_asset_urls_are_not_found() {
        let state = AppState::default();
        state
            .with_assets(|store| store.set_capacity(1))
            .expect("capacity 1 is valid");
        let doomed = state
            .publish("doomed.png", "image/png", vec![b'd'; 8])
            .expect("publish succeeds");
        state
            .publish("keeper.png", "image/png", vec![b'k'; 8])
            .expect("publish succeeds");
        let error = state
            .fetch_source(&doomed.url)
            .await
            .expect_err("the evicted asset is gone");
        assert_eq!(error.code(), "not_found", "eviction reports not_found");
        assert!(
            error.to_string().contains("was evicted"),
            "the message explains eviction, got {error}"
        );
    }

    #[tokio::test]
    async fn http_ok_reports_bytes_filename_and_mime() {
        let base = spawn_server().await;
        let state = AppState::default();
        let fetched = state
            .fetch_source(&format!("{base}/ok.png"))
            .await
            .expect("the test server answers");
        assert_eq!(fetched.bytes, OK_BYTES, "body bytes");
        assert_eq!(fetched.filename, "ok.png", "last url segment");
        assert_eq!(
            fetched.mime.as_deref(),
            Some("image/png"),
            "content-type header"
        );
    }

    #[tokio::test]
    async fn http_filenames_are_percent_decoded() {
        let base = spawn_server().await;
        let state = AppState::default();
        let fetched = state
            .fetch_source(&format!("{base}/we%20ird.png"))
            .await
            .expect("the encoded route answers");
        assert_eq!(fetched.filename, "we ird.png", "decoded segment");
    }

    #[tokio::test]
    async fn http_non_success_is_a_fetch_error() {
        let base = spawn_server().await;
        let state = AppState::default();
        let error = state
            .fetch_source(&format!("{base}/missing"))
            .await
            .expect_err("404 is a fetch failure");
        assert_eq!(error.code(), "fetch_error", "status code");
        assert!(
            error.to_string().starts_with("HTTP 404 for "),
            "the message names status and url, got {error}"
        );
    }

    #[tokio::test]
    async fn http_redirects_are_followed() {
        let base = spawn_server().await;
        let state = AppState::default();
        let fetched = state
            .fetch_source(&format!("{base}/redirect"))
            .await
            .expect("the redirect lands on ok");
        assert_eq!(fetched.bytes, OK_BYTES, "bytes from the redirect target");
        assert_eq!(fetched.filename, "redirect", "filename of the request url");
    }

    #[tokio::test]
    async fn http_content_length_above_the_cap_is_rejected() {
        let base = spawn_server().await;
        let state = AppState::default();
        let error = fetch_source_limited(&state, &format!("{base}/big"), 8)
            .await
            .expect_err("64 bytes over an 8-byte cap");
        assert_eq!(error.code(), "limit_exceeded", "declared cap code");
    }

    #[tokio::test]
    async fn http_chunked_bodies_are_capped_while_streaming() {
        let url = spawn_chunked_server().await;
        let state = AppState::default();
        let error = fetch_source_limited(&state, &url, 8)
            .await
            .expect_err("96 streamed bytes over an 8-byte cap");
        assert_eq!(error.code(), "limit_exceeded", "streamed cap code");
        assert!(
            error.to_string().contains("streams more"),
            "the message says streaming, got {error}"
        );
    }

    #[tokio::test]
    async fn http_connection_failures_are_fetch_errors() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a port to give away");
        let addr = listener.local_addr().expect("read the bound port");
        drop(listener);
        let state = AppState::default();
        let error = state
            .fetch_source(&format!("http://{addr}/gone.png"))
            .await
            .expect_err("nothing listens there");
        assert_eq!(error.code(), "fetch_error", "connection failure code");
        assert!(
            error.to_string().contains(&addr.to_string()),
            "the message names the url, got {error}"
        );
    }

    #[test]
    fn derive_lump_name_covers_the_expected_names() {
        let cases = [
            ("titlepic.png", Some("TITLEPIC")),
            ("my sprite.png", Some("MYSPRITE")),
            ("averyverylongname.wav", Some("AVERYVER")),
            (".png", None),
            ("D_RUNNIN.mid", Some("D_RUNNIN")),
            ("map01", Some("MAP01")),
            ("!!!.dat", None),
            ("", None),
        ];
        for (filename, expected) in cases {
            let derived = derive_lump_name(filename).map(|name| name.as_str().to_owned());
            assert_eq!(
                derived,
                expected.map(str::to_owned),
                "lump name derived from {filename:?}"
            );
        }
    }

    #[test]
    fn percent_decode_passes_plain_text_through() {
        assert_eq!(
            percent_decode("plain/text.png").expect("nothing to decode"),
            "plain/text.png",
            "no escapes"
        );
        assert_eq!(
            percent_decode("a%41b%C3%A9").expect("valid escapes"),
            "aAb\u{e9}",
            "hex escapes decode case-insensitively"
        );
        assert_eq!(
            percent_decode("bad%").expect_err("truncated escape").code(),
            "invalid_argument",
            "lone percent sign"
        );
    }

    #[test]
    fn url_filename_extracts_the_last_segment() {
        assert_eq!(
            url_filename("http://h/a/b.png").expect("plain"),
            "b.png",
            "plain path"
        );
        assert_eq!(
            url_filename("https://h/a/b.png?x=1#f").expect("query and fragment"),
            "b.png",
            "query and fragment dropped"
        );
        assert_eq!(
            url_filename("http://h/").expect("trailing slash"),
            "",
            "empty last segment"
        );
        assert_eq!(
            url_filename("http://h").expect("no path"),
            "",
            "no path at all"
        );
    }

    #[test]
    fn mime_from_path_falls_back_without_an_extension() {
        assert_eq!(
            mime_from_path("/x/y.wav").as_deref(),
            Some("audio/wav"),
            "known extension"
        );
        assert_eq!(
            mime_from_path("/x/y.PNG").as_deref(),
            Some("image/png"),
            "extensions match case-insensitively"
        );
        assert_eq!(
            mime_from_path("/x/E1M1").as_deref(),
            None,
            "no extension, no claim"
        );
    }
}
