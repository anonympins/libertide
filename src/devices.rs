use cpal::traits::HostTrait;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PointingDevice {
    pub name: String,
    pub kind: String,
    pub details: String,
}

/// Liste l'ensemble des microphones d'entrée audio détectés par le système
pub fn list_microphones() -> Vec<String> {
    let host = cpal::default_host();
    let mut names = Vec::new();
    if let Ok(devices) = host.input_devices() {
        for d in devices {
            let trimmed = d.to_string().trim().to_string();
            if !trimmed.is_empty() && !names.contains(&trimmed) {
                names.push(trimmed);
            }
        }
    }
    names
}

/// Retourne le nom du microphone d'entrée configuré par défaut au niveau du système
pub fn get_default_microphone() -> Option<String> {
    let host = cpal::default_host();
    host.default_input_device()
        .map(|d| d.to_string().trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Retourne le microphone configuré pour l'application ("auto" ou nom spécifique)
pub fn get_active_microphone() -> String {
    std::env::var("AUDIO_INPUT_DEVICE")
        .ok()
        .or_else(|| crate::fs_util::read_env_var_from_file(".env", "AUDIO_INPUT_DEVICE"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "auto".to_string())
}

/// Enregistre le microphone sélectionné dans la configuration persistante (.env)
pub fn set_active_microphone(name: &str) -> Result<(), String> {
    let clean = name.trim();
    std::env::set_var("AUDIO_INPUT_DEVICE", clean);
    crate::fs_util::write_env_var_to_file(".env", "AUDIO_INPUT_DEVICE", clean)
        .map_err(|e| e.to_string())
}

/// Résout le périphérique d'entrée audio CPAL selon le choix utilisateur ou le défaut système
pub fn get_audio_input_device(host: &cpal::Host) -> Option<cpal::Device> {
    let active = get_active_microphone();
    if active != "auto" && !active.is_empty() {
        if let Ok(devices) = host.input_devices() {
            for d in devices {
                let name = d.to_string();
                if name.eq_ignore_ascii_case(&active) || name.contains(&active) {
                    return Some(d);
                }
            }
        }
    }
    host.default_input_device()
}

/// Détecte les dispositifs de pointage connectés (souris, trackballs, touchpads, écrans tactiles/stylets)
#[cfg(windows)]
pub fn list_pointing_devices() -> Vec<PointingDevice> {
    use std::os::windows::process::CommandExt;
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SYSTEM_METRICS_INDEX};
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let mut devices = Vec::new();

    // 1. Analyse des métriques système matérielles Windows
    unsafe {
        let mouse_present = GetSystemMetrics(SYSTEM_METRICS_INDEX(19));      // SM_MOUSEPRESENT
        let mouse_buttons = GetSystemMetrics(SYSTEM_METRICS_INDEX(43));      // SM_CMOUSEBUTTONS
        let wheel_present = GetSystemMetrics(SYSTEM_METRICS_INDEX(75));      // SM_MOUSEWHEELPRESENT
        let digitizer = GetSystemMetrics(SYSTEM_METRICS_INDEX(94));          // SM_DIGITIZER

        if digitizer != 0 {
            let mut digitizer_details = Vec::new();
            if digitizer & 0x01 != 0 { digitizer_details.push("Écran tactile intégré"); }
            if digitizer & 0x02 != 0 { digitizer_details.push("Écran tactile externe"); }
            if digitizer & 0x04 != 0 { digitizer_details.push("Stylet / Numériseur intégré"); }
            if digitizer & 0x08 != 0 { digitizer_details.push("Stylet / Numériseur externe"); }
            if digitizer & 0x40 != 0 { digitizer_details.push("Tactile multipoint"); }

            let details = if digitizer_details.is_empty() {
                "Numériseur tactile ou stylet actif".to_string()
            } else {
                digitizer_details.join(", ")
            };

            devices.push(PointingDevice {
                name: "Numériseur tactile / Stylet (Digitizer)".to_string(),
                kind: "Écran tactile / Stylet".to_string(),
                details,
            });
        }

        if mouse_present != 0 && devices.is_empty() {
            devices.push(PointingDevice {
                name: "Souris / Pointeur système principal".to_string(),
                kind: "Souris physique".to_string(),
                details: format!("{mouse_buttons} boutons{}", if wheel_present != 0 { ", avec molette" } else { "" }),
            });
        }
    }

    // 2. Énumération des périphériques de pointage réels enregistrés dans la classe Mouse du Registre Windows
    // GUID_DEVCLASS_MOUSE : {4D36E96F-E325-11CE-BFC1-08002BE10318}
    let output = std::process::Command::new("reg")
        .args([
            "query",
            r"HKLM\SYSTEM\CurrentControlSet\Control\Class\{4D36E96F-E325-11CE-BFC1-08002BE10318}",
            "/s",
            "/v",
            "DriverDesc",
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output();

    if let Ok(out) = output {
        let text = String::from_utf8_lossy(&out.stdout);
        let mut seen = std::collections::HashSet::new();

        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("DriverDesc") {
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() >= 3 {
                    let desc = parts[2..].join(" ");
                    let clean = desc.trim().to_string();
                    if !clean.is_empty() && seen.insert(clean.to_lowercase()) {
                        let lower = clean.to_lowercase();
                        let kind = if lower.contains("trackball") {
                            "Trackball ergonomique".to_string()
                        } else if lower.contains("touchpad") || lower.contains("trackpad") || lower.contains("synaptics") || lower.contains("elan") {
                            "Pavé tactile (Touchpad)".to_string()
                        } else if lower.contains("pen") || lower.contains("tablet") || lower.contains("wacom") || lower.contains("stylus") {
                            "Tablette graphique / Stylet".to_string()
                        } else {
                            "Souris optique / standard".to_string()
                        };

                        devices.push(PointingDevice {
                            name: clean,
                            kind,
                            details: "Périphérique matériel actif".to_string(),
                        });
                    }
                }
            }
        }
    }

    devices
}

#[cfg(target_os = "macos")]
pub fn list_pointing_devices() -> Vec<PointingDevice> {
    let mut devices = Vec::new();
    if let Ok(output) = std::process::Command::new("ioreg").args(["-r", "-c", "IOHIDPointing", "-l"]).output() {
        let text = String::from_utf8_lossy(&output.stdout);
        let mut seen = std::collections::HashSet::new();
        for line in text.lines() {
            if line.contains("\"Product\" = ") {
                if let Some((_, name_part)) = line.split_once('=') {
                    let clean = name_part.trim().trim_matches('"').to_string();
                    if !clean.is_empty() && seen.insert(clean.to_lowercase()) {
                        let lower = clean.to_lowercase();
                        let kind = if lower.contains("trackball") {
                            "Trackball ergonomique".to_string()
                        } else if lower.contains("trackpad") || lower.contains("touchpad") {
                            "Trackpad / Pavé tactile".to_string()
                        } else {
                            "Souris".to_string()
                        };
                        devices.push(PointingDevice {
                            name: clean,
                            kind,
                            details: "Périphérique IOHIDPointing macOS".to_string(),
                        });
                    }
                }
            }
        }
    }
    if devices.is_empty() {
        devices.push(PointingDevice {
            name: "Trackpad / Souris Apple".to_string(),
            kind: "Dispositif de pointage".to_string(),
            details: "Pointeur standard macOS".to_string(),
        });
    }
    devices
}

#[cfg(all(not(windows), not(target_os = "macos")))]
pub fn list_pointing_devices() -> Vec<PointingDevice> {
    let mut devices = Vec::new();
    if let Ok(content) = std::fs::read_to_string("/proc/bus/input/devices") {
        let mut cur_name = String::new();
        let mut is_pointing = false;

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("N: Name=") {
                cur_name = trimmed.trim_start_matches("N: Name=")
                    .trim_matches('"')
                    .to_string();
                is_pointing = false;
            } else if trimmed.starts_with("H: Handlers=") {
                if trimmed.contains("mouse") || trimmed.contains("touchpad") {
                    is_pointing = true;
                }
            } else if trimmed.is_empty() {
                if is_pointing && !cur_name.is_empty() {
                    let lower = cur_name.to_lowercase();
                    let kind = if lower.contains("trackball") {
                        "Trackball ergonomique".to_string()
                    } else if lower.contains("touchpad") || lower.contains("trackpad") {
                        "Pavé tactile (Touchpad)".to_string()
                    } else if lower.contains("tablet") || lower.contains("pen") || lower.contains("wacom") {
                        "Tablette / Stylet".to_string()
                    } else {
                        "Souris".to_string()
                    };

                    devices.push(PointingDevice {
                        name: cur_name.clone(),
                        kind,
                        details: "Gestionnaire /dev/input actif".to_string(),
                    });
                }
                cur_name.clear();
                is_pointing = false;
            }
        }
    }
    if devices.is_empty() {
        devices.push(PointingDevice {
            name: "Pointeur système (X11 / Wayland)".to_string(),
            kind: "Souris / Pointeur".to_string(),
            details: "Dispositif d'entrée générique".to_string(),
        });
    }
    devices
}