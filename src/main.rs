use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, Utc};
use clap::{Args, Parser, Subcommand, ValueEnum};
use regex::Regex;
use reqwest::header::{self, HeaderMap, HeaderValue};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{self, IsTerminal, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const KEYCHAIN_SERVICE_NAME: &str = "WeightGurus";
const WG_DEFAULT_BASE_URL: &str = "https://api.weightgurus.com";
const WG_DEFAULT_CONFIG_FILE: &str = "config.json";
const WG_DEFAULT_CONFIG_DIR: &str = "weight-gurus";
const WG_CONFIG_ENV_VAR: &str = "WEIGHT_GURUS_CONFIG_PATH";

#[derive(Parser)]
#[command(
    name = "weight-gurus-cli",
    about = "Fetch and normalize Weight Gurus measurement data."
)]
struct Cli {
    #[arg(long, env = "WEIGHT_GURUS_EMAIL")]
    email: Option<String>,
    #[arg(long, env = "WEIGHT_GURUS_PASSWORD")]
    password: Option<String>,
    #[arg(long, env = "WEIGHT_GURUS_BASE_URL")]
    base_url: Option<String>,
    #[arg(long, env = WG_CONFIG_ENV_VAR)]
    config_path: Option<String>,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Setup(SetupArgs),
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    Weights {
        #[command(subcommand)]
        command: WeightsCommand,
    },
}

#[derive(Subcommand)]
enum AuthCommand {
    Test,
    Status,
}

#[derive(Subcommand)]
enum WeightsCommand {
    #[command(about = "List normalized Weight Gurus measurements")]
    List(WeightListArgs),
    #[command(about = "Aggregate normalized measurements by day, week, or month")]
    Aggregate(WeightAggregateArgs),
}

#[derive(Args)]
struct TimeRange {
    #[arg(long)]
    start: Option<String>,
    #[arg(long)]
    end: Option<String>,
}

#[derive(Clone, Copy, Args)]
struct UnitArgs {
    #[arg(long, value_enum, default_value_t = SourceUnit::Auto)]
    source_unit: SourceUnit,
    #[arg(long, value_enum, default_value_t = OutputUnit::Lb)]
    unit: OutputUnit,
}

#[derive(Args)]
struct WeightListArgs {
    #[command(flatten)]
    range: TimeRange,
    #[command(flatten)]
    units: UnitArgs,
    #[arg(
        long,
        help = "Include delete operations instead of only active create operations"
    )]
    include_deleted: bool,
}

#[derive(Args)]
struct WeightAggregateArgs {
    #[command(flatten)]
    range: TimeRange,
    #[command(flatten)]
    units: UnitArgs,
    #[arg(long, value_enum, default_value_t = Bucket::Week)]
    bucket: Bucket,
    #[arg(
        long,
        help = "Include delete operations instead of only active create operations"
    )]
    include_deleted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
