#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[cfg(windows)]
pub fn is_autostart_enabled() -> bool {
    let output = std::process::Command::new("reg")
        .args(["query", r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run", "/v", "Libertide"])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    if let Ok(out) = output {
        out.status.success()
    } else {
        false
    }
}

#[cfg(windows)]
pub fn set_autostart(enable: bool) -> Result<String, String> {
    if enable {
        let exe_path = std::env::current_exe()
            .map_err(|e| format!("Impossible de déterminer le chemin de l'exécutable : {e}"))?;
        let exe_str = exe_path.to_string_lossy();
        let val = format!("\"{}\"", exe_str);
        let status = std::process::Command::new("reg")
            .args([
                "add",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                "Libertide",
                "/t",
                "REG_SZ",
                "/d",
                &val,
                "/f",
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .map_err(|e| format!("Erreur lors de l'exécution de reg.exe : {e}"))?;

        if status.success() {
            Ok(format!("Lancement au démarrage de Windows activé (exécutable : {}).", exe_str))
        } else {
            Err("Échec de l'écriture dans le Registre Windows.".to_string())
        }
    } else {
        let status = std::process::Command::new("reg")
            .args([
                "delete",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                "Libertide",
                "/f",
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .map_err(|e| format!("Erreur lors de l'exécution de reg.exe : {e}"))?;

        if status.success() {
            Ok("Lancement au démarrage de Windows désactivé.".to_string())
        } else {
            Ok("Lancement au démarrage désactivé (aucune entrée existante dans le Registre).".to_string())
        }
    }
}

#[cfg(all(not(windows), not(target_os = "macos")))]
fn get_xdg_autostart_path() -> Result<std::path::PathBuf, String> {
    let config_dir = if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        let p = std::path::PathBuf::from(xdg.trim());
        if !xdg.trim().is_empty() {
            p
        } else {
            let home = std::env::var("HOME").map_err(|e| format!("Variable HOME introuvable : {e}"))?;
            std::path::PathBuf::from(home).join(".config")
        }
    } else {
        let home = std::env::var("HOME").map_err(|e| format!("Variable HOME introuvable : {e}"))?;
        std::path::PathBuf::from(home).join(".config")
    };
    Ok(config_dir.join("autostart").join("libertide.desktop"))
}

#[cfg(all(not(windows), not(target_os = "macos")))]
pub fn is_autostart_enabled() -> bool {
    let Ok(desktop_path) = get_xdg_autostart_path() else {
        return false;
    };
    if !desktop_path.is_file() {
        return false;
    }
    if let Ok(content) = std::fs::read_to_string(&desktop_path) {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.eq_ignore_ascii_case("hidden=true")
                || trimmed.eq_ignore_ascii_case("x-gnome-autostart-enabled=false")
            {
                return false;
            }
        }
        true
    } else {
        false
    }
}

#[cfg(all(not(windows), not(target_os = "macos")))]
pub fn set_autostart(enable: bool) -> Result<String, String> {
    let desktop_path = get_xdg_autostart_path()?;
    let autostart_dir = desktop_path
        .parent()
        .ok_or_else(|| "Dossier autostart invalide".to_string())?;

    if enable {
        std::fs::create_dir_all(autostart_dir)
            .map_err(|e| format!("Impossible de créer le dossier autostart ({}) : {e}", autostart_dir.display()))?;
        let exe = std::env::current_exe()
            .map_err(|e| format!("Chemin exécutable introuvable : {e}"))?;
        let exe_dir = exe.parent().unwrap_or_else(|| std::path::Path::new("/"));
        let exe_str = exe.to_string_lossy();

        let content = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=Libertide\n\
             Comment=Assistant vocal et automation Libertide\n\
             Exec=\"{}\"\n\
             Path={}\n\
             Terminal=false\n\
             StartupNotify=false\n\
             X-GNOME-Autostart-enabled=true\n",
            exe_str.replace('"', "\\\""),
            exe_dir.display()
        );
        std::fs::write(&desktop_path, content)
            .map_err(|e| format!("Échec d'écriture du fichier .desktop ({}) : {e}", desktop_path.display()))?;
        Ok(format!("Démarrage automatique activé dans {}.", desktop_path.display()))
    } else {
        if desktop_path.exists() {
            std::fs::remove_file(&desktop_path)
                .map_err(|e| format!("Échec de suppression du fichier .desktop ({}) : {e}", desktop_path.display()))?;
            Ok("Démarrage automatique désactivé (fichier .desktop supprimé).".to_string())
        } else {
            Ok("Démarrage automatique désactivé (aucune entrée existante).".to_string())
        }
    }
}

#[cfg(target_os = "macos")]
fn get_launch_agent_path() -> Result<std::path::PathBuf, String> {
    let home = std::env::var("HOME").map_err(|e| format!("Variable HOME introuvable : {e}"))?;
    Ok(std::path::PathBuf::from(home)
        .join("Library")
        .join("LaunchAgents")
        .join("com.libertide.overlay.plist"))
}

#[cfg(target_os = "macos")]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(target_os = "macos")]
pub fn is_autostart_enabled() -> bool {
    get_launch_agent_path().map_or(false, |p| p.is_file())
}

#[cfg(target_os = "macos")]
pub fn set_autostart(enable: bool) -> Result<String, String> {
    let plist_path = get_launch_agent_path()?;
    let launch_agents_dir = plist_path
        .parent()
        .ok_or_else(|| "Dossier LaunchAgents invalide".to_string())?;

    if enable {
        std::fs::create_dir_all(launch_agents_dir)
            .map_err(|e| format!("Impossible de créer le dossier LaunchAgents ({}) : {e}", launch_agents_dir.display()))?;
        let exe = std::env::current_exe()
            .map_err(|e| format!("Chemin exécutable introuvable : {e}"))?;
        let exe_dir = exe.parent().unwrap_or_else(|| std::path::Path::new("/"));
        let exe_str = exe.to_string_lossy();
        let exe_dir_str = exe_dir.to_string_lossy();

        let content = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.libertide.overlay</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
    </array>
    <key>WorkingDirectory</key>
    <string>{}</string>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <false/>
</dict>
</plist>
"#,
            xml_escape(&exe_str),
            xml_escape(&exe_dir_str)
        );

        std::fs::write(&plist_path, content)
            .map_err(|e| format!("Échec d'écriture du fichier LaunchAgent ({}) : {e}", plist_path.display()))?;

        let _ = std::process::Command::new("launchctl")
            .args(["load", "-w", &plist_path.to_string_lossy()])
            .status();

        Ok(format!("Lancement au démarrage macOS activé (LaunchAgent : {}).", plist_path.display()))
    } else {
        if plist_path.exists() {
            let _ = std::process::Command::new("launchctl")
                .args(["unload", "-w", &plist_path.to_string_lossy()])
                .status();
            std::fs::remove_file(&plist_path)
                .map_err(|e| format!("Échec de suppression du fichier LaunchAgent ({}) : {e}", plist_path.display()))?;
            Ok("Lancement au démarrage macOS désactivé (fichier LaunchAgent supprimé).".to_string())
        } else {
            Ok("Lancement au démarrage macOS désactivé (aucune entrée existante).".to_string())
        }
    }
}