use std::path::{Path, PathBuf};
use serde_json;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Ok,
    Warning,
    Error,
}

#[derive(Debug)]
pub struct Check {
    pub name: &'static str,
    pub status: Status,
    pub detail: String,
    /// Actionable recommendation when status is Warning or Error.
    pub fix: Option<String>,
}

impl Check {
    fn ok(name: &'static str, detail: impl Into<String>) -> Self {
        Self { name, status: Status::Ok, detail: detail.into(), fix: None }
    }
    fn warn(name: &'static str, detail: impl Into<String>, fix: impl Into<String>) -> Self {
        Self { name, status: Status::Warning, detail: detail.into(), fix: Some(fix.into()) }
    }
    fn err(name: &'static str, detail: impl Into<String>, fix: impl Into<String>) -> Self {
        Self { name, status: Status::Error, detail: detail.into(), fix: Some(fix.into()) }
    }
}

// ── Public entry points ───────────────────────────────────────────────────────

/// Run all system-level checks for the current platform.
pub fn system_checks() -> Vec<Check> {
    let mut checks = Vec::new();

    #[cfg(target_os = "windows")]
    checks.extend(windows_checks());

    #[cfg(target_os = "linux")]
    checks.extend(linux_checks());

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    checks.push(Check::warn(
        "Platform",
        format!("Running on {}, only Windows and Linux are tested", std::env::consts::OS),
        "Use Windows (native) or Linux (via Wine) for best compatibility",
    ));

    checks
}

/// Checks for the extraction tool required by a given archive format.
/// Returns an empty vec for ZIP (handled in pure Rust).
pub fn extractor_checks(format: &str) -> Vec<Check> {
    match format {
        "RAR" => vec![check_tool(
            "unrar",
            "unrar",
            "Required to extract RAR archives (G10 client)",
            "Install unrar:\n  \
             Fedora: sudo dnf install unrar\n  \
             Ubuntu: sudo apt install unrar\n  \
             Arch:   sudo pacman -S unrar",
        )],
        "7z" => vec![check_tool(
            "7z / p7zip",
            "7z",
            "Required to extract 7z archives (Wii U client)",
            "Install p7zip:\n  \
             Fedora: sudo dnf install p7zip\n  \
             Ubuntu: sudo apt install p7zip-full\n  \
             Arch:   sudo pacman -S p7zip",
        )],
        _ => vec![], // ZIP handled by the zip crate
    }
}

fn check_tool(name: &'static str, bin: &str, detail: &str, fix: &str) -> Check {
    match std::process::Command::new(bin).arg("--version").output()
        .or_else(|_| std::process::Command::new(bin).output())
    {
        Ok(_) => Check::ok(name, detail),
        Err(_) => Check::err(name, format!("{bin} not found in PATH"), fix),
    }
}

/// Run game-directory checks against an existing install path.
pub fn game_dir_checks(path: &Path) -> Vec<Check> {
    let mut checks = Vec::new();

    if !path.is_dir() {
        checks.push(Check::err(
            "Game directory",
            format!("'{}' is not a directory", path.display()),
            "Pass the path to an extracted game folder",
        ));
        return checks;
    }

    // Core executable
    let exe = path.join("mhf.exe");
    if exe.exists() {
        checks.push(Check::ok("mhf.exe", "found"));
    } else {
        checks.push(Check::err(
            "mhf.exe",
            "not found",
            "If deleted by antivirus, restore from: https://sek.ai/etc/mhf/mhf.7z",
        ));
    }

    // Custom launcher
    let iel = path.join("mhf-iel-cli.exe");
    if iel.exists() {
        checks.push(Check::ok("mhf-iel (launcher)", "found"));
    } else {
        checks.push(Check::warn(
            "mhf-iel (launcher)",
            "not found — game will require Internet Explorer to launch",
            "Download mhf-iel-cli.exe from the mhf-iel releases and place it in the game folder",
        ));
    }

    // D3D9 in game folder (Wine/Linux compat)
    #[cfg(target_os = "linux")]
    {
        let d3d9 = path.join("d3d9.dll");
        if d3d9.exists() {
            checks.push(Check::ok(
                "d3d9.dll (game folder)",
                "present — should be the DXVK/Wine-compatible version",
            ));
        } else {
            checks.push(Check::warn(
                "d3d9.dll (game folder)",
                "not found in game folder",
                "Copy the DXVK d3d9.dll into the game folder for Wine compatibility \
                 (see Wine/DXVK setup guide)",
            ));
        }
    }

    // GameGuard
    let gg = path.join("GameGuard.des");
    if gg.exists() {
        checks.push(Check::warn(
            "GameGuard",
            "GameGuard.des is present",
            "GameGuard is incompatible with modern Windows and Wine. \
             Apply the community no-GG patch or replace GameGuard.des with the patched version",
        ));
    } else {
        checks.push(Check::ok("GameGuard", "not present (patched or removed — OK)"));
    }

    // config.json (mhf-iel runtime config)
    let cfg = path.join("config.json");
    if cfg.exists() {
        let token_ok = std::fs::read_to_string(&cfg)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v.get("user_token").and_then(|t| t.as_str()).map(|t| t.len() == 16))
            .unwrap_or(false);
        if token_ok {
            checks.push(Check::ok("config.json", "found, session token present"));
        } else {
            checks.push(Check::warn(
                "config.json",
                "found but session token is missing or blank — not yet authenticated",
                "Use the MHF Launcher UI to log in and generate a valid token.",
            ));
        }
    } else {
        checks.push(Check::warn(
            "config.json",
            "not found — authenticate via the MHF Launcher UI before playing",
            "Open the launcher, select your game version, and click Authenticate.",
        ));
    }

    checks
}