enum SourceUnit {
    Auto,
    Lb,
    Kg,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
enum OutputUnit {
    Lb,
    Kg,
    Native,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
enum Bucket {
    Day,
    Week,
    Month,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct WeightOperation {
    #[serde(rename = "entryTimestamp")]
    pub entry_timestamp: Option<String>,
    pub weight: Option<f64>,
    pub bmi: Option<f64>,
    #[serde(rename = "operationType")]
    pub operation_type: Option<String>,
    #[serde(rename = "entryValue")]
    pub entry_value: Option<f64>,
    pub value: Option<f64>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct WeightOperationResponse {
    #[serde(default)]
    operations: Vec<WeightOperation>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedConfig {
    pub email: Option<String>,
    pub password: Option<String>,
    pub base_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LoginResponse {
    #[serde(rename = "accessToken")]
    access_token: Option<String>,
}

#[derive(Debug, Serialize)]
struct SetupResult {
    action: &'static str,
    platform: &'static str,
    config_path: String,
    config_exists: bool,
    config_written: bool,
    email_present: bool,
    email_source: &'static str,
    password_present: bool,
    password_source: &'static str,
    base_url: String,
    next_steps: Vec<String>,
}

#[derive(Debug, Serialize)]
struct AuthStatusResult {
    can_authenticate: bool,
    email_present: bool,
    email_source: &'static str,
    password_present: bool,
    password_source: &'static str,
    base_url: String,
    config_file: Option<String>,
    config_exists: bool,
    keychain_supported: bool,
    keychain_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum CredentialSource {
    Cli,
    Config,
    Keychain,
    Missing,
}

impl CredentialSource {
    fn as_label(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Config => "config",
            Self::Keychain => "keychain",
            Self::Missing => "missing",
        }
    }
}

#[derive(Debug, Args)]
struct SetupArgs {
    #[arg(long, help = "Use supplied flags/env/config only; never prompt")]
    non_interactive: bool,
    #[arg(long, help = "Replace an existing config file without prompting")]
    overwrite: bool,
}

#[derive(Debug, Clone)]
struct Measurement {
    entry_timestamp: String,
    date: NaiveDate,
    raw_weight: f64,
    weight: f64,
    unit: Unit,
    inferred_source_unit: Unit,
    source_unit_confidence: &'static str,
    operation_type: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Unit {
    Lb,
    Kg,
}

#[derive(Debug, Serialize)]
struct MeasurementOutput {
    #[serde(rename = "entryTimestamp")]
    entry_timestamp: String,
    date: String,
    #[serde(rename = "rawWeight")]
    raw_weight: f64,
    #[serde(rename = "rawScale")]
    raw_scale: &'static str,
    weight: f64,
    unit: Unit,
    #[serde(rename = "sourceUnit")]
    source_unit: Unit,
    #[serde(rename = "sourceUnitConfidence")]
    source_unit_confidence: &'static str,
    #[serde(rename = "operationType", skip_serializing_if = "Option::is_none")]
    operation_type: Option<String>,
}

#[derive(Debug, Serialize)]
struct AggregateRow {
    bucket: String,
    #[serde(rename = "avgWeight")]
    avg_weight: f64,
    #[serde(rename = "minWeight")]
    min_weight: f64,
    #[serde(rename = "maxWeight")]
    max_weight: f64,
    count: usize,
    #[serde(rename = "diff")]
    diff: String,
}

#[derive(Debug, Serialize)]
struct AggregateSummary {
    first: Option<String>,
    last: Option<String>,
    #[serde(rename = "totalChange")]
    total_change: Option<f64>,
    unit: Unit,
}

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    let config_path = resolve_config_path(cli.config_path.as_deref());
    let config = match config_path.as_ref() {
        Some(path) => read_config_file(path)?,
        None => None,
    };
    let resolved_base_url = resolve_base_url(cli.base_url.as_deref(), config.as_ref());

    let payload = match &cli.command {
        Commands::Setup(args) => setup_command(
            &cli,
            args,
            config_path.as_deref(),
            &config,
            resolved_base_url.clone(),
        )?,
        Commands::Auth {
            command: AuthCommand::Status,
        } => auth_status(
            cli.email.clone(),
            cli.password.clone(),
            config_path.as_deref(),
            &config,
            resolved_base_url.clone(),
        )?,
        Commands::Auth {
            command: AuthCommand::Test,
        } => {
            let credentials = resolve_credentials_required(
                cli.email.clone(),
                cli.password.clone(),
                config.as_ref(),
            )?;
            let mut client = WeightGurusClient::new(
                &resolved_base_url,
                &credentials.email,
                &credentials.password,
            )
            .await?;
            client.login().await?;
            auth_test()?
        }
        command => {
            let credentials = resolve_credentials_required(
                cli.email.clone(),
                cli.password.clone(),
                config.as_ref(),
            )?;
            let mut client = WeightGurusClient::new(
                &resolved_base_url,
                &credentials.email,
                &credentials.password,
            )
            .await?;
            client.login().await?;

            match command {
                Commands::Auth {
                    command: AuthCommand::Test,
                } => unreachable!(),
                Commands::Auth {
                    command: AuthCommand::Status,
                } => unreachable!(),
                Commands::Weights {
                    command: WeightsCommand::List(args),
                } => {
                    let weights = client
                        .get_raw_weights(
                            parse_opt_start_datetime(&args.range.start)?,
                            parse_opt_end_datetime(&args.range.end)?,
                        )
                        .await?;
                    let measurements =
                        normalize_measurements(&weights, args.units, args.include_deleted)?;
                    serde_json::json!({
                        "entries": serialize_measurements(&measurements),
                        "count": measurements.len(),
                        "start": args.range.start.clone(),
                        "end": args.range.end.clone(),
                        "unit": resolved_output_unit(&measurements, args.units.unit),
                        "sourceUnit": inferred_source_unit_label(&measurements),
                    })
                }
                Commands::Weights {
                    command: WeightsCommand::Aggregate(args),
                } => {
                    let weights = client
                        .get_raw_weights(
                            parse_opt_start_datetime(&args.range.start)?,
                            parse_opt_end_datetime(&args.range.end)?,
                        )
                        .await?;
                    let measurements =
                        normalize_measurements(&weights, args.units, args.include_deleted)?;
                    let rows = aggregate_measurements(&measurements, args.bucket);
                    let unit = resolved_output_unit(&measurements, args.units.unit);
                    serde_json::json!({
                        "entries": serialize_aggregate_rows(&rows),
                        "summary": summarize_aggregate_rows(&rows, unit),
                        "count": rows.len(),
                        "bucket": args.bucket,
                        "start": args.range.start.clone(),
                        "end": args.range.end.clone(),
                        "unit": unit,
                        "sourceUnit": inferred_source_unit_label(&measurements),
                    })
                }
                _ => unreachable!(),
            }
        }
    };

    println!("{}", serde_json::to_string_pretty(&payload)?);
    Ok(())
}

fn auth_test() -> Result<Value> {
    Ok(serde_json::json!({
        "status": "ok",
        "message": "weight-gurus credentials validated"
    }))
}

fn setup_command(
    cli: &Cli,
    args: &SetupArgs,
    config_path: Option<&Path>,
    config: &Option<PersistedConfig>,
    base_url: String,
) -> Result<Value> {
    let resolved_email = pick_credential(
        "email",
        cli.email.clone(),
        config.as_ref(),
        |cfg| cfg.email.clone(),
        true,
    );
    let resolved_password = pick_credential(
        "password",
        cli.password.clone(),
        config.as_ref(),
        |cfg| cfg.password.clone(),
        true,
    );

    let mut email = resolved_email.0;
    let mut password = resolved_password.0;
    let mut email_source = resolved_email.1;
    let mut password_source = resolved_password.1;
    if !args.non_interactive && !io::stdin().is_terminal() {
        bail!(
            "setup requires an interactive terminal; use --non-interactive with --email and --password"
        );
    }

    if !args.non_interactive {
        if email.is_none() {
            email = prompt_if_missing("weight gurus email", email.as_deref())?;
            if email.is_some() {
                email_source = CredentialSource::Cli;
            }
        }
        if password.is_none() {
            password = prompt_if_missing("weight gurus password", password.as_deref())?;
            if password.is_some() {
                password_source = CredentialSource::Cli;
            }
        }
    }

    if email.is_none() || password.is_none() {
        bail!("setup requires email and password");
    }

    let path = match config_path {
        Some(path) => path.to_owned(),
        None => {
            let default = resolve_config_path(None)
                .context("could not determine default config path for setup")?;
            default
        }
    };

    if path.exists() && !args.overwrite {
        if args.non_interactive {
            bail!(
                "config file {} already exists; use --overwrite to replace",
                path.display()
            );
        }
        if !confirm(&format!(
            "config file {} already exists; replace it? [y/N]: ",
            path.display()
        ))? {
            bail!("setup cancelled");
        }
    }

    let persist = PersistedConfig {
        email: email.clone(),
        password: password.clone(),
        base_url: Some(base_url.clone()),
    };

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create config parent {}", parent.display()))?;
    }
    fs::write(&path, serde_json::to_vec_pretty(&persist)?)
        .with_context(|| format!("write config {}", path.display()))?;
    #[cfg(unix)]
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
        .context("set config permissions")?;
    let next_steps = vec![
        "weight-gurus-cli auth status".to_string(),
        "weight-gurus-cli weights list".to_string(),
        "weight-gurus-cli weights aggregate --bucket week".to_string(),
    ];

    let config_path_display = config_path
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| {
            resolve_config_path(None).as_ref().map_or_else(
                || "/tmp/weight-gurus/config.json".to_string(),
                |path| path.display().to_string(),
            )
        });
    let wrote_config = true;

    Ok(serde_json::to_value(SetupResult {
        action: "setup",
        platform: std::env::consts::OS,
        config_path: config_path_display,
        config_exists: config.is_some(),
        config_written: wrote_config,
        email_present: email.is_some(),
        email_source: email_source.as_label(),
        password_present: password.is_some(),
        password_source: password_source.as_label(),
        base_url,
        next_steps,
    })?)
}

fn auth_status(
    cli_email: Option<String>,
    cli_password: Option<String>,
    config_path: Option<&Path>,
    config: &Option<PersistedConfig>,
    base_url: String,
) -> Result<Value> {
    let (email_value, email_source) = pick_credential(
        "email",
        cli_email,
        config.as_ref(),
        |cfg| cfg.email.clone(),
        true,
    );
    let (password_value, password_source) = pick_credential(
        "password",
        cli_password,
        config.as_ref(),
        |cfg| cfg.password.clone(),
        true,
    );
    Ok(serde_json::to_value(AuthStatusResult {
        can_authenticate: email_value.is_some() && password_value.is_some(),
        email_present: email_value.is_some(),
        email_source: email_source.as_label(),
        password_present: password_value.is_some(),
        password_source: password_source.as_label(),
        base_url,
        config_file: config_path.map(|path| path.display().to_string()),
        config_exists: config.is_some(),
        keychain_supported: keychain_supported(),
        keychain_error: None,
    })?)
}

fn resolve_base_url(cli_base_url: Option<&str>, config: Option<&PersistedConfig>) -> String {
    cli_base_url
        .map(|value| value.to_string())
        .or_else(|| config.and_then(|cfg| cfg.base_url.clone()))
        .unwrap_or_else(|| WG_DEFAULT_BASE_URL.to_string())
}

fn resolve_config_path(explicit_path: Option<&str>) -> Option<PathBuf> {
    if let Some(path) = explicit_path {
        return Some(PathBuf::from(path));
    }
    if let Ok(path) = std::env::var(WG_CONFIG_ENV_VAR) {
        return Some(PathBuf::from(path));
    }

    #[cfg(target_os = "windows")]
    let home = std::env::var("APPDATA").ok();
    #[cfg(not(target_os = "windows"))]
    let home = std::env::var("HOME").ok();
    home.map(|h| {
        Path::new(&h)
            .join(".config")
            .join(WG_DEFAULT_CONFIG_DIR)
            .join(WG_DEFAULT_CONFIG_FILE)
    })
}

fn read_config_file(path: &Path) -> Result<Option<PersistedConfig>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read config file {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(
        serde_json::from_str::<PersistedConfig>(&raw)
            .with_context(|| format!("invalid JSON in config file {}", path.display()))?,
    ))
}

fn prompt_if_missing(label: &str, existing: Option<&str>) -> Result<Option<String>> {
    let prompt = match existing {
        Some(value) => format!("{label} [{}]: ", value),
        None => format!("{label}: "),
    };
    print!("{prompt}");
    io::stdout().flush().context("failed to write prompt")?;
    let mut value = String::new();
    io::stdin().read_line(&mut value)?;
    let value = value.trim().to_string();
    if value.is_empty() {
        Ok(existing.map(str::to_string))
    } else {
        Ok(Some(value))
    }
}

fn confirm(prompt: &str) -> Result<bool> {
    print!("{prompt}");
    io::stdout().flush().context("failed to write prompt")?;
    let mut value = String::new();
    io::stdin().read_line(&mut value)?;
    Ok(matches!(value.trim().to_lowercase().as_str(), "y" | "yes"))
}

fn pick_credential(
    label: &str,
    cli_value: Option<String>,
    config: Option<&PersistedConfig>,
    get_config: impl FnOnce(&PersistedConfig) -> Option<String>,
    allow_keychain: bool,
) -> (Option<String>, CredentialSource) {
    let mut resolved = match cli_value {
        Some(value) => (Some(value), CredentialSource::Cli),
        None => {
            if let Some(cfg) = config.and_then(get_config) {
                (Some(cfg), CredentialSource::Config)
            } else {
                (None, CredentialSource::Missing)
            }
        }
    };

    if resolved.0.is_none() && allow_keychain {
        match keychain_credentials(KEYCHAIN_SERVICE_NAME) {
            Ok((email, password)) => {
                let source_val = match label {
                    "email" => email,
                    "password" => password,
                    _ => String::new(),
                };
                resolved = if source_val.is_empty() {
                    (None, CredentialSource::Missing)
                } else {
                    (Some(source_val), CredentialSource::Keychain)
                };
            }
            Err(_) => {}
        }
    }

    resolved
}

fn resolve_credentials_required(
    cli_email: Option<String>,
    cli_password: Option<String>,
    config: Option<&PersistedConfig>,
) -> Result<CredentialStore> {
    let (email, email_source) =
        pick_credential("email", cli_email, config, |cfg| cfg.email.clone(), true);
    let (password, password_source) = pick_credential(
        "password",
        cli_password,
        config,
        |cfg| cfg.password.clone(),
        true,
    );
    let _ = email_source;
    let _ = password_source;

    match (email, password) {
        (Some(email), Some(password)) => Ok(CredentialStore { email, password }),
        (None, Some(_)) => bail!(
            "provided password only; use --email with --password or use config/keychain credentials"
        ),
        (Some(_), None) => bail!(
            "provided email only; use --password with --email or use config/keychain credentials"
        ),
        _ => {
            if !keychain_supported() {
                bail!(
                    "provide --email and --password, or set WEIGHT_GURUS_EMAIL and WEIGHT_GURUS_PASSWORD"
                );
            }
            bail!(
                "credentials missing; provide --email and --password, or set WEIGHT_GURUS_EMAIL and WEIGHT_GURUS_PASSWORD"
            )
        }
    }
}

#[derive(Debug)]
struct CredentialStore {
    email: String,
    password: String,
}

#[cfg(target_os = "macos")]
fn keychain_credentials(service: &str) -> Result<(String, String)> {
    let output = Command::new("security")
        .args(["find-generic-password", "-s", service, "-g"])
        .output()
        .with_context(|| "failed to execute macOS security command")?;

    if !output.status.success() {
        bail!(
            "could not read credentials from keychain service '{service}': {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    let acct_re = Regex::new(r#""acct"<blob>="([^"]+)""#).context("invalid account regex")?;
    let email = acct_re
        .captures(&stdout)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
        .context("could not find account email in keychain output")?;

    let password_raw = stderr
        .lines()
        .find_map(|line| {
            line.strip_prefix("password:")
                .map(|value| value.trim().trim_start_matches(" ").to_string())
        })
        .context("could not find password in keychain output")?;

    let password = if password_raw.starts_with('"') && password_raw.ends_with('"') {
        password_raw[1..password_raw.len() - 1].to_string()
    } else if password_raw.starts_with("0x") {
        decode_security_hex_password(&password_raw)?
    } else {
        password_raw.to_string()
    };

    Ok((email, password))
}

#[cfg(not(target_os = "macos"))]
fn keychain_credentials(_service: &str) -> Result<(String, String)> {
    bail!(
        "no native credential lookup is configured for this platform; provide --email and --password or set WEIGHT_GURUS_EMAIL and WEIGHT_GURUS_PASSWORD"
    )
}

#[cfg(target_os = "macos")]
fn keychain_supported() -> bool {
    true
}

#[cfg(not(target_os = "macos"))]
fn keychain_supported() -> bool {
    false
}

#[cfg(target_os = "macos")]
fn decode_security_hex_password(raw: &str) -> Result<String> {
    let hex = raw.trim_start_matches("0x").replace(' ', "");
    if (hex.len() & 1) != 0 {
        bail!("invalid hex password output");
    }

    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for i in (0..hex.len()).step_by(2) {
        let chunk = &hex[i..i + 2];
        let byte =
            u8::from_str_radix(chunk, 16).with_context(|| format!("invalid hex byte: {chunk}"))?;
        bytes.push(byte);
    }

    String::from_utf8(bytes).context("password bytes are not valid UTF-8")
}

struct WeightGurusClient {
    client: Client,
    base_url: String,
    email: String,
    password: String,
    token: Option<String>,
}

impl WeightGurusClient {
    async fn new(base_url: &str, email: &str, password: &str) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ACCEPT,
            HeaderValue::from_static("application/json, text/plain, */*"),
        );
        let client = Client::builder()
            .default_headers(headers)
            .user_agent("weight-gurus-cli")
            .build()
            .context("failed to build HTTP client")?;

        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            email: email.to_string(),
            password: password.to_string(),
            token: None,
        })
    }

    async fn login(&mut self) -> Result<()> {
        let login_url = format!("{}/v3/account/login", self.base_url);
        let response = self
            .client
            .post(&login_url)
            .form(&[
                ("email", self.email.as_str()),
                ("password", self.password.as_str()),
            ])
            .send()
            .await
            .context("login request failed")?;

        if !response.status().is_success() {
            bail!("login failed: {}", response.status());
        }

        let payload: LoginResponse = response
            .json()
            .await
            .context("failed to parse login response")?;
        self.token = Some(
            payload
                .access_token
                .context("login response did not include accessToken")?,
        );
        Ok(())
    }

    async fn get_raw_weights(
        &mut self,
        start: Option<DateTime<Utc>>,
        end: Option<DateTime<Utc>>,
    ) -> Result<Vec<WeightOperation>> {
        let token = self
            .token
            .as_ref()
            .context("not authenticated: call login first")?;

        let weight_url = format!("{}/v3/operation/", self.base_url);
        let response = self
            .client
            .get(&weight_url)
            .bearer_auth(token)
            .send()
            .await
            .context("fetch operations request failed")?;

        if !response.status().is_success() {
            bail!("failed to fetch operations: {}", response.status());
        }

        let payload: WeightOperationResponse = response
            .json()
            .await
            .context("failed to parse operations response")?;

        let mut result = Vec::new();

        for operation in payload.operations {
            let Some(entry_str) = operation.entry_timestamp.as_deref() else {
                continue;
            };
            let entry_time = parse_datetime_utc(entry_str)
                .with_context(|| format!("failed to parse entry timestamp '{entry_str}'"))?;

            if let Some(start_time) = start {
                if entry_time < start_time {
                    continue;
                }
            }
            if let Some(end_time) = end {
                if entry_time > end_time {
                    continue;
                }
            }
            result.push(operation);
        }

        Ok(result)
    }
}

