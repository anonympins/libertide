use std::path::{Path, PathBuf};

/// Lit une variable d'environnement définie dans un fichier texte (par exemple `.env`).
pub fn read_env_var_from_file(file_path: &str, key: &str) -> Option<String> {
    let content = std::fs::read_to_string(file_path).ok()?;
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