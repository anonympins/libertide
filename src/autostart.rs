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

#[cfg(not(windows))]
pub fn is_autostart_enabled() -> bool {
    std::env::var("HOME")
        .map(|home| std::path::PathBuf::from(home).join(".config/autostart/libertide.desktop").exists())
        .unwrap_or(false)
}

#[cfg(not(windows))]
pub fn set_autostart(enable: bool) -> Result<String, String> {
    let home = std::env::var("HOME").map_err(|e| format!("Variable HOME introuvable : {e}"))?;
    let autostart_dir = std::path::PathBuf::from(home).join(".config/autostart");
    let desktop_file = autostart_dir.join("libertide.desktop");

    if enable {
        std::fs::create_dir_all(&autostart_dir)
            .map_err(|e| format!("Impossible de créer le dossier autostart : {e}"))?;
        let exe = std::env::current_exe()
            .map_err(|e| format!("Chemin exécutable introuvable : {e}"))?;
        let content = format!(
            "[Desktop Entry]\nType=Application\nName=Libertide\nExec={}\nTerminal=false\nStartupNotify=false\n",
            exe.display()
        );
        std::fs::write(&desktop_file, content)
            .map_err(|e| format!("Échec d'écriture du fichier .desktop : {e}"))?;
        Ok("Démarrage automatique activé dans ~/.config/autostart/libertide.desktop.".to_string())
    } else {
        if desktop_file.exists() {
            std::fs::remove_file(&desktop_file)
                .map_err(|e| format!("Échec de suppression du fichier .desktop : {e}"))?;
        }
        Ok("Démarrage automatique désactivé (fichier .desktop supprimé).".to_string())
    }
}