fn parse_opt_start_datetime(input: &Option<String>) -> Result<Option<DateTime<Utc>>> {
    let Some(value) = input.as_deref() else {
        return Ok(None);
    };

    Ok(Some(parse_datetime_utc(value).with_context(|| {
        format!("failed to parse datetime '{value}'")
    })?))
}

fn parse_opt_end_datetime(input: &Option<String>) -> Result<Option<DateTime<Utc>>> {
    let Some(value) = input.as_deref() else {
        return Ok(None);
    };

    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        let next_midnight = date
            .checked_add_signed(Duration::days(1))
            .context("invalid end date")?
            .and_hms_milli_opt(0, 0, 0, 0)
            .context("invalid end date")?;
        return Ok(Some(DateTime::from_naive_utc_and_offset(
            next_midnight - Duration::milliseconds(1),
            Utc,
        )));
    }

    Ok(Some(parse_datetime_utc(value).with_context(|| {
        format!("failed to parse datetime '{value}'")
    })?))
}

fn parse_datetime_utc(input: &str) -> Result<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(input) {
        return Ok(dt.with_timezone(&Utc));
    }

    let formats = [
        "%Y-%m-%dT%H:%M:%S%.3f",
        "%Y-%m-%dT%H:%M:%S%.6f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d",
    ];

    for format in formats {
        if format == "%Y-%m-%d" {
            if let Ok(date) = NaiveDate::parse_from_str(input, format) {
                let midnight = date.and_hms_opt(0, 0, 0).context("invalid date")?;
                return Ok(DateTime::from_naive_utc_and_offset(midnight, Utc));
            }
            continue;
        }

        if let Ok(naive_dt) = NaiveDateTime::parse_from_str(input, format) {
            return Ok(DateTime::from_naive_utc_and_offset(naive_dt, Utc));
        }
    }

    Err(anyhow!("unsupported date format: {input}"))
}