// ── Windows ──────────────────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
fn windows_checks() -> Vec<Check> {
    vec![
        check_dx9_windows(),
        check_japanese_fonts_windows(),
    ]
}

#[cfg(target_os = "windows")]
fn check_dx9_windows() -> Check {
    // d3dx9_43.dll is the final file shipped by the DirectX End-User Runtime.
    // Its presence means the full DX9 runtime is installed.
    let sys32 = PathBuf::from(r"C:\Windows\System32");
    if sys32.join("d3dx9_43.dll").exists() {
        Check::ok("DirectX 9 runtime", "d3dx9_43.dll found in System32")
    } else {
        Check::err(
            "DirectX 9 runtime",
            "d3dx9_43.dll not found — DirectX End-User Runtime is not installed",
            "Download and run the DirectX End-User Runtime Web Installer:\
             \nhttps://www.microsoft.com/en-us/download/details.aspx?id=35",
        )
    }
}

#[cfg(target_os = "windows")]
fn check_japanese_fonts_windows() -> Check {
    let fonts = PathBuf::from(r"C:\Windows\Fonts");
    // msgothic.ttc ships with the Japanese language pack and is required by MHF
    // for displaying game text. Without it all text renders as boxes (tofu).
    let candidates = ["msgothic.ttc", "meiryo.ttc", "YuGothR.ttc", "yumin.ttf"];
    let found: Vec<&str> = candidates
        .iter()
        .copied()
        .filter(|f| fonts.join(f).exists())
        .collect();

    if found.contains(&"msgothic.ttc") {
        Check::ok("Japanese fonts", format!("MS Gothic found ({})", found.join(", ")))
    } else if !found.is_empty() {
        Check::warn(
            "Japanese fonts",
            format!("Some CJK fonts present ({}) but MS Gothic (msgothic.ttc) is missing", found.join(", ")),
            "Install the Japanese language pack:\
             \nSettings → Time & Language → Language & region → Add a language → 日本語\
             \nThen install the optional font package for that language",
        )
    } else {
        Check::err(
            "Japanese fonts",
            "No Japanese fonts found — game text will render as boxes",
            "Install the Japanese language pack:\
             \nSettings → Time & Language → Language & region → Add a language → 日本語\
             \nOr download MS Gothic directly and place it in C:\\Windows\\Fonts\\",
        )
    }
}

// ── Linux (Wine) ─────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
fn linux_checks() -> Vec<Check> {
    vec![
        check_wine(),
        check_dxvk(),
        check_japanese_fonts_linux(),
    ]
}

#[cfg(target_os = "linux")]
fn check_wine() -> Check {
    match std::process::Command::new("wine").arg("--version").output() {
        Ok(out) if out.status.success() => {
            let ver = String::from_utf8_lossy(&out.stdout).trim().to_string();
            Check::ok("Wine", format!("{} — recommended: Wine 9.0+", ver))
        }
        _ => Check::err(
            "Wine",
            "wine not found in PATH",
            "Install Wine 9.0 or later:\
             \n  Fedora:  sudo dnf install wine\
             \n  Ubuntu:  sudo apt install wine\
             \n  Arch:    sudo pacman -S wine\
             \nThen install DXVK 2.x into your Wine prefix for DX9 support",
        ),
    }
}

#[cfg(target_os = "linux")]
fn check_dxvk() -> Check {
    // DXVK installs d3d9.dll.so into the Wine prefix system32.
    // Check the default prefix; users with custom WINEPREFIX should set it.
    let prefix = std::env::var("WINEPREFIX")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_default();
            PathBuf::from(home).join(".wine")
        });

    let sys32_dll = prefix.join("drive_c/windows/system32/d3d9.dll");

    if !prefix.exists() {
        return Check::warn(
            "DXVK / Wine prefix",
            format!("Wine prefix not found at '{}'", prefix.display()),
            "Run 'wineboot' to initialise your Wine prefix, then install DXVK 2.x",
        );
    }

    if sys32_dll.exists() {
        // If the dll is a symlink to a .so it's almost certainly DXVK or WineD3D.
        let source = if sys32_dll.is_symlink() { "symlink to .so" } else { "file" };
        Check::ok(
            "DXVK / d3d9",
            format!("d3d9.dll present in Wine prefix ({})", source),
        )
    } else {
        Check::warn(
            "DXVK / d3d9",
            "d3d9.dll not found in Wine prefix — DX9 may not work",
            "Install DXVK 2.x into your Wine prefix:\
             \n  Using winetricks:  winetricks dxvk\
             \n  Manually:          https://github.com/doitsujin/dxvk/releases\
             \nTested with DXVK 2.7.1 + Wine 9.0 (see mhf-iel README)",
        )
    }
}

#[cfg(target_os = "linux")]
fn check_japanese_fonts_linux() -> Check {
    // fc-list :lang=ja returns one line per Japanese font face; empty = none installed.
    match std::process::Command::new("fc-list").args([":lang=ja"]).output() {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let count = stdout.lines().count();
            if count > 0 {
                Check::ok(
                    "Japanese fonts",
                    format!("{} Japanese font face(s) available via fontconfig", count),
                )
            } else {
                Check::err(
                    "Japanese fonts",
                    "No Japanese fonts found — game text will render as boxes in Wine",
                    "Install CJK fonts:\
                     \n  Fedora:  sudo dnf install google-noto-sans-cjk-fonts\
                     \n  Ubuntu:  sudo apt install fonts-noto-cjk\
                     \n  Arch:    sudo pacman -S noto-fonts-cjk\
                     \nThen run 'fc-cache -fv' to rebuild the font cache",
                )
            }
        }
        Err(_) => Check::warn(
            "Japanese fonts",
            "fc-list not available — cannot check font status",
            "Install fontconfig (fc-list) and CJK fonts for your distro",
        ),
    }
}
