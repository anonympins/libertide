use std::path::{Path, PathBuf};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// Résout l'emplacement réel du fichier .env sur le disque
pub fn find_env_file() -> PathBuf {
    let manifest_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(".env");
    if manifest_path.is_file() {
        return manifest_path;
    }
    if let Ok(exe) = std::env::current_exe() {
        let mut curr = exe.parent();
        for _ in 0..6 {
            if let Some(dir) = curr {
                let p = dir.join(".env");
                if p.is_file() {
                    return p;
                }
                curr = dir.parent();
            } else {
                break;
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        let mut curr = Some(cwd.as_path());
        for _ in 0..5 {
            if let Some(dir) = curr {
                let p = dir.join(".env");
                if p.is_file() {
                    return p;
                }
                curr = dir.parent();
            } else {
                break;
            }
        }
    }
    PathBuf::from(".env")
}

/// Lit une variable d'environnement définie dans un fichier texte (par exemple `.env`).
pub fn read_env_var_from_file(file_path: &str, key: &str) -> Option<String> {
    let resolved_path = if file_path == ".env" { find_env_file() } else { PathBuf::from(file_path) };
    let content = std::fs::read_to_string(&resolved_path).ok()?;
    let prefix = format!("{key}=");
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix(&prefix) {
            let val = rest.trim().trim_matches('"').trim_matches('\'');
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

/// Met à jour ou ajoute une variable d'environnement dans un fichier texte (ex: `.env`).
pub fn write_env_var_to_file(file_path: &str, key: &str, value: &str) -> std::io::Result<()> {
    let resolved_path = if file_path == ".env" { find_env_file() } else { PathBuf::from(file_path) };
    let prefix = format!("{key}=");
    let content = std::fs::read_to_string(&resolved_path).unwrap_or_default();
    let mut replaced = false;
    let mut new_lines = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('#') && trimmed.starts_with(&prefix) {
            new_lines.push(format!("{key}={value}"));
            replaced = true;
        } else {
            new_lines.push(line.to_string());
        }
    }

    if !replaced {
        if !content.is_empty() && !content.ends_with('\n') {
            new_lines.push(String::new());
        }
        new_lines.push(format!("{key}={value}"));
    }

    let mut out = new_lines.join("\n");
    if !out.ends_with('\n') {
        out.push('\n');
    }
    std::fs::write(&resolved_path, out)
}

#[cfg(windows)]
pub fn open_path_with_default_app(path: &str) -> Result<(), String> {
    let p = std::path::Path::new(path);
    let resolved = if p.exists() {
        p.to_path_buf()
    } else if let Some((found, _)) = find_file_in_workspace(path) {
        std::path::PathBuf::from(found)
    } else {
        std::path::PathBuf::from(path)
    };

    let target_str = resolved.to_string_lossy().to_string();
    let status = std::process::Command::new("cmd")
        .args(["/C", "start", "", &target_str])
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(|e| format!("Erreur lors de l'ouverture du fichier : {e}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("Code de retour non nul lors de l'ouverture de '{target_str}'"))
    }
}

#[cfg(not(windows))]
pub fn open_path_with_default_app(path: &str) -> Result<(), String> {
    let status = std::process::Command::new("xdg-open")
        .arg(path)
        .status()
        .map_err(|e| format!("Erreur lors de l'ouverture du fichier : {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("Code de retour non nul pour xdg-open '{path}'"))
    }
}

/// Crée un fichier texte temporaire contenant le texte fourni.
pub fn create_temp_note_file(content: &str) -> std::io::Result<PathBuf> {
    let temp_dir = std::env::temp_dir();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let temp_path = temp_dir.join(format!("note_{timestamp}.txt"));
    std::fs::write(&temp_path, content)?;
    Ok(temp_path)
}

/// Cherche un fichier cible dans le projet ou le répertoire courant (avec repli récursif).
pub fn find_file_in_workspace(target: &str) -> Option<(String, String)> {
    let t_trim = target.trim();
    if t_trim.is_empty() {
        return None;
    }

    let t_path = Path::new(t_trim);
    if t_path.is_file() {
        if let Ok(s) = std::fs::read_to_string(t_path) {
            let trim = s.trim();
            if !trim.is_empty() {
                return Some((t_path.display().to_string(), s));
            }
        }
    }

    let src_path = PathBuf::from("src").join(t_trim);
    if src_path.is_file() {
        if let Ok(s) = std::fs::read_to_string(&src_path) {
            let trim = s.trim();
            if !trim.is_empty() {
                return Some((src_path.display().to_string(), s));
            }
        }
    }

    let target_file_name = t_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(t_trim)
        .to_lowercase();

    let current_dir = std::env::current_dir().ok()?;
    let mut stack = vec![(current_dir, 0usize)];

    while let Some((dir, depth)) = stack.pop() {
        if depth > 5 {
            continue;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let dir_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                if dir_name.starts_with('.') || dir_name == "target" || dir_name == "node_modules" {
                    continue;
                }
                stack.push((path, depth + 1));
            } else if path.is_file() {
                let is_match = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.to_lowercase() == target_file_name)
                    .unwrap_or(false);

                if is_match {
                    if let Ok(s) = std::fs::read_to_string(&path) {
                        let trim = s.trim();
                        if !trim.is_empty() {
                            return Some((path.display().to_string(), s));
                        }
                    }
                }
            }
        }
    }

    None
}

#[cfg(windows)]
pub fn get_system_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Some(path_var) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path_var));
    }

    if let Ok(sysroot) = std::env::var("SystemRoot") {
        let p = PathBuf::from(&sysroot);
        dirs.push(p.join("System32"));
        dirs.push(p.clone());
        dirs.push(p.join("SysWOW64"));
    } else {
        dirs.push(PathBuf::from(r"C:\Windows\System32"));
        dirs.push(PathBuf::from(r"C:\Windows"));
    }

    if let Ok(localappdata) = std::env::var("LOCALAPPDATA") {
        dirs.push(PathBuf::from(localappdata).join(r"Microsoft\WindowsApps"));
    }

    dirs
}

