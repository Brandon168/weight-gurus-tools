use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, Utc};
use clap::{Args, Parser, Subcommand};
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
const CHART_LINE_COLOR: &str = "#38bdf8";
const WG_DEFAULT_BASE_URL: &str = "https://api.weightgurus.com";
const WG_DEFAULT_CONFIG_FILE: &str = "config.json";
const WG_DEFAULT_CONFIG_DIR: &str = "weight-gurus";
const WG_CONFIG_ENV_VAR: &str = "WEIGHT_GURUS_CONFIG_PATH";

#[derive(Parser)]
#[command(
    name = "weight-gurus-cli",
    about = "Fetch Weight Gurus data and update markdown logs."
)]
struct Cli {
    #[arg(long, env = "WEIGHT_GURUS_EMAIL")]
    email: Option<String>,
    #[arg(long, env = "WEIGHT_GURUS_PASSWORD")]
    password: Option<String>,
    #[arg(long, env = "WEIGHT_GURUS_BASE_URL")]
    base_url: Option<String>,
    #[arg(long, env = "WEIGHT_GURUS_NOTE_PATH")]
    note_path: Option<String>,
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
    Vault {
        #[command(subcommand)]
        command: VaultCommand,
    },
}

#[derive(Subcommand)]
enum AuthCommand {
    Test,
    Status,
}

#[derive(Subcommand)]
enum WeightsCommand {
    Raw(TimeRange),
    Weekly(TimeRange),
}

#[derive(Subcommand)]
enum VaultCommand {
    Preview(VaultArgs),
    Update(VaultUpdateArgs),
}

#[derive(Args)]
struct TimeRange {
    #[arg(long)]
    start: Option<String>,
    #[arg(long)]
    end: Option<String>,
}

#[derive(Args)]
struct VaultArgs {
    #[arg(long)]
    file: Option<String>,
    #[arg(long)]
    start: Option<String>,
    #[arg(long)]
    end: Option<String>,
}

