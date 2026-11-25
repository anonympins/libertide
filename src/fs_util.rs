use std::path::{Path, PathBuf};
use std::sync::OnceLock;

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

/// Résout l'emplacement du fichier de configuration des macros (macros.json)
pub fn find_macros_file() -> PathBuf {
    let env_file = find_env_file();
    env_file
        .parent()
        .map(|p| p.join("macros.json"))
        .unwrap_or_else(|| PathBuf::from("macros.json"))
}

/// Résout l'emplacement du fichier de configuration des remappages de touches (key_remaps.json)
pub fn find_key_remaps_file() -> PathBuf {
    let direct = PathBuf::from("key_remaps.json");
    if direct.is_file() {
        return direct;
    }
    let manifest_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("key_remaps.json");
    if manifest_path.is_file() {
        return manifest_path;
    }
    let env_file = find_env_file();
    if let Some(parent) = env_file.parent() {
        let p = parent.join("key_remaps.json");
        if p.is_file() {
            return p;
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        let mut curr = exe.parent();
        for _ in 0..6 {
            if let Some(dir) = curr {
                let p = dir.join("key_remaps.json");
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
                let p = dir.join("key_remaps.json");
                if p.is_file() {
                    return p;
                }
                curr = dir.parent();
            } else {
                break;
            }
        }
    }
    PathBuf::from("key_remaps.json")
}

/// Ouvre une boîte de dialogue native pour sélectionner un fichier ou une image
#[cfg(windows)]
pub fn pick_file_dialog() -> Option<PathBuf> {
    #[repr(C)]
    struct OPENFILENAMEW {
        lStructSize: u32,
        hwndOwner: *mut std::ffi::c_void,
        hInstance: *mut std::ffi::c_void,
        lpstrFilter: *const u16,
        lpstrCustomFilter: *mut u16,
        nMaxCustFilter: u32,
        nFilterIndex: u32,
        lpstrFile: *mut u16,
        nMaxFile: u32,
        lpstrFileTitle: *mut u16,
        nMaxFileTitle: u32,
        lpstrInitialDir: *const u16,
        lpstrTitle: *const u16,
        Flags: u32,
        nFileOffset: u16,
        nFileExtension: u16,
        lpstrDefExt: *const u16,
        lCustData: usize,
        lpfnHook: *const (),
        lpTemplateName: *const u16,
        pvReserved: *mut std::ffi::c_void,
        dwReserved: u32,
        FlagsEx: u32,
    }

    #[link(name = "comdlg32")]
    extern "system" {
        fn GetOpenFileNameW(lpofn: *mut OPENFILENAMEW) -> windows::Win32::Foundation::BOOL;
    }

    let mut file_buf = vec![0u16; 1024];
    let filter: Vec<u16> = "Fichiers multimédias et documents\0*.png;*.jpg;*.jpeg;*.webp;*.bmp;*.svg;*.pdf;*.txt;*.md;*.json;*.rs;*.py;*.c;*.cpp;*.toml;*.csv\0Tous les fichiers (*.*)\0*.*\0\0"
        .encode_utf16()
        .collect();
    let title: Vec<u16> = "Sélectionner un fichier ou une image\0".encode_utf16().collect();

    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: std::ptr::null_mut(),
        hInstance: std::ptr::null_mut(),
        lpstrFilter: filter.as_ptr(),
        lpstrCustomFilter: std::ptr::null_mut(),
        nMaxCustFilter: 0,
        nFilterIndex: 1,
        lpstrFile: file_buf.as_mut_ptr(),
        nMaxFile: file_buf.len() as u32,
        lpstrFileTitle: std::ptr::null_mut(),
        nMaxFileTitle: 0,
        lpstrInitialDir: std::ptr::null(),
        lpstrTitle: title.as_ptr(),
        Flags: 0x00080000 | 0x00001000 | 0x00000800 | 0x00000004, // OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_HIDEREADONLY
        nFileOffset: 0,
        nFileExtension: 0,
        lpstrDefExt: std::ptr::null(),
        lCustData: 0,
        lpfnHook: std::ptr::null(),
        lpTemplateName: std::ptr::null(),
        pvReserved: std::ptr::null_mut(),
        dwReserved: 0,
        FlagsEx: 0,
    };

    let ok = unsafe { GetOpenFileNameW(&mut ofn) };
    if ok.as_bool() {
        let len = file_buf.iter().position(|&c| c == 0).unwrap_or(file_buf.len());
        if len > 0 {
            let path_str = String::from_utf16_lossy(&file_buf[..len]);
            let p = PathBuf::from(path_str.trim());
            if p.is_file() {
                return Some(p);
            }
        }
    }

    // Repli de secours via PowerShell configuré avec l'appartement STA
    let ps_script = r#"[System.Reflection.Assembly]::LoadWithPartialName('System.Windows.Forms') | Out-Null; $f = New-Object System.Windows.Forms.OpenFileDialog; $f.Title = 'Sélectionner un fichier ou une image'; $f.Filter = 'Fichiers multimédias et documents (*.png;*.jpg;*.jpeg;*.webp;*.bmp;*.svg;*.pdf;*.txt;*.md;*.json;*.rs;*.py)|*.png;*.jpg;*.jpeg;*.webp;*.bmp;*.svg;*.pdf;*.txt;*.md;*.json;*.rs;*.py|Tous les fichiers (*.*)|*.*'; if ($f.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) { Write-Output $f.FileName }"#;
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-STA", "-Command", ps_script])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    if out.status.success() {
        let path_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !path_str.is_empty() {
            let p = PathBuf::from(path_str);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
pub fn pick_file_dialog() -> Option<PathBuf> {
    let script = "POSIX path of (choose file with prompt \"Sélectionner un fichier ou une image\")";
    let out = std::process::Command::new("osascript")
        .args(["-e", script])
        .output()
        .ok()?;
    if out.status.success() {
        let path_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !path_str.is_empty() {
            let p = PathBuf::from(path_str);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

#[cfg(all(not(windows), not(target_os = "macos")))]
pub fn pick_file_dialog() -> Option<PathBuf> {
    if let Ok(out) = std::process::Command::new("zenity")
        .args(["--file-selection", "--title=Sélectionner un fichier ou une image"])
        .output()
    {
        if out.status.success() {
            let path_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !path_str.is_empty() {
                let p = PathBuf::from(path_str);
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// Extrait le texte d'un fichier PDF (documents vectoriels, attestations CPAM, France Travail, factures)
#[cfg(windows)]
pub fn read_pdf_text(path: &Path) -> Option<String> {
    if !path.is_file() {
        return None;
    }
    let path_str = path.to_string_lossy().to_string();

    // 1. Essai ultra-rapide avec pdftotext si présent dans le PATH
    if let Ok(out) = std::process::Command::new("pdftotext")
        .args(["-layout", "-enc", "UTF-8", &path_str, "-"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if s.len() > 10 {
                return Some(s);
            }
        }
    }

    // 2. Décompression native des flux /FlateDecode PDF via PowerShell et .NET DeflateStream
    let ps_script = r#"
$p = if ($args[0]) { $args[0] } else { [Console]::In.ReadLine() }
$ErrorActionPreference = 'SilentlyContinue'
if (-not (Test-Path -LiteralPath $p)) { exit 1 }
$bytes = [System.IO.File]::ReadAllBytes($p)
$latin1 = [System.Text.Encoding]::GetEncoding(28591)
$raw = $latin1.GetString($bytes)
$sb = New-Object System.Text.StringBuilder

$pos = 0
while (($sIdx = $raw.IndexOf('stream', $pos)) -ge 0) {
    $start = $sIdx + 6
    if ($start -lt $bytes.Length -and $bytes[$start] -eq 13) { $start++ }
    if ($start -lt $bytes.Length -and $bytes[$start] -eq 10) { $start++ }
    $eIdx = $raw.IndexOf('endstream', $start)
    if ($eIdx -lt 0) { break }
    $len = $eIdx - $start
    while ($len -gt 0 -and ($bytes[$start + $len - 1] -eq 10 -or $bytes[$start + $len - 1] -eq 13)) { $len-- }
    if ($len -gt 0) {
        $sBytes = New-Object byte[] $len
        [System.Array]::Copy($bytes, $start, $sBytes, 0, $len)
        $text = $null
        if ($len -gt 2 -and $sBytes[0] -eq 120) {
            try {
                $ms = New-Object System.IO.MemoryStream($sBytes, 2, $len - 2)
                $ds = New-Object System.IO.Compression.DeflateStream($ms, [System.IO.Compression.CompressionMode]::Decompress)
                $outMs = New-Object System.IO.MemoryStream
                $ds.CopyTo($outMs)
                $text = $latin1.GetString($outMs.ToArray())
            } catch {}
        } else {
            $text = $latin1.GetString($sBytes)
        }
        if ($text -and ($text.Contains('BT') -or $text.Contains('Tj') -or $text.Contains('TJ'))) {
            $mList = [regex]::Matches($text, '(?s)\((?:[^()\\]|\\.)*\)\s*(?:Tj|\'')|\[(.*?)\]\s*TJ|<([0-9a-fA-F\s]+)>\s*(?:Tj|TJ)')
            foreach ($m in $mList) {
                if ($m.Groups[1].Success) {
                    $subs = [regex]::Matches($m.Groups[1].Value, '\((?:[^()\\]|\\.)*\)')
                    foreach ($sm in $subs) {
                        $v = $sm.Value
                        $v = $v.Substring(1, $v.Length - 2)
                        $v = [regex]::Replace($v, '\\([0-7]{1,3})', { param($o) [char][Convert]::ToInt32($o.Groups[1].Value, 8) })
                        $v = $v.Replace('\(', '(').Replace('\)', ')').Replace('\\', '\').Replace('\n', ' ').Replace('\r', '').Replace('\t', ' ')
                        [void]$sb.Append($v)
                    }
                    [void]$sb.Append(' ')
                } elseif ($m.Groups[2].Success) {
                    $h = $m.Groups[2].Value -replace '\s',''
                    if ($h.Length -ge 4 -and ($h.Length % 4 -eq 0) -and ($h.StartsWith('FEFF') -or $h.StartsWith('00'))) {
                        for ($k=0; $k -lt $h.Length; $k+=4) {
                            $code = [Convert]::ToUInt16($h.Substring($k, 4), 16)
                            if ($code -ne 0xFEFF) { [void]$sb.Append([char]$code) }
                        }
                        [void]$sb.Append(' ')
                    } elseif ($h.Length -ge 2 -and ($h.Length % 2 -eq 0)) {
                        for ($k=0; $k -lt $h.Length; $k+=2) {
                            [void]$sb.Append([char][Convert]::ToByte($h.Substring($k, 2), 16))
                        }
                        [void]$sb.Append(' ')
                    }
                } else {
                    $v = $m.Value
                    $ep = $v.LastIndexOf(')')
                    if ($ep -gt 0) {
                        $v = $v.Substring(1, $ep - 1)
                        $v = [regex]::Replace($v, '\\([0-7]{1,3})', { param($o) [char][Convert]::ToInt32($o.Groups[1].Value, 8) })
                        $v = $v.Replace('\(', '(').Replace('\)', ')').Replace('\\', '\').Replace('\n', ' ').Replace('\r', '').Replace('\t', ' ')
                        [void]$sb.Append($v)
                        [void]$sb.Append(' ')
                    }
                }
            }
            [void]$sb.AppendLine()
        }
    }
    $pos = $eIdx + 9
}
$res = $sb.ToString().Trim()
if ($res.Length -gt 10) {
    [Console]::OutputEncoding = [System.Text.Encoding]::UTF8
    Write-Output $res
    exit 0
}
if (Get-Command python -ErrorAction SilentlyContinue) {
    try {
        $pyCmd = "import pypdf, sys; r=pypdf.PdfReader(sys.argv[1]); print('\n'.join(p.extract_text() or '' for p in r.pages))"
        $pyRes = & python -c $pyCmd $p
        if ($pyRes -and $pyRes.Trim().Length -gt 10) {
            [Console]::OutputEncoding = [System.Text.Encoding]::UTF8
            Write-Output $pyRes.Trim()
            exit 0
        }
    } catch {}
}
"#;
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", ps_script, &path_str])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    if out.status.success() {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if s.len() > 10 {
            return Some(s);
        }
    }
    None
}

#[cfg(target_os = "macos")]
pub fn read_pdf_text(path: &Path) -> Option<String> {
    if !path.is_file() {
        return None;
    }
    let path_str = path.to_string_lossy();
    let script = format!(
        "ObjC.import('PDFKit'); var url = $.NSURL.fileURLWithPath('{}'); var doc = $.PDFDocument.alloc.initWithURL(url); doc ? doc.string.js : '';",
        path_str.replace('\'', "\\'")
    );
    let out = std::process::Command::new("osascript")
        .args(["-l", "JavaScript", "-e", &script])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !s.is_empty() { Some(s) } else { None }
}

#[cfg(all(not(windows), not(target_os = "macos")))]
pub fn read_pdf_text(path: &Path) -> Option<String> {
    let out = std::process::Command::new("pdftotext").args(["-layout", "-enc", "UTF-8"]).arg(path).arg("-").output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !s.is_empty() { Some(s) } else { None }
}

/// Charge la liste des macros enregistrées ('raccourci' => 'texte complet') en fusionnant .env et macros.json
pub fn load_macros() -> Vec<(String, String)> {
    let path = find_macros_file();
    if let Ok(content) = std::fs::read_to_string(&path) {
        if let Ok(list) = serde_json::from_str::<Vec<(String, String)>>(&content) {
            return list;
        }
        if let Ok(map) = serde_json::from_str::<std::collections::BTreeMap<String, String>>(&content) {
            return map.into_iter().collect();
        }
    }

    // Lecture de macros par défaut configurées dans le fichier .env (DEFAULT_MACROS ou MACROS)
    if let Ok(env_macros) = std::env::var("DEFAULT_MACROS")
        .or_else(|_| read_env_var_from_file(".env", "DEFAULT_MACROS").ok_or(()))
        .or_else(|_| std::env::var("MACROS"))
        .or_else(|_| read_env_var_from_file(".env", "MACROS").ok_or(()))
    {
        if let Ok(list) = serde_json::from_str::<Vec<(String, String)>>(&env_macros) {
            return list;
        }
        if let Ok(map) = serde_json::from_str::<std::collections::BTreeMap<String, String>>(&env_macros) {
            return map.into_iter().collect();
        }
    }

    vec![
        ("actu".to_string(), "Donne les actualités et titres majeurs du jour sous forme de synthèse concise.".to_string()),
        ("colis".to_string(), "Vérifie le statut de ma commande ou le suivi de livraison à l'écran.".to_string()),
        ("aide".to_string(), "Explique clairement ce qui est affiché à l'écran et guide-moi sur les actions possibles.".to_string()),
        ("resume".to_string(), "Fais un résumé clair et accessible de la page ou du document actif.".to_string()),
        ("meteo".to_string(), "Donne la météo actuelle et les prévisions de la journée.".to_string()),
        ("panier".to_string(), "Consulte mon panier d'achat et présente les articles avant toute validation.".to_string()),
    ]
}

/// Enregistre les macros dans macros.json
pub fn save_macros(macros: &[(String, String)]) -> std::io::Result<()> {
    let path = find_macros_file();
    let json = serde_json::to_string_pretty(macros).unwrap_or_default();
    std::fs::write(path, json)
}

/// Déploie un raccourci de macro en son texte complet si une correspondance existe
pub fn expand_macro_text(input: &str, macros: &[(String, String)]) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return input.to_string();
    }
    for (shortcut, expansion) in macros {
        let sc = shortcut.trim();
        if sc.is_empty() {
            continue;
        }
        if trimmed.eq_ignore_ascii_case(sc)
            || (trimmed.starts_with('!') && trimmed[1..].trim().eq_ignore_ascii_case(sc))
            || (trimmed.starts_with('/') && trimmed[1..].trim().eq_ignore_ascii_case(sc))
            || (trimmed.starts_with('#') && trimmed[1..].trim().eq_ignore_ascii_case(sc))
        {
            return expansion.clone();
        }
    }
    input.to_string()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_macro_only_when_exact() {
        let macros = vec![("actu".to_string(), "Donne les actualités.".to_string())];
        assert_eq!(expand_macro_text("actu", &macros), "Donne les actualités.");
        assert_eq!(expand_macro_text("!actu", &macros), "Donne les actualités.");
        assert_eq!(expand_macro_text("/actu", &macros), "Donne les actualités.");
        assert_eq!(expand_macro_text("#actu", &macros), "Donne les actualités.");

        // Pas d'expansion si du texte supplémentaire est présent
        assert_eq!(expand_macro_text("actu du jour", &macros), "actu du jour");
        assert_eq!(expand_macro_text("!actu de demain", &macros), "!actu de demain");
    }
}

/// Collecte et met en cache l'ensemble des exécutables disponibles dans le PATH et le système
pub fn get_available_applications() -> &'static [String] {
    static APPS: OnceLock<Vec<String>> = OnceLock::new();
    APPS.get_or_init(|| {
        let mut apps = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // Applications système et utilitaires courants indispensables
        let builtins = [
            "calc", "notepad", "chrome", "firefox", "edge", "brave", "explorer",
            "cmd", "powershell", "wt", "code", "spotify", "discord", "vlc",
            "steam", "taskmgr", "settings", "mspaint", "terminal", "obs",
        ];
        for b in builtins {
            if seen.insert(b.to_lowercase()) {
                apps.push(b.to_string());
            }
        }

        #[cfg(windows)]
        {
            let dirs = get_system_search_dirs();
            for dir in dirs {
                if let Ok(entries) = std::fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_file() {
                            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                                if ext.eq_ignore_ascii_case("exe")
                                    || ext.eq_ignore_ascii_case("cmd")
                                    || ext.eq_ignore_ascii_case("bat")
                                {
                                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                                        let clean = stem.trim();
                                        let lower = clean.to_lowercase();
                                        if clean.len() >= 2
                                            && !clean.starts_with('.')
                                            && !clean.starts_with('_')
                                            && !lower.contains("uninstall")
                                            && !lower.starts_with("vcruntime")
                                            && !lower.starts_with("d3dcompiler")
                                            && seen.insert(lower)
                                        {
                                            apps.push(clean.to_string());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Découverte des raccourcis du menu Démarrer (.lnk)
            let mut start_menu_dirs = Vec::new();
            if let Ok(program_data) = std::env::var("ProgramData") {
                start_menu_dirs.push(PathBuf::from(program_data).join(r"Microsoft\Windows\Start Menu\Programs"));
            }
            if let Ok(app_data) = std::env::var("APPDATA") {
                start_menu_dirs.push(PathBuf::from(app_data).join(r"Microsoft\Windows\Start Menu\Programs"));
            }

            for start_dir in start_menu_dirs {
                if let Ok(entries) = std::fs::read_dir(&start_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_file() {
                            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                                if ext.eq_ignore_ascii_case("lnk") {
                                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                                        let clean = stem.trim();
                                        let lower = clean.to_lowercase();
                                        if clean.len() >= 2 && !lower.contains("uninstall") && seen.insert(lower) {
                                            apps.push(clean.to_string());
                                        }
                                    }
                                }
                            }
                        } else if path.is_dir() {
                            if let Ok(sub_entries) = std::fs::read_dir(&path) {
                                for sub in sub_entries.flatten() {
                                    let sub_p = sub.path();
                                    if let Some(ext) = sub_p.extension().and_then(|e| e.to_str()) {
                                        if ext.eq_ignore_ascii_case("lnk") {
                                            if let Some(stem) = sub_p.file_stem().and_then(|s| s.to_str()) {
                                                let clean = stem.trim();
                                                let lower = clean.to_lowercase();
                                                if clean.len() >= 2 && !lower.contains("uninstall") && seen.insert(lower) {
                                                    apps.push(clean.to_string());
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        #[cfg(not(windows))]
        {
            if let Some(path_var) = std::env::var_os("PATH") {
                for dir in std::env::split_paths(&path_var) {
                    if let Ok(entries) = std::fs::read_dir(dir) {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if path.is_file() {
                                if let Some(fname) = path.file_name().and_then(|s| s.to_str()) {
                                    let clean = fname.trim();
                                    let lower = clean.to_lowercase();
                                    if clean.len() >= 2 && !clean.starts_with('.') && seen.insert(lower) {
                                        apps.push(clean.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        apps
    })
}

/// Recherche les suggestions d'applications correspondant au texte saisi avec distance de Damerau-Levenshtein
pub fn find_matching_applications(input: &str, limit: usize) -> Vec<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let lower_trimmed = trimmed.to_lowercase();
    let prefixes = ["ouvre ", "ouvrir ", "lance ", "lancer ", "open ", "run ", "demarre ", "démarre "];
    let mut query = lower_trimmed.as_str();
    for p in prefixes {
        if query.starts_with(p) {
            query = &query[p.len()..];
            break;
        }
    }
    let q = query.trim();
    if q.len() < 2 {
        return Vec::new();
    }

    let all_apps = get_available_applications();
    let mut scored: Vec<(f32, &String)> = Vec::new();

    for app in all_apps {
        let app_lower = app.to_lowercase();
        if app_lower == q {
            scored.push((1000.0, app));
        } else if app_lower.starts_with(q) {
            let score = 500.0 + (q.len() as f32 / app_lower.len() as f32) * 50.0;
            scored.push((score, app));
        } else if app_lower.contains(q) {
            let score = 300.0 + (q.len() as f32 / app_lower.len() as f32) * 30.0;
            scored.push((score, app));
        } else if q.len() >= 3 {
            let dist = crate::pattern::damerau_levenshtein_distance(q, &app_lower);
            let max_len = q.chars().count().max(app_lower.chars().count());
            let sim = 1.0 - (dist as f32 / max_len as f32);

            let prefix_sim = if app_lower.chars().count() > q.chars().count() {
                let prefix: String = app_lower.chars().take(q.chars().count()).collect();
                let p_dist = crate::pattern::damerau_levenshtein_distance(q, &prefix);
                1.0 - (p_dist as f32 / q.chars().count() as f32)
            } else {
                0.0
            };

            if sim >= 0.60 {
                let score = 200.0 * sim;
                scored.push((score, app));
            } else if prefix_sim >= 0.70 && q.len() >= 4 {
                let score = 150.0 * prefix_sim;
                scored.push((score, app));
            }
        }
    }

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    let mut result = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (_, app) in scored {
        if seen.insert(app.to_lowercase()) {
            result.push(app.clone());
            if result.len() >= limit {
                break;
            }
        }
    }
    result
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
    let trimmed = path.trim();

    // Gestion directe des URIs Windows (ex: ms-settings:sound) sans invite de commande
    if trimmed.starts_with("ms-settings:") {
        let explorer_path = r"C:\Windows\explorer.exe";
        let status = std::process::Command::new(explorer_path)
            .arg(trimmed)
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .map_err(|e| format!("Erreur lors du lancement de explorer.exe : {e}"))?;
        return if status.success() { Ok(()) } else { Err("Code retour non nul".into()) };
    }

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

#[cfg(target_os = "macos")]
pub fn open_path_with_default_app(path: &str) -> Result<(), String> {
    let status = std::process::Command::new("open")
        .arg(path)
        .status()
        .map_err(|e| format!("Erreur lors de l'ouverture du fichier : {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("Code de retour non nul pour open '{path}'"))
    }
}

#[cfg(all(not(windows), not(target_os = "macos")))]
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
pub fn find_executable_in_path(name: &str) -> Option<PathBuf> {
    let name_trimmed = name.trim();
    if name_trimmed.is_empty() {
        return None;
    }
    let p = Path::new(name_trimmed);
    if p.is_absolute() && p.is_file() {
        return Some(p.to_path_buf());
    }
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name_trimmed);
        if candidate.is_file() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = candidate.metadata() {
                    if meta.permissions().mode() & 0o111 != 0 {
                        return Some(candidate);
                    }
                }
            }
            #[cfg(not(unix))]
            return Some(candidate);
        }
    }
    None
}