#[cfg(windows)]
pub fn find_in_program_dirs(name: &str, extensions: &[String]) -> Option<PathBuf> {
    let mut base_dirs = Vec::new();
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        base_dirs.push(PathBuf::from(pf));
    }
    if let Some(pfx86) = std::env::var_os("ProgramFiles(x86)") {
        base_dirs.push(PathBuf::from(pfx86));
    }
    if let Some(pfw64) = std::env::var_os("ProgramW6432") {
        base_dirs.push(PathBuf::from(pfw64));
    }
    if let Ok(localappdata) = std::env::var("LOCALAPPDATA") {
        let p = PathBuf::from(localappdata);
        base_dirs.push(p.join("Programs"));
        base_dirs.push(p);
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        let p = PathBuf::from(appdata);
        base_dirs.push(p.join("Programs"));
        base_dirs.push(p);
    }

    let stem = Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(name)
        .to_lowercase();
    let no_spaces = stem.replace(' ', "").replace('-', "").replace('_', "");
    let mut clean_stems = vec![stem.clone()];
    if no_spaces != stem {
        clean_stems.push(no_spaces);
    }

    for base in &base_dirs {
        let entries = match std::fs::read_dir(base) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            for stem in &clean_stems {
                for ext in extensions {
                    let ext_clean = if ext.starts_with('.') { ext.clone() } else { format!(".{ext}") };
                    let candidate_file = path.join(format!("{stem}{ext_clean}"));
                    if candidate_file.is_file() {
                        return Some(candidate_file);
                    }
                }
            }
        }
    }
    None
}

#[cfg(windows)]
pub fn find_executable_in_path(name: &str) -> Option<PathBuf> {
    let name_trimmed = name.trim();
    if name_trimmed.is_empty() {
        return None;
    }

    let pathext = std::env::var_os("PATHEXT").unwrap_or_else(|| std::ffi::OsString::from(".EXE;.CMD;.BAT;.COM"));
    let mut extensions: Vec<String> = pathext
        .to_str()
        .unwrap_or(".EXE;.CMD;.BAT;.COM")
        .split(';')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();

    if !extensions.iter().any(|e| e.eq_ignore_ascii_case(".exe")) {
        extensions.push(".EXE".to_string());
    }

    let dirs = get_system_search_dirs();
    let p = Path::new(name_trimmed);
    if p.is_absolute() && p.exists() {
        return Some(p.to_path_buf());
    }
    let has_ext = p.extension().is_some();
    for dir in &dirs {
        let direct = dir.join(name_trimmed);
        if direct.exists() {
            return Some(direct);
        }
        if !has_ext {
            for ext in &extensions {
                let ext_clean = if ext.starts_with('.') { ext.clone() } else { format!(".{ext}") };
                let full_candidate = dir.join(format!("{name_trimmed}{ext_clean}"));
                if full_candidate.exists() {
                    return Some(full_candidate);
                }
            }
        }
    }

    find_in_program_dirs(name_trimmed, &extensions)
}

#[cfg(not(windows))]
pub fn find_executable_in_path(_name: &str) -> Option<PathBuf> {
    None
}