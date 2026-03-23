use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

// ── Server API types ──────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LoginResponse {
    #[serde(rename = "currentTs")]
    pub current_ts: u32,
    #[serde(rename = "expiryTs")]
    pub expiry_ts: u32,
    #[serde(rename = "entranceCount")]
    pub entrance_count: u32,
    pub notices: Vec<Notice>,
    pub user: User,
    pub characters: Vec<Character>,
    #[serde(rename = "mezFes")]
    pub mez_fes: MezFes,
    #[serde(rename = "patchServer")]
    pub patch_server: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct User {
    #[serde(rename = "tokenId")]
    pub token_id: u32,
    pub token: String,
    pub rights: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Character {
    pub id: u32,
    pub name: String,
    #[serde(rename = "isFemale")]
    pub is_female: bool,
    pub weapon: u32,
    pub hr: u32,
    pub gr: u32,
    #[serde(rename = "lastLogin")]
    pub last_login: i32,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Notice {
    pub flags: u16,
    pub data: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MezFes {
    pub id: u32,
    pub start: u32,
    pub end: u32,
    #[serde(rename = "soloTickets")]
    pub solo_tickets: u32,
    #[serde(rename = "groupTickets")]
    pub group_tickets: u32,
    #[serde(deserialize_with = "deserialize_stalls")]
    pub stalls: Vec<String>,
}

fn deserialize_stalls<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{Deserialize, Error};
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Array(arr) => Ok(arr
            .into_iter()
            .filter_map(|v| match v {
                serde_json::Value::String(s) => Some(s),
                serde_json::Value::Number(n) => {
                    Some(match n.as_u64().unwrap_or(0) as u32 {
                        3  => "Pachinko",
                        4  => "Nyanrendo",
                        5  => "DokkanBattleCats",
                        6  => "VolpakkunTogether",
                        7  => "PointStall",
                        8  => "HoneyPanic",
                        9  => "GoocooScoop",
                        10 => "TokotokoPartnya",
                        _  => "StallMap",
                    }.to_string())
                }
                _ => None,
            })
            .collect()),
        _ => Err(D::Error::custom("expected array for stalls")),
    }
}

#[derive(Debug, Serialize)]
struct CreateCharRequest {
    token: String,
}

// ── config.json format (matches mhf-iel MhfConfig exactly) ──────────────────

/// Written to `config.json` in the game folder.
/// Field names must match mhf-iel's MhfConfig serde output.
#[derive(Debug, Serialize)]
pub struct GameConfig {
    pub char_id: u32,
    pub char_name: String,
    pub char_gr: u32,
    pub char_hr: u32,
    pub char_ids: Vec<u32>,
    pub char_new: bool,
    pub user_token_id: u32,
    pub user_token: String,
    pub user_name: String,
    pub user_password: String,
    pub user_rights: u32,
    pub server_host: String,
    pub server_port: u32,
    pub entrance_count: u32,
    pub current_ts: u32,
    pub expiry_ts: u32,
    pub notices: Vec<ConfigNotice>,
    pub mez_event_id: u32,
    pub mez_start: u32,
    pub mez_end: u32,
    pub mez_solo_tickets: u32,
    pub mez_group_tickets: u32,
    pub mez_stalls: Vec<String>, // e.g. "TokotokoPartnya"
    pub version: String,         // "ZZ" | "F5"
    pub mhf_folder: Option<String>,
    pub mhf_flags: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
pub struct ConfigNotice {
    pub flags: u16,
    pub data: String,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Authenticate with the Erupe server (login or register).
/// Returns the full login response on success so the caller can handle
/// character selection before writing config.json.
pub fn authenticate(server: &str, action: &str, username: &str, password: &str) -> Result<LoginResponse> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("mhf-outpost/0.1")
        .build()?;
    let url = format!("{}/{}", server.trim_end_matches('/'), action);

    let resp = client
        .post(&url)
        .json(&LoginRequest { username: username.to_string(), password: password.to_string() })
        .send()
        .with_context(|| format!("failed to connect to {url}"))?;

    if !resp.status().is_success() {
        bail!(
            "server returned {}: {}",
            resp.status(),
            resp.text().unwrap_or_default()
        );
    }

    resp.json::<LoginResponse>().context("failed to parse server response")
}

/// Create a new character on the server and return it.
pub fn create_character(server: &str, token: &str) -> Result<Character> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("mhf-outpost/0.1")
        .build()?;
    let url = format!("{}/character/create", server.trim_end_matches('/'));

    let resp = client
        .post(&url)
        .json(&CreateCharRequest { token: token.to_string() })
        .send()
        .context("failed to create character")?;

    if !resp.status().is_success() {
        bail!(
            "character creation failed: {} - {}",
            resp.status(),
            resp.text().unwrap_or_default()
        );
    }

    resp.json::<Character>().context("failed to parse character response")
}

/// Build and write `config.json` into `game_dir`.
pub fn save_config(
    game_dir: &Path,
    server: &str,
    login: &LoginResponse,
    char_id: u32,
    char_data: &Character,
    version: &str,
) -> Result<()> {
    let url = reqwest::Url::parse(server).context("invalid server URL")?;
    let server_host = url.host_str().context("no host in server URL")?.to_string();

    let char_ids: Vec<u32> = login.characters.iter().map(|c| c.id).collect();

    let config = GameConfig {
        char_id,
        char_name: char_data.name.clone(),
        char_gr: char_data.gr,
        char_hr: char_data.hr,
        char_ids,
        char_new: false,
        user_token_id: login.user.token_id,
        user_token: login.user.token.clone(),
        user_name: String::new(),
        user_password: String::new(),
        user_rights: login.user.rights,
        server_host,
        server_port: 53310,
        entrance_count: login.entrance_count,
        current_ts: login.current_ts,
        expiry_ts: login.expiry_ts,
        notices: login.notices.iter().map(|n| ConfigNotice { flags: n.flags, data: n.data.clone() }).collect(),
        mez_event_id: login.mez_fes.id,
        mez_start: login.mez_fes.start,
        mez_end: login.mez_fes.end,
        mez_solo_tickets: login.mez_fes.solo_tickets,
        mez_group_tickets: login.mez_fes.group_tickets,
        mez_stalls: login.mez_fes.stalls.clone(),
        version: version.to_string(),
        mhf_folder: None,
        mhf_flags: None,
    };

    let json = serde_json::to_string_pretty(&config)?;
    std::fs::create_dir_all(game_dir)?;
    std::fs::write(game_dir.join("config.json"), json)
        .context("failed to write config.json")?;

    Ok(())
}
