use mhf_installer_core::{auth, check, download, launcher, manifest};
use serde::Serialize;
use std::path::PathBuf;
use tauri::Emitter;

// ── DTOs ─────────────────────────────────────────────────────────────────────

#[derive(Serialize, Clone)]
pub struct VersionDto {
    pub id: String,
    pub name: String,
    pub description: String,
    pub platform: String,
    pub has_archive: bool,
    pub archive_size_gb: Option<f64>,
    pub archive_format: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct CheckDto {
    pub name: String,
    pub status: String, // "ok" | "warning" | "error"
    pub detail: String,
    pub fix: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct DownloadProgressEvent {
    pub version: String,
    pub phase: String,  // "download" | "verify" | "extract" | "done" | "error"
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub message: Option<String>,
}

// ── Commands ──────────────────────────────────────────────────────────────────

#[tauri::command]
pub fn list_versions() -> Vec<VersionDto> {
    manifest::Manifest::all()
        .into_iter()
        .map(|m| VersionDto {
            id: m.version.id.clone(),
            name: m.version.name.clone(),
            description: m.version.description.clone(),
            platform: m.version.platform.clone(),
            has_archive: m.archive.is_some(),
            archive_size_gb: m.archive.as_ref().map(|a| a.size as f64 / 1_073_741_824.0),
            archive_format: m.archive.as_ref().map(|a| a.format.clone()),
        })
        .collect()
}

#[tauri::command]
pub fn get_version_info(version: String) -> Result<VersionDto, String> {
    let m = manifest::Manifest::load(&version).map_err(|e| e.to_string())?;
    Ok(VersionDto {
        id: m.version.id.clone(),
        name: m.version.name.clone(),
        description: m.version.description.clone(),
        platform: m.version.platform.clone(),
        has_archive: m.archive.is_some(),
        archive_size_gb: m.archive.as_ref().map(|a| a.size as f64 / 1_073_741_824.0),
        archive_format: m.archive.as_ref().map(|a| a.format.clone()),
    })
}

#[tauri::command]
pub fn run_checks(game_path: Option<String>) -> Vec<CheckDto> {
    let mut all = check::system_checks();
    if let Some(path) = game_path {
        all.extend(check::game_dir_checks(std::path::Path::new(&path)));
    }
    all.into_iter().map(|c| CheckDto {
        name: c.name.to_string(),
        status: match c.status {
            check::Status::Ok      => "ok",
            check::Status::Warning => "warning",
            check::Status::Error   => "error",
        }.to_string(),
        detail: c.detail,
        fix: c.fix,
    }).collect()
}

#[tauri::command]
pub async fn download_version(
    window: tauri::Window,
    version: String,
    dest: String,
) -> Result<(), String> {
    let manifest = manifest::Manifest::load(&version).map_err(|e| e.to_string())?;
    let dest_path = PathBuf::from(&dest);
    let version_for_event = version.clone();
    let archive_size = manifest.archive.as_ref().map(|a| a.size).unwrap_or(0);

    let _ = window.emit("download-progress", DownloadProgressEvent {
        version: version.clone(),
        phase: "download".to_string(),
        bytes_done: 0,
        bytes_total: archive_size,
        message: Some("Downloading…".to_string()),
    });

    let result = tauri::async_runtime::spawn_blocking(move || {
        download::run(&manifest, download::DownloadOptions {
            dest: dest_path,
            archive_path: None,
            yes: true,
            keep_archive: false,
        })
    }).await.map_err(|e| e.to_string())?;

    match result {
        Ok(()) => {
            let _ = window.emit("download-progress", DownloadProgressEvent {
                version: version_for_event,
                phase: "done".to_string(),
                bytes_done: archive_size,
                bytes_total: archive_size,
                message: Some("Installation complete!".to_string()),
            });
            Ok(())
        }
        Err(e) => {
            let _ = window.emit("download-progress", DownloadProgressEvent {
                version: version_for_event,
                phase: "error".to_string(),
                bytes_done: 0,
                bytes_total: archive_size,
                message: Some(e.to_string()),
            });
            Err(e.to_string())
        }
    }
}

/// Launch the game. Runs in a background thread so the UI stays responsive.
/// Note: this blocks until the game process exits.
#[tauri::command]
pub async fn launch_game(path: String, auth: bool) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        launcher::launch(std::path::Path::new(&path), auth)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn fetch_launcher(path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        launcher::fetch_launcher(std::path::Path::new(&path))
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn av_exclude(path: String) -> Result<(), String> {
    launcher::av_exclude(std::path::Path::new(&path))
        .map_err(|e| e.to_string())
}

// ── Auth commands ─────────────────────────────────────────────────────────────

#[derive(Serialize, Clone)]
pub struct CharacterDto {
    pub id: u32,
    pub name: String,
    pub hr: u32,
    pub gr: u32,
    pub is_female: bool,
}

/// Opaque session token passed back to the frontend after login so it can call
/// select_character without the server re-authenticating.
#[derive(Serialize, Clone)]
pub struct AuthSession {
    pub characters: Vec<CharacterDto>,
    /// Serialised LoginResponse — held by the frontend and passed to select_character.
    pub session_json: String,
}

/// Authenticate against an Erupe server. Returns the list of characters plus an
/// opaque session blob. If exactly one character exists the caller can immediately
/// call select_character; otherwise it should prompt the user to pick one.
#[tauri::command]
pub async fn authenticate(
    server: String,
    username: String,
    password: String,
    action: String, // "login" | "register"
) -> Result<AuthSession, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let login = auth::authenticate(&server, &action, &username, &password)
            .map_err(|e| e.to_string())?;

        let characters = login.characters.iter().map(|c| CharacterDto {
            id: c.id, name: c.name.clone(), hr: c.hr, gr: c.gr, is_female: c.is_female,
        }).collect();

        let session_json = serde_json::to_string(&login)
            .map_err(|e| e.to_string())?;

        Ok(AuthSession { characters, session_json })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Finalise authentication by selecting a character and writing config.json.
/// `session_json` is the blob returned by `authenticate`.
/// Pass `char_id = 0` to create a new character.
#[tauri::command]
pub async fn select_character(
    game_path: String,
    server: String,
    session_json: String,
    char_id: u32,
    version: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let login: auth::LoginResponse = serde_json::from_str(&session_json)
            .map_err(|e| format!("invalid session: {e}"))?;

        let (id, char_data) = if char_id == 0 {
            // Create a new character
            let new_char = auth::create_character(&server, &login.user.token)
                .map_err(|e| e.to_string())?;
            let id = new_char.id;
            (id, new_char)
        } else {
            let c = login.characters.iter().find(|c| c.id == char_id)
                .ok_or_else(|| format!("character {char_id} not found"))?
                .clone();
            (c.id, c)
        };

        auth::save_config(
            std::path::Path::new(&game_path),
            &server,
            &login,
            id,
            &char_data,
            &version,
        ).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