fn extract_weight_value(op: &WeightOperation) -> Result<f64> {
    if let Some(weight) = op.weight {
        return Ok(weight);
    }
    if let Some(weight) = op.entry_value {
        return Ok(weight);
    }
    op.value.context("operation missing weight field")
}

fn normalize_measurements(
    operations: &[WeightOperation],
    units: UnitArgs,
    include_deleted: bool,
) -> Result<Vec<Measurement>> {
    let source_unit = infer_source_unit(operations, units.source_unit);
    let output_unit = match units.unit {
        OutputUnit::Lb => Unit::Lb,
        OutputUnit::Kg => Unit::Kg,
        OutputUnit::Native => source_unit.0,
    };
    let mut measurements = Vec::new();
    for op in operations {
        if !include_deleted && op.operation_type.as_deref() == Some("delete") {
            continue;
        }
        let Some(entry_timestamp) = op.entry_timestamp.as_deref() else {
            continue;
        };
        let entry_time = match parse_datetime_utc(entry_timestamp) {
            Ok(dt) => dt,
            Err(_) => continue,
        };
        let raw_weight = match extract_weight_value(op) {
            Ok(weight) => weight,
            Err(_) => continue,
        };
        if raw_weight.is_nan() {
            continue;
        }

        let native_weight = raw_weight / 10.0;
        let weight = convert_weight(native_weight, source_unit.0, output_unit);
        measurements.push(Measurement {
            entry_timestamp: entry_timestamp.to_string(),
            date: entry_time.date_naive(),
            raw_weight,
            weight,
            unit: output_unit,
            inferred_source_unit: source_unit.0,
            source_unit_confidence: source_unit.1,
            operation_type: op.operation_type.clone(),
        });
    }
    measurements.sort_by(|a, b| a.entry_timestamp.cmp(&b.entry_timestamp));
    Ok(measurements)
}