#[derive(Args)]
struct VaultUpdateArgs {
    #[command(flatten)]
    args: VaultArgs,
    #[arg(long)]
    confirm: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct WeightOperation {
    #[serde(rename = "entryTimestamp")]
    pub entry_timestamp: Option<String>,
    pub weight: Option<f64>,
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
    pub note_path: Option<String>,
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
    note_path: Option<String>,
    note_path_source: &'static str,
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
    note_path: Option<String>,
    note_path_source: &'static str,
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
    #[arg(long, help = "Markdown note path to save in the config file")]
    note_path: Option<String>,
}

#[derive(Debug, Clone)]
struct ParsedWeight {
    week_ending: NaiveDate,
    avg_weight: f64,
    low_weight: f64,
}

#[derive(Debug)]
struct SectionMatch {
    start: usize,
    end: usize,
    text: String,
}

#[derive(Debug, Serialize)]
struct WeeklyRow {
    #[serde(rename = "weekEnding")]
    week_ending: String,
    #[serde(rename = "avgWeight")]
    avg_weight: f64,
    #[serde(rename = "lowWeight")]
    low_weight: f64,
    #[serde(rename = "diff")]
    diff: String,
}

#[derive(Debug, Serialize)]
struct WeeklySummary {
    #[serde(rename = "firstWeek")]
    first_week: Option<String>,
    #[serde(rename = "lastWeek")]
    last_week: Option<String>,
    #[serde(rename = "totalLoss")]
    total_loss: Option<f64>,
    #[serde(rename = "avgWeeklyLoss")]
    avg_weekly_loss: Option<f64>,
}

#[derive(Debug, Serialize)]
struct VaultUpdateResult {
    file: String,
    section_found: bool,
    rows: usize,
    has_update: bool,
    mermaid_blocks_removed: usize,
    updated_section_preview: String,
    output: Option<String>,
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
            cli.note_path.as_deref(),
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
                    command: WeightsCommand::Raw(range),
                } => {
                    let weights = client
                        .get_raw_weights(
                            parse_opt_datetime(&range.start)?,
                            parse_opt_datetime(&range.end)?,
                        )
                        .await?;
                    serde_json::json!({
                        "entries": weights,
                        "count": weights.len(),
                        "start": range.start.clone(),
                        "end": range.end.clone(),
                    })
                }
                Commands::Weights {
                    command: WeightsCommand::Weekly(range),
                } => {
                    let weights = client
                        .get_raw_weights(
                            parse_opt_datetime(&range.start)?,
                            parse_opt_datetime(&range.end)?,
                        )
                        .await?;
                    let rows = build_weekly_rows(&weights)?;
                    let filtered_rows =
                        filter_rows_after(&rows, parse_opt_naive_date(&range.start)?);
                    let serialized_rows: Vec<WeeklyRow> = serialize_weekly_rows(&filtered_rows);
                    serde_json::json!({
                        "entries": serialized_rows,
                        "summary": summarize_rows(&filtered_rows),
                        "count": filtered_rows.len(),
                        "start": range.start.clone(),
                        "end": range.end.clone(),
                    })
                }
                Commands::Vault {
                    command: VaultCommand::Preview(args),
                } => {
                    let file = resolve_note_path(
                        args.file.as_deref(),
                        cli.note_path.as_deref(),
                        config.as_ref(),
                    )?;
                    let operation_rows = client
                        .get_raw_weights(
                            parse_opt_datetime(&args.start)?,
                            parse_opt_datetime(&args.end)?,
                        )
                        .await?;
                    let weekly_rows = build_weekly_rows(&operation_rows)?;
                    let explicit_start = parse_opt_naive_date(&args.start)?;
                    let result = preview_vault_update(&file, &weekly_rows, explicit_start)?;
                    serde_json::to_value(result)?
                }
                Commands::Vault {
                    command: VaultCommand::Update(args),
                } => {
                    if !args.confirm {
                        bail!("vault update requires --confirm");
                    }
                    let file = resolve_note_path(
                        args.args.file.as_deref(),
                        cli.note_path.as_deref(),
                        config.as_ref(),
                    )?;
                    let operation_rows = client
                        .get_raw_weights(
                            parse_opt_datetime(&args.args.start)?,
                            parse_opt_datetime(&args.args.end)?,
                        )
                        .await?;
                    let weekly_rows = build_weekly_rows(&operation_rows)?;
                    let explicit_start = parse_opt_naive_date(&args.args.start)?;
                    let result = apply_vault_update(&file, &weekly_rows, explicit_start)?;
                    serde_json::to_value(result)?
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
    let mut note_path = args
        .note_path
        .clone()
        .or(cli.note_path.clone())
        .or_else(|| config.as_ref().and_then(|c| c.note_path.clone()));

    if !args.non_interactive && !io::stdin().is_terminal() {
        bail!(
            "setup requires an interactive terminal; use --non-interactive with --email, --password, and --note-path"
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
        if note_path.is_none() {
            note_path = prompt_if_missing("weight log note path", None)?;
        }
    }

    if email.is_none() || password.is_none() || note_path.is_none() {
        bail!("setup requires email, password, and note path");
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
        note_path: note_path.clone(),
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
        "weight-gurus-cli weights weekly".to_string(),
        "weight-gurus-cli vault preview --file /path/to/note.md".to_string(),
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
        note_path: note_path.clone(),
        note_path_source: if args.note_path.is_some() {
            "cli"
        } else if cli.note_path.is_some() {
            "cli"
        } else if config
            .as_ref()
            .and_then(|cfg| cfg.note_path.as_ref())
            .is_some()
        {
            "config"
        } else {
            "missing"
        },
        next_steps,
    })?)
}

