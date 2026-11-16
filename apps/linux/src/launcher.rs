//! Freedesktop `.desktop` and executable resolution, Snap confinement check,
//! and isolated Chromium/Electron profile configuration.

use crate::ports::{LaunchRequest, PortError};
use agent_screen_model::model::DisplayId;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct ResolvedCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub is_chromium_like: bool,
    pub is_snap: bool,
    pub isolated_xdg_config: Option<PathBuf>,
}

pub fn resolve_launch_command(
    display_id: DisplayId,
    req: &LaunchRequest,
) -> Result<ResolvedCommand, PortError> {
    let target = validate_open_target(req.open.as_deref())?;
    let (program, mut base_args) = resolve_program_and_args(&req.app)?;

    let is_snap = program.starts_with("/snap/bin")
        || std::fs::read_link(&program)
            .ok()
            .is_some_and(|t| t == Path::new("/usr/bin/snap") || t.starts_with("/snap/"));

    let is_chromium_like = detect_chromium_or_electron(&program);
    let mut args = Vec::new();
    args.append(&mut base_args);
    let mut isolated_xdg_config = None;

    if is_chromium_like {
        let user_args = &req.arguments;
        let has_flag = |prefix: &str| user_args.iter().any(|a| a.starts_with(prefix));
        let force_x11 = req
            .environment
            .get("GDK_BACKEND")
            .is_some_and(|v| v == "x11")
            || req
                .environment
                .get("OZONE_PLATFORM")
                .is_some_and(|v| v == "x11");
        if !has_flag("--ozone-platform") && !force_x11 {
            args.push("--ozone-platform=wayland".to_string());
        }
        if !has_flag("--force-renderer-accessibility") {
            args.push("--force-renderer-accessibility".to_string());
        }
        if !has_flag("--password-store") {
            args.push("--password-store=basic".to_string());
        }
        if !has_flag("--no-first-run") {
            args.push("--no-first-run".to_string());
        }
        if !has_flag("--no-default-browser-check") {
            args.push("--no-default-browser-check".to_string());
        }
        let profile = default_browser_profile_dir(&program, display_id)?;
        let xdg_cfg = profile.join("xdg-config");
        let _ = std::fs::create_dir_all(&xdg_cfg);
        let _ = std::fs::set_permissions(&xdg_cfg, std::fs::Permissions::from_mode(0o700));
        isolated_xdg_config = Some(xdg_cfg);
        if !has_flag("--user-data-dir") {
            args.push(format!("--user-data-dir={}", profile.display()));
        }
    }

    args.extend(req.arguments.iter().cloned());
    if let Some(target) = target {
        args.push(target);
    }

    Ok(ResolvedCommand {
        program,
        args,
        is_chromium_like,
        is_snap,
        isolated_xdg_config,
    })
}

fn validate_open_target(open: Option<&str>) -> Result<Option<String>, PortError> {
    let Some(raw) = open.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if raw.contains("://") || raw.starts_with('/') {
        return Ok(Some(raw.to_string()));
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        let home = std::env::var("HOME").unwrap_or_default();
        return Ok(Some(format!("{home}/{rest}")));
    }
    Err(PortError::invalid(
        "open must be a URL (https://..., file://...) or an absolute path starting with / or ~/",
    ))
}

fn resolve_program_and_args(app: &str) -> Result<(PathBuf, Vec<String>), PortError> {
    let trimmed = app.trim();
    if trimmed.is_empty() {
        return Err(PortError::invalid("app is required"));
    }
    let as_path = Path::new(trimmed);
    if as_path.is_absolute() || trimmed.contains('/') {
        if as_path.is_file() {
            return Ok((as_path.to_path_buf(), Vec::new()));
        }
        return Err(PortError::not_found(format!(
            "executable {} does not exist",
            as_path.display()
        )));
    }

    if let Some(found) = find_in_path(trimmed) {
        return Ok((found, Vec::new()));
    }
    if let Some((prog, args)) = find_in_desktop_entries(trimmed) {
        return Ok((prog, args));
    }
    // Common friendly browser/editor aliases across Linux distros.
    let aliases: &[(&str, &[&str])] = &[
        (
            "chrome",
            &[
                "google-chrome-stable",
                "google-chrome",
                "chromium",
                "chromium-browser",
            ],
        ),
        (
            "chromium",
            &["chromium", "chromium-browser", "google-chrome-stable"],
        ),
        ("firefox", &["firefox", "firefox-esr"]),
    ];
    let lower = trimmed.to_ascii_lowercase();
    for (alias, candidates) in aliases {
        if lower == *alias {
            for c in *candidates {
                if let Some(found) = find_in_path(c) {
                    return Ok((found, Vec::new()));
                }
            }
        }
    }

    Err(PortError::not_found(format!(
        "could not find Linux application {trimmed:?} in PATH or XDG desktop entries"
    )))
}