fn infer_source_unit(
    operations: &[WeightOperation],
    requested: SourceUnit,
) -> (Unit, &'static str) {
    match requested {
        SourceUnit::Lb => return (Unit::Lb, "user"),
        SourceUnit::Kg => return (Unit::Kg, "user"),
        SourceUnit::Auto => {}
    }

    for op in operations {
        let (Ok(raw_weight), Some(raw_bmi)) = (extract_weight_value(op), op.bmi) else {
            continue;
        };
        if raw_weight <= 0.0 || raw_bmi <= 0.0 {
            continue;
        }
        let weight = raw_weight / 10.0;
        let bmi = raw_bmi / 10.0;
        let height_if_lb_inches = ((weight * 703.0) / bmi).sqrt();
        let height_if_kg_meters = (weight / bmi).sqrt();
        let lb_plausible = (48.0..=84.0).contains(&height_if_lb_inches);
        let kg_plausible = (1.35..=2.20).contains(&height_if_kg_meters);
        match (lb_plausible, kg_plausible) {
            (true, false) => return (Unit::Lb, "bmi"),
            (false, true) => return (Unit::Kg, "bmi"),
            _ => {}
        }
    }

    for op in operations {
        if let Ok(raw_weight) = extract_weight_value(op) {
            let scaled = raw_weight / 10.0;
            if scaled >= 300.0 {
                return (Unit::Kg, "magnitude");
            }
            if scaled >= 90.0 {
                return (Unit::Lb, "magnitude");
            }
        }
    }

    (Unit::Lb, "default")
}

