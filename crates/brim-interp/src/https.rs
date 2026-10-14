//! HTTPS requests for `std.http` in the interpreter, through libcurl (the
//! `curl` crate). Native executables make the same libcurl calls in
//! `brim-codegen/runtime/brim_rt_https.c`, with the same options and error
//! texts.

use curl::easy::{Easy, List};

/// The reason for a libcurl error, the same words the C runtime uses.
fn reason(e: &curl::Error) -> String {
    if e.is_couldnt_resolve_host() {
        "host not found"
    } else if e.is_couldnt_connect() {
        "connection refused"
    } else if e.is_operation_timedout() {
        "timed out"
    } else if e.is_peer_failed_verification() || e.is_ssl_cacert() || e.is_ssl_cacert_badfile() {
        "certificate not trusted"
    } else if e.is_ssl_connect_error() {
        "TLS handshake failed"
    } else if e.is_url_malformed() || e.is_unsupported_protocol() {
        "invalid URL"
    } else if e.is_got_nothing() || e.is_recv_error() || e.is_send_error() {
        "connection reset"
    } else {
        "network error"
    }
    .to_string()
}

/// Makes a request; see `RuntimeFn::HttpRequest` for the result's form.
pub(crate) fn request(
    method: &str,
    url: &str,
    headers: &str,
    body: &str,
    timeout_ms: i64,
) -> Result<String, String> {
    let mut easy = Easy::new();
    let mut list = List::new();
    let fail = |e: curl::Error| reason(&e);
    list.append("Expect:").map_err(fail)?;
    for line in headers.split(['\r', '\n']).filter(|l| !l.is_empty()) {
        list.append(line).map_err(fail)?;
    }
    easy.url(url).map_err(fail)?;
    easy.custom_request(method).map_err(fail)?;
    easy.http_headers(list).map_err(fail)?;
    easy.signal(false).map_err(fail)?;
    easy.follow_location(false).map_err(fail)?;
    easy.timeout(std::time::Duration::from_millis(
        u64::try_from(timeout_ms).unwrap_or(0),
    ))
    .map_err(fail)?;
    if method == "HEAD" {
        easy.nobody(true).map_err(fail)?;
    }
    if !body.is_empty() || matches!(method, "POST" | "PUT" | "PATCH") {
        easy.post_field_size(body.len() as u64).map_err(fail)?;
        easy.post_fields_copy(body.as_bytes()).map_err(fail)?;
    }
    let mut head: Vec<u8> = Vec::new();
    let mut content: Vec<u8> = Vec::new();
    {
        let mut transfer = easy.transfer();
        transfer
            .write_function(|data| {
                content.extend_from_slice(data);
                Ok(data.len())
            })
            .map_err(fail)?;
        transfer
            .header_function(|line| {
                if line.starts_with(b"HTTP/") {
                    head.clear();
                } else if line != b"\r\n" && line != b"\n" {
                    head.extend_from_slice(line);
                }
                true
            })
            .map_err(fail)?;
        transfer.perform().map_err(fail)?;
    }
    let status = easy.response_code().map_err(fail)?;
    let content =
        String::from_utf8(content).map_err(|_| "the response is not valid UTF-8".to_string())?;
    let head = String::from_utf8_lossy(&head);
    Ok(format!("{status}\r\n{head}\r\n{content}"))
}
