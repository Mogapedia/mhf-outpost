use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

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
    /// Plain-text notice strings shown in the launcher.
    pub notices: Vec<String>,
    pub user: User,
    pub characters: Vec<Character>,
    pub courses: Vec<CourseInfo>,
    #[serde(rename = "mezFes")]
    pub mez_fes: Option<MezFes>,
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
    /// True if the character has not logged in for 90+ days.
    pub returning: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CourseInfo {
    pub id: u16,
    pub name: String,
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
    /// Stall type IDs (3 = Pachinko, 4 = Nyanrendo, …).
    pub stalls: Vec<u32>,
}

// ── config.json format (matches mhf-iel MhfConfig exactly) ──────────────────

/// Mirrors mhf-iel's `Notice { flags: u16, data: String }` — the Erupe API only
/// gives us plain strings, so `flags` is always written as 0.
#[derive(Debug, Serialize)]
pub struct NoticeOut {
    pub flags: u16,
    pub data: String,
}

/// mhf-iel's `MezFesStall` is a fieldless enum with explicit u32 discriminants;
/// serde's default derive serializes it as the variant *name*, not the number.
/// Maps Erupe's raw stall IDs to those variant names, dropping unknown IDs.
fn mez_fes_stall_name(id: u32) -> Option<&'static str> {
    Some(match id {
        2 => "TokotokoPartnya",
        3 => "Pachinko",
        4 => "VolpakkunTogether",
        5 => "GoocooScoop",
        6 => "Nyanrendo",
        7 => "HoneyPanic",
        8 => "DokkanBattleCats",
        9 => "PointStall",
        10 => "StallMap",
        _ => return None,
    })
}

/// Written to `config.json` in the game folder.
/// Field names and shapes must match mhf-iel's MhfConfig serde output exactly.
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
    pub notices: Vec<NoticeOut>,
    pub mez_event_id: u32,
    pub mez_start: u32,
    pub mez_end: u32,
    pub mez_solo_tickets: u32,
    pub mez_group_tickets: u32,
    pub mez_stalls: Vec<String>,
    pub version: String, // "ZZ" | "F5"
    pub mhf_folder: Option<String>,
    pub mhf_flags: Option<Vec<String>>,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Authenticate with the Erupe server (login or register) via the v2 API.
/// Returns the full login response on success so the caller can handle
/// character selection before writing config.json.
pub fn authenticate(
    server: &str,
    action: &str,
    username: &str,
    password: &str,
) -> Result<LoginResponse> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("mhf-outpost/0.1")
        .timeout(REQUEST_TIMEOUT)
        .build()?;
    let url = format!("{}/v2/{}", server.trim_end_matches('/'), action);

    let resp = client
        .post(&url)
        .json(&LoginRequest {
            username: username.to_string(),
            password: password.to_string(),
        })
        .send()
        .with_context(|| format!("failed to connect to {url}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        // Erupe answers JSON ({"error": ..., "message": ...}); anything else
        // means the address is not the API (a web server's 404 page, say).
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
            let msg = v["message"].as_str().or(v["error"].as_str()).unwrap_or("");
            bail!("{status}: {msg}");
        }
        bail!(
            "{url} answered {status} — not an Erupe API. The address should be \
             the API origin, usually http://<host>:8080"
        );
    }

    resp.json::<LoginResponse>()
        .context("failed to parse server response")
}

/// Create a new character on the server and return it.
/// Uses `POST /v2/characters` with a Bearer token.
pub fn create_character(server: &str, token: &str) -> Result<Character> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("mhf-outpost/0.1")
        .timeout(REQUEST_TIMEOUT)
        .build()?;
    let url = format!("{}/v2/characters", server.trim_end_matches('/'));

    let resp = client
        .post(&url)
        .bearer_auth(token)
        .send()
        .context("failed to create character")?;

    if !resp.status().is_success() {
        bail!(
            "character creation failed: {} - {}",
            resp.status(),
            resp.text().unwrap_or_default()
        );
    }

    resp.json::<Character>()
        .context("failed to parse character response")
}

/// Build and write `config.json` into `game_dir`.
///
/// `is_new` must be true when `char_data` came from `create_character()`: it
/// becomes `char_new`, which makes the game run its own character creation.
/// Otherwise the game asks for savedata the server has never stored, and
/// Erupe drops the connection.
pub fn save_config(
    game_dir: &Path,
    server: &str,
    login: &LoginResponse,
    char_id: u32,
    char_data: &Character,
    is_new: bool,
    version: &str,
) -> Result<()> {
    let url = reqwest::Url::parse(server).context("invalid server URL")?;
    let server_host = url.host_str().context("no host in server URL")?.to_string();

    let char_ids: Vec<u32> = login.characters.iter().map(|c| c.id).collect();
    let mez = login.mez_fes.as_ref();

    let config = GameConfig {
        char_id,
        char_name: char_data.name.clone(),
        char_gr: char_data.gr,
        char_hr: char_data.hr,
        char_ids,
        char_new: is_new,
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
        notices: login
            .notices
            .iter()
            .map(|data| NoticeOut {
                flags: 0,
                data: data.clone(),
            })
            .collect(),
        mez_event_id: mez.map_or(0, |m| m.id),
        mez_start: mez.map_or(0, |m| m.start),
        mez_end: mez.map_or(0, |m| m.end),
        mez_solo_tickets: mez.map_or(0, |m| m.solo_tickets),
        mez_group_tickets: mez.map_or(0, |m| m.group_tickets),
        mez_stalls: mez.map_or_else(Vec::new, |m| {
            m.stalls
                .iter()
                .filter_map(|id| mez_fes_stall_name(*id))
                .map(String::from)
                .collect()
        }),
        version: version.to_string(),
        mhf_folder: None,
        mhf_flags: None,
    };

    let json = serde_json::to_string_pretty(&config)?;
    std::fs::create_dir_all(game_dir)?;
    std::fs::write(game_dir.join("config.json"), json).context("failed to write config.json")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn login() -> LoginResponse {
        LoginResponse {
            current_ts: 0,
            expiry_ts: 0,
            entrance_count: 1,
            notices: Vec::new(),
            user: User {
                token_id: 1,
                token: "tok".into(),
                rights: 0,
            },
            characters: Vec::new(),
            courses: Vec::new(),
            mez_fes: None,
            patch_server: String::new(),
        }
    }

    fn character() -> Character {
        Character {
            id: 400,
            name: String::new(),
            is_female: false,
            weapon: 0,
            hr: 0,
            gr: 0,
            last_login: 0,
            returning: false,
        }
    }

    fn written_char_new(is_new: bool) -> bool {
        let dir = std::env::temp_dir().join(format!(
            "mhf-outpost-char-new-{}-{is_new}",
            std::process::id()
        ));
        save_config(
            &dir,
            "http://localhost:8080",
            &login(),
            400,
            &character(),
            is_new,
            "ZZ",
        )
        .unwrap();
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("config.json")).unwrap())
                .unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        json["char_new"].as_bool().unwrap()
    }

    #[test]
    fn save_config_marks_created_character_as_new() {
        assert!(written_char_new(true));
    }

    #[test]
    fn save_config_keeps_existing_character_not_new() {
        assert!(!written_char_new(false));
    }
}