fn convert_weight(value: f64, from: Unit, to: Unit) -> f64 {
    match (from, to) {
        (Unit::Lb, Unit::Lb) | (Unit::Kg, Unit::Kg) => value,
        (Unit::Lb, Unit::Kg) => value * 0.453_592_37,
        (Unit::Kg, Unit::Lb) => value / 0.453_592_37,
    }
}

fn resolved_output_unit(measurements: &[Measurement], requested: OutputUnit) -> Unit {
    match requested {
        OutputUnit::Lb => Unit::Lb,
        OutputUnit::Kg => Unit::Kg,
        OutputUnit::Native => measurements
            .first()
            .map(|measurement| measurement.inferred_source_unit)
            .unwrap_or(Unit::Lb),
    }
}

fn inferred_source_unit_label(measurements: &[Measurement]) -> Option<Unit> {
    measurements
        .first()
        .map(|measurement| measurement.inferred_source_unit)
}

fn serialize_measurements(measurements: &[Measurement]) -> Vec<MeasurementOutput> {
    measurements
        .iter()
        .map(|measurement| MeasurementOutput {
            entry_timestamp: measurement.entry_timestamp.clone(),
            date: measurement.date.format("%Y-%m-%d").to_string(),
            raw_weight: measurement.raw_weight,
            raw_scale: "tenths",
            weight: round_to_one_dp(measurement.weight),
            unit: measurement.unit,
            source_unit: measurement.inferred_source_unit,
            source_unit_confidence: measurement.source_unit_confidence,
            operation_type: measurement.operation_type.clone(),
        })
        .collect()
}

