// SPDX-License-Identifier: Apache-2.0
use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};

fn command(url: &str) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_engine-worker"));
    cmd.env_remove("ELASTIC_TEST_AUTH")
        .env_remove("ELASTIC_TEST_INDEX")
        .env_remove("ELASTIC_TEST_VALUE_FIELD")
        .env("ANTALYA_TEST_URL", "http://127.0.0.1:1")
        .env("ELASTIC_TEST_URL", url)
        .env("ANTALYA_TEST_TOKEN", "never-print-this-token")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

#[test]
fn oversized_command_is_a_typed_limit_error() {
    let mut child = command("http://127.0.0.1:1").spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&vec![b'x'; 131073])
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("Limit"));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("never-print"));
}

#[test]
fn oversized_body_and_partial_response_are_typed_failures() {
    for (response, expected) in [
        (
            "HTTP/1.1 200 OK\r\nContent-Length: 16777217\r\nConnection: close\r\n\r\n".to_owned(),
            "Limit",
        ),
        (
            "HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{}".to_owned(),
            "Transport",
        ),
        (
            {
                let body = r#"{"timed_out":true,"hits":{},"_shards":{}}"#;
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
            },
            "Incomplete",
        ),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut buf = [0; 16384];
            let _ = socket.read(&mut buf).unwrap();
            socket.write_all(response.as_bytes()).unwrap();
        });
        let mut child = command(&url).spawn().unwrap();
        child.stdin.take().unwrap().write_all(b"{\"engine\":\"elastic\",\"mode\":\"count\",\"body\":{},\"expected_rows\":1,\"expected_total\":0,\"expected_digest\":\"unused\"}\n").unwrap();
        let out = child.wait_with_output().unwrap();
        task.join().unwrap();
        assert!(!out.status.success());
        let error = String::from_utf8_lossy(&out.stderr);
        assert!(error.contains(expected), "{error}");
        assert!(!error.contains("never-print"));
        assert!(!error.contains("panicked"));
    }
}

#[test]
fn bounded_large_compiled_command_and_truncated_line() {
    for (size, newline, expected) in [
        (20000, true, "Input"),
        (131072, true, "Input"),
        (131073, true, "Limit"),
        (20000, false, "Limit"),
    ] {
        let prefix = b"{\"engine\":\"unsupported\",\"mode\":\"count\",\"padding\":\"";
        let suffix: &[u8] = if newline { b"\"}\n" } else { b"\"}" };
        let mut input = prefix.to_vec();
        input.resize(size - suffix.len(), b'x');
        input.extend(suffix);
        let mut child = command("http://127.0.0.1:1").spawn().unwrap();
        child.stdin.take().unwrap().write_all(&input).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(!out.status.success());
        let error = String::from_utf8_lossy(&out.stderr);
        assert!(error.contains(expected), "{error}");
        assert!(!error.contains("never-print"));
    }
}

fn elastic_success(source: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"timed_out":false,"_shards":{"total":1,"successful":1,"failed":0},"hits":{"hits":[{"_source":source}]}})
}
fn request_and_result(
    auth: Option<&str>,
    index: Option<&str>,
    field: Option<&str>,
    source: serde_json::Value,
) -> (String, std::process::Output) {
    use sha2::{Digest, Sha256};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut bytes = Vec::new();
        let mut header_end = None;
        let mut length = 0;
        loop {
            let mut chunk = [0; 4096];
            let n = socket.read(&mut chunk).unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&chunk[..n]);
            assert!(bytes.len() < 131072);
            if header_end.is_none()
                && let Some(pos) = bytes.windows(4).position(|s| s == b"\r\n\r\n")
            {
                header_end = Some(pos + 4);
                let h = String::from_utf8(bytes[..pos].to_vec()).unwrap();
                length = h
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .map(|n| n.parse::<usize>().unwrap())
                    })
                    .unwrap();
            }
            if header_end.is_some_and(|end| bytes.len() >= end + length) {
                break;
            }
        }
        let body = elastic_success(source).to_string();
        write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
        String::from_utf8(bytes).unwrap()
    });
    let mut cmd = command(&url);
    cmd.env_remove("ELASTIC_TEST_AUTH")
        .env_remove("ELASTIC_TEST_INDEX")
        .env_remove("ELASTIC_TEST_VALUE_FIELD");
    if let Some(v) = auth {
        cmd.env("ELASTIC_TEST_AUTH", v);
    }
    if let Some(v) = index {
        cmd.env("ELASTIC_TEST_INDEX", v);
    }
    if let Some(v) = field {
        cmd.env("ELASTIC_TEST_VALUE_FIELD", v);
    }
    let text = "C:\\Windows\\cmd.exe";
    let mut hash = Sha256::new();
    hash.update(1u64.to_le_bytes());
    hash.update([1]);
    hash.update((text.len() as u64).to_le_bytes());
    hash.update(text.as_bytes());
    let input = serde_json::json!({"engine":"elastic","mode":"page","body":{"query":{"match_all":{}},"size":1,"_source":true},"expected_rows":1,"expected_total":1,"expected_digest":hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect::<String>(),"index":"foreign","authorization":"wrong"});
    let mut child = cmd.spawn().unwrap();
    writeln!(child.stdin.take().unwrap(), "{input}").unwrap();
    let out = child.wait_with_output().unwrap();
    (task.join().unwrap(), out)
}
#[test]
fn page_projection_and_clock_are_explicit_in_the_actual_worker() {
    let (request, out) = request_and_result(
        None,
        None,
        None,
        serde_json::json!({"id":1,"v":"C:\\Windows\\cmd.exe"}),
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let body: serde_json::Value =
        serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body["_source"], serde_json::json!(["id", "v"]));
    assert!(
        request
            .lines()
            .next()
            .unwrap()
            .contains("allow_partial_search_results=false")
    );
    let result: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        result["clock_boundary"],
        "request-construction-through-verified-result-v2"
    );
    assert_eq!(result["result_verified"], true);
    assert_eq!(result["elastic_authentication"], "none-diagnostic");
}
#[test]
fn host_auth_index_and_raw_message_field_reach_the_actual_request() {
    let (request, out) = request_and_result(
        Some("Basic Zml4dHVyZTpwYXNz"),
        Some("bench_dmz"),
        Some("message"),
        serde_json::json!({"id":1,"message":"C:\\Windows\\cmd.exe"}),
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(request.starts_with("POST /bench_dmz/_search?"));
    assert!(
        request
            .to_ascii_lowercase()
            .contains("authorization: basic zml4dhvyztpwyxnz")
    );
    let body: serde_json::Value =
        serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body["_source"], serde_json::json!(["id", "message"]));
    let result: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(result["elastic_authentication"], "basic");
    for bytes in [&out.stdout, &out.stderr] {
        assert!(!String::from_utf8_lossy(bytes).contains("Zml4dHVyZTpwYXNz"));
    }
}
#[test]
fn malformed_host_identity_refuses_without_credentials_in_errors() {
    for (key, value) in [
        (
            "ELASTIC_TEST_AUTH",
            "Basic private-secret\r\nInjected: header",
        ),
        ("ELASTIC_TEST_INDEX", "../private-secret"),
        ("ELASTIC_TEST_VALUE_FIELD", "private-secret"),
    ] {
        let out = command("http://127.0.0.1:1")
            .env(key, value)
            .output()
            .unwrap();
        assert!(!out.status.success());
        let message = String::from_utf8_lossy(&out.stderr);
        assert!(message.contains("Configuration"));
        assert!(!message.contains("private-secret"));
    }
}
