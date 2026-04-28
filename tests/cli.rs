use assert_cmd::Command;
use serde_json::Value;
use std::io::Read;
use std::io::Write;
use tempfile::NamedTempFile;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TEST_EMAIL: &str = "user@example.com";
const TEST_PASSWORD: &str = "secret";

#[tokio::test]
async fn cli_auth_test_happy_path() {
    let mock_server = start_mock_server().await;
    let output = run_cli(&[
        "--base-url",
        &mock_server.uri(),
        "--email",
        TEST_EMAIL,
        "--password",
        TEST_PASSWORD,
        "auth",
        "test",
    ]);

    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).expect("valid JSON output");
    assert_eq!(json["status"], "ok");
}

#[tokio::test]
async fn cli_weights_raw_and_weekly_return_valid_json() {
    let mock_server = start_mock_server().await;

    let raw_output = run_cli(&[
        "--base-url",
        &mock_server.uri(),
        "--email",
        TEST_EMAIL,
        "--password",
        TEST_PASSWORD,
        "weights",
        "raw",
    ]);
    assert!(
        raw_output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&raw_output.stderr)
    );
    let raw_json: Value = serde_json::from_slice(&raw_output.stdout).expect("valid JSON output");
    assert!(raw_json["entries"].is_array());
    assert_eq!(raw_json["entries"].as_array().unwrap().len(), 3);

    let weekly_output = run_cli(&[
        "--base-url",
        &mock_server.uri(),
        "--email",
        TEST_EMAIL,
        "--password",
        TEST_PASSWORD,
        "weights",
        "weekly",
        "--start",
        "2026-01-01",
        "--end",
        "2026-03-01",
    ]);
    assert!(
        weekly_output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&weekly_output.stderr)
    );
    let weekly_json: Value =
        serde_json::from_slice(&weekly_output.stdout).expect("valid JSON output");
    assert!(weekly_json["entries"].is_array());
    assert!(weekly_json["summary"]["firstWeek"].is_string());
    assert!(weekly_json["summary"]["totalLoss"].is_number());
}

#[tokio::test]
async fn cli_vault_preview_does_not_write_and_update_requires_confirm() {
    let mock_server = start_mock_server().await;
    let file = create_temp_note().expect("temp note");
    let file_path = file.path().to_str().expect("file path");

    let preview = run_cli(&[
        "--base-url",
        &mock_server.uri(),
        "--email",
        TEST_EMAIL,
        "--password",
        TEST_PASSWORD,
        "vault",
        "preview",
        "--file",
        file_path,
    ]);
    assert!(preview.status.success());
    let preview_json: Value = serde_json::from_slice(&preview.stdout).expect("preview json");
    assert_eq!(preview_json["section_found"], true);
    assert!(preview_json["output"].is_string());

    let mut preview_content = String::new();
    std::fs::File::open(file_path)
        .expect("open temp note")
        .read_to_string(&mut preview_content)
        .expect("read");
    let initial_content = std::fs::read_to_string(file_path).expect("temp note still exists");
    assert_eq!(preview_content, initial_content);

    let denied_update = run_cli(&[
        "--base-url",
        &mock_server.uri(),
        "--email",
        TEST_EMAIL,
        "--password",
        TEST_PASSWORD,
        "vault",
        "update",
        "--file",
        file_path,
    ]);
    assert!(!denied_update.status.success());
    assert!(
        String::from_utf8_lossy(&denied_update.stderr).contains("vault update requires --confirm")
    );

    let confirmed_update = run_cli(&[
        "--base-url",
        &mock_server.uri(),
        "--email",
        TEST_EMAIL,
        "--password",
        TEST_PASSWORD,
        "vault",
        "update",
        "--file",
        file_path,
        "--confirm",
    ]);
    assert!(
        confirmed_update.status.success(),
        "{}",
        String::from_utf8_lossy(&confirmed_update.stderr)
    );
}

async fn start_mock_server() -> MockServer {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v3/account/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "accessToken": "test-token",
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/v3/operation/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "operations": [
                { "entryTimestamp": "2026-01-02T10:00:00.000Z", "weight": 2000.0 },
                { "entryTimestamp": "2026-01-09T10:00:00.000Z", "weight": 1990.0 },
                { "entryTimestamp": "2026-01-16T10:00:00.000Z", "weight": 1980.0 },
            ]
        })))
        .mount(&server)
        .await;

    server
}

fn run_cli(args: &[&str]) -> std::process::Output {
    Command::cargo_bin("weight-gurus-cli")
        .expect("weight-gurus-cli binary")
        .args(args)
        .output()
        .expect("run command")
}

fn create_temp_note() -> std::io::Result<NamedTempFile> {
    let mut file = NamedTempFile::new()?;
    writeln!(
        file,
        "## Some notes\n### Weight Logs\n| **Week Ending** | **Avg Weight** | **Diff** |     | ***Low Weight*** |\n| --------------- | -------------- | -------- | --- | ---------------- |\n| 2026-01-11 | 202.0 lbs | - |     | *200.0 lbs* |\n\n**Total Loss _(based on avg)_:** 2.0 lbs\n## End"
    )?;
    file.flush()?;
    Ok(file)
}