fn aggregate_measurements(measurements: &[Measurement], bucket: Bucket) -> Vec<AggregateRow> {
    let mut grouped: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for measurement in measurements {
        grouped
            .entry(bucket_key(measurement.date, bucket))
            .or_default()
            .push(measurement.weight);
    }

    let mut previous_avg: Option<f64> = None;
    grouped
        .into_iter()
        .map(|(bucket, values)| {
            let avg = values.iter().sum::<f64>() / values.len() as f64;
            let min = values.iter().copied().fold(f64::INFINITY, f64::min);
            let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let rounded_avg = round_to_one_dp(avg);
            let diff = previous_avg
                .map(|previous| format!("{:+.1}", rounded_avg - previous))
                .unwrap_or_else(|| "-".to_string());
            previous_avg = Some(rounded_avg);
            AggregateRow {
                bucket,
                avg_weight: rounded_avg,
                min_weight: round_to_one_dp(min),
                max_weight: round_to_one_dp(max),
                count: values.len(),
                diff,
            }
        })
        .collect()
}

fn serialize_aggregate_rows(rows: &[AggregateRow]) -> Vec<&AggregateRow> {
    rows.iter().collect()
}

fn summarize_aggregate_rows(rows: &[AggregateRow], unit: Unit) -> AggregateSummary {
    if rows.is_empty() {
        return AggregateSummary {
            first: None,
            last: None,
            total_change: None,
            unit,
        };
    }
    let first = rows.first().expect("checked non-empty");
    let last = rows.last().expect("checked non-empty");
    AggregateSummary {
        first: Some(first.bucket.clone()),
        last: Some(last.bucket.clone()),
        total_change: Some(round_to_one_dp(last.avg_weight - first.avg_weight)),
        unit,
    }
}

