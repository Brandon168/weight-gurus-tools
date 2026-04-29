use assert_cmd::Command;
use serde_json::Value;
use tempfile::tempdir;
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
async fn cli_weights_list_and_aggregate_return_valid_json() {
    let mock_server = start_mock_server().await;

    let list_output = run_cli(&[
        "--base-url",
        &mock_server.uri(),
        "--email",
        TEST_EMAIL,
        "--password",
        TEST_PASSWORD,
        "weights",
        "list",
    ]);
    assert!(
        list_output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&list_output.stderr)
    );
    let list_json: Value = serde_json::from_slice(&list_output.stdout).expect("valid JSON output");
    assert!(list_json["entries"].is_array());
    assert_eq!(list_json["entries"].as_array().unwrap().len(), 3);
    assert_eq!(list_json["entries"][0]["weight"], 200.0);
    assert_eq!(list_json["entries"][0]["unit"], "lb");
    assert_eq!(list_json["entries"][0]["sourceUnit"], "lb");

    let aggregate_output = run_cli(&[
        "--base-url",
        &mock_server.uri(),
        "--email",
        TEST_EMAIL,
        "--password",
        TEST_PASSWORD,
        "weights",
        "aggregate",
        "--start",
        "2026-01-01",
        "--end",
        "2026-03-01",
    ]);
    assert!(
        aggregate_output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&aggregate_output.stderr)
    );
    let aggregate_json: Value =
        serde_json::from_slice(&aggregate_output.stdout).expect("valid JSON output");
    assert!(aggregate_json["entries"].is_array());
    assert!(aggregate_json["summary"]["first"].is_string());
    assert!(aggregate_json["summary"]["totalChange"].is_number());
}

#[test]
fn cli_setup_requires_non_interactive_without_tty() {
    let output = run_cli(&["setup"]);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("setup requires an interactive terminal")
    );
}

#[test]
fn cli_setup_non_interactive_writes_config() {
    let dir = tempdir().expect("temp dir");
    let config_path = dir.path().join("config.json");
    let config_path_str = config_path.to_str().expect("config path");

    let output = run_cli(&[
        "--email",
        TEST_EMAIL,
        "--password",
        TEST_PASSWORD,
        "--config-path",
        config_path_str,
        "setup",
        "--non-interactive",
    ]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).expect("setup json");
    assert_eq!(json["config_written"], true);
    assert_eq!(json["email_present"], true);
    assert_eq!(json["password_present"], true);

    let config = std::fs::read_to_string(config_path).expect("config written");
    let config_json: Value = serde_json::from_str(&config).expect("config json");
    assert_eq!(config_json["email"], TEST_EMAIL);
    assert_eq!(config_json["password"], TEST_PASSWORD);
    assert!(config_json["note_path"].is_null());
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
                { "entryTimestamp": "2026-01-02T10:00:00.000Z", "operationType": "create", "weight": 2000.0, "bmi": 287.0 },
                { "entryTimestamp": "2026-01-09T10:00:00.000Z", "operationType": "create", "weight": 1990.0, "bmi": 286.0 },
                { "entryTimestamp": "2026-01-16T10:00:00.000Z", "operationType": "create", "weight": 1980.0, "bmi": 284.0 },
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