fn auth_status(
    cli_email: Option<String>,
    cli_password: Option<String>,
    config_path: Option<&Path>,
    config: &Option<PersistedConfig>,
    cli_note_path: Option<&str>,
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
    let note_path = cli_note_path
        .map(str::to_string)
        .or_else(|| config.as_ref().and_then(|cfg| cfg.note_path.clone()));

    Ok(serde_json::to_value(AuthStatusResult {
        can_authenticate: email_value.is_some() && password_value.is_some(),
        email_present: email_value.is_some(),
        email_source: email_source.as_label(),
        password_present: password_value.is_some(),
        password_source: password_source.as_label(),
        base_url,
        note_path: note_path.clone(),
        note_path_source: if cli_note_path.is_some() {
            "cli"
        } else if note_path.is_some() {
            "config"
        } else {
            "missing"
        },
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

fn resolve_note_path(
    file: Option<&str>,
    fallback: Option<&str>,
    config: Option<&PersistedConfig>,
) -> Result<String> {
    let path = file
        .or(fallback)
        .or_else(|| config.and_then(|cfg| cfg.note_path.as_deref()));
    let Some(path) = path else {
        bail!("provide --file or set WEIGHT_GURUS_NOTE_PATH");
    };
    Ok(path.to_string())
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

fn parse_opt_datetime(input: &Option<String>) -> Result<Option<DateTime<Utc>>> {
    let Some(value) = input.as_deref() else {
        return Ok(None);
    };

    Ok(Some(parse_datetime_utc(value).with_context(|| {
        format!("failed to parse datetime '{value}'")
    })?))
}

fn parse_opt_naive_date(input: &Option<String>) -> Result<Option<NaiveDate>> {
    let Some(value) = input.as_deref() else {
        return Ok(None);
    };

    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        return Ok(Some(date));
    }
    if let Ok(dt) = parse_datetime_utc(value) {
        return Ok(Some(dt.date_naive()));
    }

    bail!("failed to parse date '{value}'");
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

fn detect_scale_factor(operations: &[WeightOperation]) -> f64 {
    for op in operations {
        if let Ok(sample) = extract_weight_value(op) {
            return if sample > 1000.0 { 0.1 } else { 1.0 };
        }
    }
    1.0
}

fn build_weekly_rows(operations: &[WeightOperation]) -> Result<Vec<ParsedWeight>> {
    let scale = detect_scale_factor(operations);
    let mut grouped: BTreeMap<NaiveDate, Vec<f64>> = BTreeMap::new();

    for op in operations {
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

        let week_ending = sunday_week_end(entry_time);
        grouped
            .entry(week_ending)
            .or_default()
            .push(raw_weight * scale);
    }

    let mut rows: Vec<ParsedWeight> = grouped
        .into_iter()
        .map(|(week_ending, values)| {
            let avg = values.iter().sum::<f64>() / values.len() as f64;
            let low = values.iter().copied().fold(f64::INFINITY, f64::min);
            ParsedWeight {
                week_ending,
                avg_weight: avg,
                low_weight: low,
            }
        })
        .collect();

    rows.sort_by_key(|r| r.week_ending);
    Ok(rows)
}

fn sunday_week_end(date_time: DateTime<Utc>) -> NaiveDate {
    let weekday = date_time.date_naive().weekday().num_days_from_monday() as i64;
    let days_until_sunday = 6 - weekday;
    date_time
        .date_naive()
        .checked_add_signed(Duration::days(days_until_sunday))
        .unwrap_or_else(|| date_time.date_naive())
}

fn summarize_rows(rows: &[ParsedWeight]) -> WeeklySummary {
    if rows.is_empty() {
        return WeeklySummary {
            first_week: None,
            last_week: None,
            total_loss: None,
            avg_weekly_loss: None,
        };
    }

    let first = &rows[0];
    let last = rows.last().unwrap_or(&rows[0]);
    let total_loss = first.avg_weight - last.avg_weight;
    let weeks = (last.week_ending - first.week_ending).num_days() as f64 / 7.0;
    let avg_weekly_loss = if weeks > 0.0 {
        Some(total_loss / weeks)
    } else {
        None
    };

    WeeklySummary {
        first_week: Some(first.week_ending.format("%Y-%m-%d").to_string()),
        last_week: Some(last.week_ending.format("%Y-%m-%d").to_string()),
        total_loss: Some(total_loss),
        avg_weekly_loss,
    }
}

fn serialize_weekly_rows(rows: &[ParsedWeight]) -> Vec<WeeklyRow> {
    let mut prev_avg: Option<f64> = None;
    rows.iter()
        .map(|row| {
            let diff = prev_avg
                .map(|prev| {
                    let delta = row.avg_weight - prev;
                    format!("{:+.1} lbs", delta)
                })
                .unwrap_or_else(|| "-".to_string());
            prev_avg = Some(row.avg_weight);

            WeeklyRow {
                week_ending: row.week_ending.format("%Y-%m-%d").to_string(),
                avg_weight: round_to_one_dp(row.avg_weight),
                low_weight: round_to_one_dp(row.low_weight),
                diff,
            }
        })
        .collect()
}

fn filter_rows_after(rows: &[ParsedWeight], start: Option<NaiveDate>) -> Vec<ParsedWeight> {
    let mut filtered = rows.to_vec();
    if let Some(start_date) = start {
        filtered.retain(|row| row.week_ending >= start_date);
    }
    filtered
}

fn round_to_one_dp(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

fn preview_vault_update(
    file: &str,
    rows: &[ParsedWeight],
    explicit_start: Option<NaiveDate>,
) -> Result<VaultUpdateResult> {
    let content = read_note(file)?;
    let section = find_weight_section(&content)?;
    let updated = build_updated_section(&section.text, rows, explicit_start)?;

    let output = format!(
        "{}{}{}",
        &content[..section.start],
        &updated.section_text,
        &content[section.end..]
    );

    Ok(VaultUpdateResult {
        file: file.to_string(),
        section_found: true,
        rows: rows.len(),
        has_update: true,
        mermaid_blocks_removed: updated.mermaid_blocks_removed,
        updated_section_preview: updated.section_text,
        output: Some(output),
    })
}

fn apply_vault_update(
    file: &str,
    rows: &[ParsedWeight],
    explicit_start: Option<NaiveDate>,
) -> Result<VaultUpdateResult> {
    let content = read_note(file)?;
    let section = find_weight_section(&content)?;
    let updated = build_updated_section(&section.text, rows, explicit_start)?;
    let output_content = format!(
        "{}{}{}",
        &content[..section.start],
        &updated.section_text,
        &content[section.end..]
    );

    fs::write(file, output_content)
        .with_context(|| format!("failed to write updated markdown to {file}"))?;

    Ok(VaultUpdateResult {
        file: file.to_string(),
        section_found: true,
        rows: rows.len(),
        has_update: true,
        mermaid_blocks_removed: updated.mermaid_blocks_removed,
        updated_section_preview: String::new(),
        output: None,
    })
}

fn read_note(path: &str) -> Result<String> {
    if !Path::new(path).exists() {
        bail!("markdown file not found: {path}");
    }
    fs::read_to_string(path).with_context(|| format!("failed to read markdown file: {path}"))
}

#[derive(Debug)]
struct SectionUpdate {
    section_text: String,
    mermaid_blocks_removed: usize,
}

fn build_updated_section(
    section_text: &str,
    rows: &[ParsedWeight],
    explicit_start: Option<NaiveDate>,
) -> Result<SectionUpdate> {
    let existing_rows = parse_weekly_table(section_text);
    let mut rows_map: BTreeMap<NaiveDate, (f64, f64)> = existing_rows
        .into_iter()
        .map(|(date, row)| (date, (row.avg_weight, row.low_weight)))
        .collect();

    let cutoff = explicit_start.or_else(|| rows_map.keys().next().copied());
    for row in rows {
        if let Some(cutoff) = cutoff {
            if row.week_ending < cutoff {
                continue;
            }
        }
        rows_map.insert(row.week_ending, (row.avg_weight, row.low_weight));
    }

    let sorted: Vec<(NaiveDate, (f64, f64))> = rows_map.into_iter().collect();
    let mut updated_lines = vec![
        "| **Week Ending** | **Avg Weight** | **Diff** |     | ***Low Weight*** |".to_string(),
        "| --------------- | -------------- | -------- | --- | ---------------- |".to_string(),
    ];

    let mut prev_avg: Option<f64> = None;
    let mut chart_values: Vec<f64> = Vec::new();
    for (date, (avg, low)) in sorted.iter() {
        let rounded_avg = round_to_one_dp(*avg);
        let rounded_low = round_to_one_dp(*low);
        let diff = prev_avg
            .map(|prev| {
                let d = rounded_avg - prev;
                if d == 0.0 {
                    "0.0 lbs".to_string()
                } else if d > 0.0 {
                    format!("{d:+.1} lbs")
                } else {
                    format!("{d:.1} lbs")
                }
            })
            .unwrap_or_else(|| "-".to_string());

        prev_avg = Some(rounded_avg);
        chart_values.push(rounded_avg);

        updated_lines.push(format!(
            "| {:<15} | {:.1} lbs      | {:<8} |     | *{:.1} lbs*      |",
            date.format("%Y-%m-%d"),
            rounded_avg,
            diff,
            rounded_low
        ));
    }

    let new_table = updated_lines.join("\n");
    let mermaid_re = Regex::new(r"(?s)```mermaid.*?```").context("invalid mermaid regex")?;
    let mermaid_blocks_removed = mermaid_re.find_iter(section_text).count();
    let section_without_mermaid = mermaid_re.replace_all(section_text, "").to_string();

    let table_re = Regex::new(r"(?m)^\|\s*\*\*Week Ending\*\*.*\n\|[^\n]*\n(?:\|.*\n)*")
        .context("invalid table regex")?;
    let with_table = if table_re.is_match(&section_without_mermaid) {
        table_re
            .replace(&section_without_mermaid, new_table.as_str())
            .to_string()
    } else {
        section_without_mermaid + "\n\n" + &new_table
    };

    let with_chart = if !chart_values.is_empty() {
        let values = chart_values
            .iter()
            .map(|value| format!("{:.1}", value))
            .collect::<Vec<_>>()
            .join(", ");
        let first = sorted
            .first()
            .map(|(d, _)| d.format("%m/%d").to_string())
            .unwrap_or_default();
        let last = sorted
            .last()
            .map(|(d, _)| d.format("%m/%d").to_string())
            .unwrap_or_default();
        let min = sorted
            .iter()
            .map(|(_, (avg, _))| *avg)
            .fold(f64::INFINITY, f64::min);
        let max = sorted
            .iter()
            .map(|(_, (avg, _))| *avg)
            .fold(f64::NEG_INFINITY, f64::max);
        let init = serde_json::json!({
            "themeVariables": {
                "xyChart": {
                    "plotColorPalette": CHART_LINE_COLOR,
                }
            }
        });
        let chart = format!(
            "```mermaid\n%%{{init: {}}}%%\nxychart-beta\n  title \"Average Weight by Week ({}-{})\"\n  x-axis \"Week\" 1 --> {}\n  y-axis \"Weight (lbs)\" {} --> {}\n  line [{}]\n```",
            init,
            first,
            last,
            sorted.len(),
            (min.floor() as i32) - 1,
            (max.ceil() as i32) + 1,
            values
        );
        with_table.replace(&new_table, &format!("{}\n\n{}", chart, new_table))
    } else {
        with_table
    };

    let final_section = if !sorted.is_empty() {
        let summaries = summarize_rows(
            &sorted
                .iter()
                .map(|(date, (avg, low))| ParsedWeight {
                    week_ending: *date,
                    avg_weight: *avg,
                    low_weight: *low,
                })
                .collect::<Vec<_>>(),
        );

        let mut summary_lines = Vec::new();
        if let Some(total) = summaries.total_loss {
            summary_lines.push(format!("**Total Loss _(based on avg)_:** {:.1} lbs", total));
        }
        if let Some(weekly) = summaries.avg_weekly_loss {
            summary_lines.push(format!("**Avg Weekly Loss:** {:.2} lbs/week", weekly));
        }
        replace_or_append_summary(&with_chart, &summary_lines.join("\n"))
    } else {
        with_chart
    };

    Ok(SectionUpdate {
        mermaid_blocks_removed,
        section_text: final_section,
    })
}

fn find_weight_section(content: &str) -> Result<SectionMatch> {
    let heading_re = Regex::new(
        r"(?im)^(#{2,6}\s+[^\n]*\bweight\s+(?:logs?|log)(?:\s*(?:and|&)\s*dexa)?[^\n]*\s*$)",
    )
    .context("invalid heading regex")?;
    let matches: Vec<_> = heading_re.find_iter(content).collect();
    let Some(first_match) = matches.first() else {
        bail!("could not find Weight Log/Weight Logs section");
    };
    let first_idx = matches
        .iter()
        .position(|item| item.start() == first_match.start())
        .context("failed to locate first section match")?;
    let end = matches
        .get(first_idx + 1)
        .map(|m| m.start())
        .unwrap_or_else(|| content.len());

    Ok(SectionMatch {
        start: first_match.start(),
        end,
        text: content[first_match.start()..end].to_string(),
    })
}

#[derive(Debug)]
struct ParsedRow {
    avg_weight: f64,
    low_weight: f64,
}

fn parse_weekly_table(section: &str) -> BTreeMap<NaiveDate, ParsedRow> {
    let table_re = Regex::new(r"(?m)^\|\s*\*\*Week Ending\*\*.*\n\|.*\n(?:\|.*\n)+").unwrap();
    let mut map = BTreeMap::new();

    let Some(table_match) = table_re.find(section) else {
        return map;
    };
    let table = table_match.as_str();
    let mut lines = table.lines();
    let _ = lines.next();
    let _ = lines.next();

    for line in lines {
        let cols: Vec<&str> = line
            .split('|')
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
            .collect();
        if cols.len() < 5 {
            continue;
        }

        let date = match NaiveDate::parse_from_str(cols[0], "%Y-%m-%d") {
            Ok(date) => date,
            Err(_) => continue,
        };
        let avg = parse_weight_cell(cols[1]);
        let low = parse_weight_cell(cols[4]);
        if let (Some(avg), Some(low)) = (avg, low) {
            map.insert(
                date,
                ParsedRow {
                    avg_weight: avg,
                    low_weight: low,
                },
            );
        }
    }

    map
}

fn parse_weight_cell(cell: &str) -> Option<f64> {
    let mut cleaned = cell.replace("lbs", "").replace("*", "").trim().to_string();
    cleaned = cleaned.replace(',', "").trim().to_string();
    cleaned.parse::<f64>().ok().map(round_to_one_dp)
}

fn replace_or_append_summary(section_text: &str, summary_block: &str) -> String {
    let mut lines: Vec<&str> = section_text.lines().collect();
    let mut summary_start: Option<usize> = None;
    let mut summary_end: Option<usize> = None;

    for (idx, line) in lines.iter().enumerate() {
        if line.trim_start().starts_with("**Total Loss") {
            summary_start = Some(idx);
            if let Some(next) = lines.get(idx + 1) {
                if next.trim_start().starts_with("**Avg Weekly Loss:**") {
                    summary_end = Some(idx + 2);
                } else {
                    summary_end = Some(idx + 1);
                }
            } else {
                summary_end = Some(idx + 1);
            }
            break;
        }
    }

    match (summary_start, summary_end) {
        (Some(start), Some(end)) => {
            lines.splice(start..end, summary_block.lines());
            lines.join("\n")
        }
        _ => {
            if section_text.ends_with('\n') {
                format!("{section_text}{summary_block}")
            } else {
                format!("{section_text}\n\n{summary_block}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_and_build_weekly_rows() {
        let rows = vec![
            WeightOperation {
                entry_timestamp: Some("2026-01-05T10:00:00.000Z".to_string()),
                weight: Some(2000.0),
                entry_value: None,
                value: None,
                extra: HashMap::new(),
            },
            WeightOperation {
                entry_timestamp: Some("2026-01-08T10:00:00.000Z".to_string()),
                weight: Some(1990.0),
                entry_value: None,
                value: None,
                extra: HashMap::new(),
            },
        ];

        let parsed = build_weekly_rows(&rows).expect("rows should parse");
        assert_eq!(parsed.len(), 1);
        assert_eq!(
            parsed[0].week_ending,
            NaiveDate::from_ymd_opt(2026, 1, 11).unwrap()
        );
        assert!((parsed[0].avg_weight - 199.5).abs() < 0.01);
        assert!((parsed[0].low_weight - 199.0).abs() < 0.01);
    }

    #[test]
    fn test_parse_weekly_rows_range_filter() {
        let rows = vec![
            ParsedWeight {
                week_ending: NaiveDate::from_ymd_opt(2026, 1, 3).unwrap(),
                avg_weight: 200.0,
                low_weight: 198.0,
            },
            ParsedWeight {
                week_ending: NaiveDate::from_ymd_opt(2026, 1, 10).unwrap(),
                avg_weight: 199.5,
                low_weight: 198.5,
            },
        ];
        let filtered = filter_rows_after(&rows, Some(NaiveDate::from_ymd_opt(2026, 1, 5).unwrap()));
        assert_eq!(filtered.len(), 1);
        assert_eq!(
            filtered[0].week_ending,
            NaiveDate::from_ymd_opt(2026, 1, 10).unwrap()
        );
    }

    #[test]
    fn test_weekly_summary() {
        let rows = vec![
            ParsedWeight {
                week_ending: NaiveDate::from_ymd_opt(2026, 1, 3).unwrap(),
                avg_weight: 200.0,
                low_weight: 198.0,
            },
            ParsedWeight {
                week_ending: NaiveDate::from_ymd_opt(2026, 1, 10).unwrap(),
                avg_weight: 198.0,
                low_weight: 197.0,
            },
            ParsedWeight {
                week_ending: NaiveDate::from_ymd_opt(2026, 1, 17).unwrap(),
                avg_weight: 197.0,
                low_weight: 196.0,
            },
        ];

        let summary = summarize_rows(&rows);
        assert_eq!(summary.first_week.as_deref(), Some("2026-01-03"));
        assert_eq!(summary.last_week.as_deref(), Some("2026-01-17"));
        assert_eq!(summary.total_loss, Some(3.0));
        assert!((summary.avg_weekly_loss.unwrap() - 1.5).abs() < 0.0001);
    }

    #[test]
    fn test_weight_section_matches_common_variants() {
        let text = "## Something\n### Weight Logs\nrow\n## Other";
        let section = find_weight_section(text).expect("section exists");
        assert!(section.text.contains("Weight Logs"));
        let text2 = "### Weight Log and DEXA\nrow\n### End";
        let section2 = find_weight_section(text2).expect("section exists");
        assert!(section2.text.contains("Weight Log and DEXA"));
    }

    #[test]
    fn test_duplicate_mermaid_is_normalized() {
        let section = "### Weight Logs\n```mermaid\nchart 1\n```\n\n```mermaid\nchart 2\n```\n| **Week Ending** | **Avg Weight** | **Diff** |     | ***Low Weight*** |\n| --------------- | -------------- | -------- | --- | ---------------- |\n| 2026-01-10 | 200.0 lbs | - |     | *198.0 lbs* |\n";
        let parsed = vec![ParsedWeight {
            week_ending: NaiveDate::from_ymd_opt(2026, 1, 10).unwrap(),
            avg_weight: 200.0,
            low_weight: 198.0,
        }];
        let updated = build_updated_section(section, &parsed, None).expect("should update");
        let chart_count = updated.section_text.matches("xychart-beta").count();
        assert_eq!(chart_count, 1);
        assert_eq!(updated.mermaid_blocks_removed, 2);
        assert!(updated.section_text.contains("Average Weight by Week"));
    }
}