fn bucket_key(date: NaiveDate, bucket: Bucket) -> String {
    match bucket {
        Bucket::Day => date.format("%Y-%m-%d").to_string(),
        Bucket::Week => sunday_week_end(date).format("%Y-%m-%d").to_string(),
        Bucket::Month => date.format("%Y-%m").to_string(),
    }
}

fn sunday_week_end(date: NaiveDate) -> NaiveDate {
    let weekday = date.weekday().num_days_from_monday() as i64;
    let days_until_sunday = 6 - weekday;
    date.checked_add_signed(Duration::days(days_until_sunday))
        .unwrap_or(date)
}

fn round_to_one_dp(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalizes_live_api_tenths_to_lbs() {
        let rows = vec![
            test_operation("2026-04-28T12:35:00.000Z", 1782.0, 255.0),
            test_operation("2026-04-27T12:57:00.000Z", 1782.0, 255.0),
        ];

        let parsed = normalize_measurements(
            &rows,
            UnitArgs {
                source_unit: SourceUnit::Auto,
                unit: OutputUnit::Lb,
            },
            false,
        )
        .expect("rows should parse");

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].inferred_source_unit, Unit::Lb);
        assert_eq!(parsed[0].source_unit_confidence, "bmi");
        assert!((parsed[0].weight - 178.2).abs() < 0.01);
    }

    #[test]
    fn test_can_convert_lbs_to_kg() {
        let rows = vec![test_operation("2026-04-28T12:35:00.000Z", 1782.0, 255.0)];
        let parsed = normalize_measurements(
            &rows,
            UnitArgs {
                source_unit: SourceUnit::Auto,
                unit: OutputUnit::Kg,
            },
            false,
        )
        .expect("rows should parse");

        assert!((parsed[0].weight - 80.8).abs() < 0.1);
        assert_eq!(parsed[0].unit, Unit::Kg);
    }

    #[test]
    fn test_weekly_aggregation_is_generic_bucketed_output() {
        let rows = vec![
            test_operation("2026-01-05T10:00:00.000Z", 2000.0, 287.0),
            test_operation("2026-01-08T10:00:00.000Z", 1990.0, 286.0),
            test_operation("2026-01-12T10:00:00.000Z", 1980.0, 284.0),
        ];
        let parsed = normalize_measurements(
            &rows,
            UnitArgs {
                source_unit: SourceUnit::Lb,
                unit: OutputUnit::Lb,
            },
            false,
        )
        .expect("rows should parse");
        let aggregate = aggregate_measurements(&parsed, Bucket::Week);

        assert_eq!(aggregate.len(), 2);
        assert_eq!(aggregate[0].bucket, "2026-01-11");
        assert_eq!(aggregate[0].avg_weight, 199.5);
        assert_eq!(aggregate[0].min_weight, 199.0);
        assert_eq!(aggregate[0].max_weight, 200.0);
        assert_eq!(aggregate[1].bucket, "2026-01-18");
        assert_eq!(aggregate[1].diff, "-1.5");
    }

    fn test_operation(timestamp: &str, weight: f64, bmi: f64) -> WeightOperation {
        WeightOperation {
            entry_timestamp: Some(timestamp.to_string()),
            weight: Some(weight),
            bmi: Some(bmi),
            operation_type: Some("create".to_string()),
            entry_value: None,
            value: None,
            extra: HashMap::new(),
        }
    }
}