fn find_in_path(binary: &str) -> Option<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let path_var = std::env::var("PATH").unwrap_or_default();
    let extra = [
        PathBuf::from(&home).join(".local/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
        PathBuf::from("/snap/bin"),
    ];
    std::env::split_paths(&path_var)
        .chain(extra)
        .map(|dir| dir.join(binary))
        .find(|p| is_executable(p))
}

fn is_executable(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|m| m.is_file() && (m.permissions().mode() & 0o111) != 0)
}

fn find_in_desktop_entries(query: &str) -> Option<(PathBuf, Vec<String>)> {
    let q = query.trim_end_matches(".desktop").to_ascii_lowercase();
    let home = std::env::var("HOME").unwrap_or_default();
    let mut dirs = vec![PathBuf::from(&home).join(".local/share/applications")];
    if let Ok(xdg) = std::env::var("XDG_DATA_DIRS") {
        dirs.extend(std::env::split_paths(&xdg).map(|d| d.join("applications")));
    } else {
        dirs.push(PathBuf::from("/usr/local/share/applications"));
        dirs.push(PathBuf::from("/usr/share/applications"));
    }

    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let mut in_entry = false;
            let mut name = String::new();
            let mut wm_class = String::new();
            let mut exec = String::new();
            for line in text.lines() {
                let line = line.trim();
                if line.starts_with('[') {
                    in_entry = line == "[Desktop Entry]";
                    continue;
                }
                if !in_entry {
                    continue;
                }
                if let Some(v) = line.strip_prefix("Name=")
                    && name.is_empty()
                {
                    name = v.trim().to_ascii_lowercase();
                } else if let Some(v) = line.strip_prefix("StartupWMClass=") {
                    wm_class = v.trim().to_ascii_lowercase();
                } else if let Some(v) = line.strip_prefix("Exec=")
                    && exec.is_empty()
                {
                    exec = v.trim().to_string();
                }
            }
            if (stem == q || name == q || wm_class == q)
                && let Some(parsed) = parse_desktop_exec(&exec)
            {
                return Some(parsed);
            }
        }
    }
    None
}

fn parse_desktop_exec(exec: &str) -> Option<(PathBuf, Vec<String>)> {
    let tokens: Vec<String> = exec
        .split_whitespace()
        .map(|t| t.trim_matches('"').to_string())
        .filter(|t| !t.starts_with('%'))
        .collect();
    let (first, rest) = tokens.split_first()?;
    let prog = if Path::new(first).is_absolute() {
        PathBuf::from(first)
    } else {
        find_in_path(first)?
    };
    Some((prog, rest.to_vec()))
}

fn detect_chromium_or_electron(program: &Path) -> bool {
    let name = program
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if [
        "chromium",
        "chromium-browser",
        "google-chrome",
        "google-chrome-stable",
        "brave",
        "brave-browser",
        "microsoft-edge",
        "electron",
    ]
    .iter()
    .any(|k| name.starts_with(k))
    {
        return true;
    }
    program
        .parent()
        .is_some_and(|dir| dir.join("chrome_100_percent.pak").exists())
}

fn default_browser_profile_dir(
    program: &Path,
    display_id: DisplayId,
) -> Result<PathBuf, PortError> {
    let app_slug: String = program
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("browser")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let state_home = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_default();
            PathBuf::from(home).join(".local/state")
        });
    let instance_suffix = agent_screen_mcp::endpoint::instance()
        .map(|i| format!("-{i}"))
        .unwrap_or_default();
    let dir = state_home
        .join(format!("agent-screen{instance_suffix}"))
        .join("profiles")
        .join(format!("{app_slug}-d{display_id}"));
    std::fs::create_dir_all(&dir).map_err(|e| PortError::failed(e.to_string()))?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| PortError::failed(e.to_string()))?;
    Ok(dir)
}
