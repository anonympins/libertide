use std::path::PathBuf;

#[cfg(windows)]
use windows::Win32::Foundation::SYSTEMTIME;
#[cfg(windows)]
use windows::Win32::System::SystemInformation::{
    GetLocalTime, GetSystemPowerStatus, GlobalMemoryStatusEx, MEMORYSTATUSEX, SYSTEM_POWER_STATUS,
};

/// Retourne la date et l'heure courante détaillée en français
pub fn current_datetime_full_str() -> String {
    #[cfg(windows)]
    unsafe {
        let mut st = SYSTEMTIME::default();
        GetLocalTime(&mut st);

        const DAYS: [&str; 7] = ["Dimanche", "Lundi", "Mardi", "Mercredi", "Jeudi", "Vendredi", "Samedi"];
        const MONTHS: [&str; 13] = [
            "", "janvier", "février", "mars", "avril", "mai", "juin",
            "juillet", "août", "septembre", "octobre", "novembre", "décembre",
        ];

        let day_name = DAYS.get(st.day_of_week as usize).unwrap_or(&"");
        let day_name = DAYS.get(st.wDayOfWeek as usize).unwrap_or(&"");
        let month_name = MONTHS.get(st.wMonth as usize).unwrap_or(&"");

        format!(
            "{} {} {} {}, {:02}:{:02}:{:02}",
            day_name, st.wDay, month_name, st.wYear, st.wHour, st.wMinute, st.wSecond
        )
    }
    #[cfg(not(windows))]
    {
        let now = std::time::SystemTime::now();
        let output = std::process::Command::new("date")
            .arg("+%A %d %B %Y, %H:%M:%S")
            .output();
        if let Ok(out) = output {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !s.is_empty() {
                return s;
            }
        }
        let secs = now.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
        format!("Timestamp UNIX: {} ({:02}:{:02}:{:02})", secs, (secs / 3600) % 24, (secs / 60) % 60, secs % 60)
    }
}

#[cfg(windows)]
/// Récupère l'état de l'alimentation et de la batterie
pub fn get_power_status() -> String {
    #[cfg(windows)]
    unsafe {
        let mut sps = SYSTEM_POWER_STATUS::default();
        if GetSystemPowerStatus(&mut sps).is_ok() {
            let ac = match sps.ACLineStatus {
                0 => "Sur batterie",
                1 => "Branché sur secteur",
                _ => "Alimentation inconnue",
            };
            let percent = if sps.BatteryLifePercent <= 100 {
                format!("{}%", sps.BatteryLifePercent)
            } else {
                "Niveau inconnu / absent".to_string()
            };
            return format!("{ac} (Batterie : {percent})");
        }
    }
    "Non disponible".to_string()
}

#[cfg(not(windows))]
pub fn get_power_status() -> String {
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = std::process::Command::new("pmset").args(["-g", "batt"]).output() {
            let s = String::from_utf8_lossy(&output.stdout);
            if let Some(line) = s.lines().nth(1) {
                return line.trim().to_string();
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    let base_path = std::path::Path::new("/sys/class/power_supply");
    if let Ok(entries) = std::fs::read_dir(base_path) {
        for entry in entries.flatten() {
            let p = entry.path();
            let fname = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if fname.starts_with("BAT") {
                let capacity = std::fs::read_to_string(p.join("capacity"))
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                let status = std::fs::read_to_string(p.join("status"))
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                return format!("Batterie: {}% ({})", capacity, status);
            }
        }
    }
    "Secteur / Information non disponible".to_string()
}

#[cfg(windows)]
/// Récupère la charge et la capacité mémoire vive (RAM)
pub fn get_memory_info() -> String {
    #[cfg(windows)]
    unsafe {
        let mut mem = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        if GlobalMemoryStatusEx(&mut mem).is_ok() {
            let total_gb = mem.ullTotalPhys as f64 / (1024.0 * 1024.0 * 1024.0);
            let avail_gb = mem.ullAvailPhys as f64 / (1024.0 * 1024.0 * 1024.0);
            let used_gb = total_gb - avail_gb;
            return format!(
                "{:.1} Go utilisés / {:.1} Go totaux ({:.1} Go libres, charge : {}%)",
                used_gb, total_gb, avail_gb, mem.dwMemoryLoad
            );
        }
    }
    "Non disponible".to_string()
}

#[cfg(not(windows))]
pub fn get_memory_info() -> String {
    #[cfg(target_os = "macos")]
    {
        if let Ok(out) = std::process::Command::new("sysctl").arg("hw.memsize").output() {
            let s = String::from_utf8_lossy(&out.stdout);
            if let Some((_, bytes_str)) = s.split_once(':') {
                if let Ok(bytes) = bytes_str.trim().parse::<u64>() {
                    let gb = bytes as f64 / (1024.0 * 1024.0 * 1024.0);
                    return format!("{:.1} Go installés", gb);
                }
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    if let Ok(content) = std::fs::read_to_string("/proc/meminfo") {
        let mut total_kb = 0u64;
        let mut avail_kb = 0u64;
        for line in content.lines() {
            if line.starts_with("MemTotal:") {
                total_kb = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            } else if line.starts_with("MemAvailable:") {
                avail_kb = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            }
        }
        if total_kb > 0 {
            let total_gb = total_kb as f64 / (1024.0 * 1024.0);
            let avail_gb = avail_kb as f64 / (1024.0 * 1024.0);
            let used_gb = total_gb - avail_gb;
            let load = ((used_gb / total_gb) * 100.0) as u32;
            return format!("{:.1} Go utilisés / {:.1} Go totaux ({:.1} Go libres, charge : {}%)", used_gb, total_gb, avail_gb, load);
        }
    }
    "Non disponible".to_string()
}

/// Agrège l'ensemble des capteurs environnementaux en un rapport textuel pour l'IA
pub fn gather_environmental_sensors() -> String {
    let datetime = current_datetime_full_str();
    let username = std::env::var("USERNAME").unwrap_or_else(|_| "Utilisateur".to_string());
    let computer_name = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "PC".to_string());
    let cpus = std::thread::available_parallelism().map_or(1, |p| p.get());
    let os_arch = std::env::consts::ARCH;
    let os_name = std::env::consts::OS;
    let power = get_power_status();
    let ram = get_memory_info();

    let user_profile = std::env::var("USERPROFILE").unwrap_or_default();
    let desktop = PathBuf::from(&user_profile).join("Desktop");
    let downloads = PathBuf::from(&user_profile).join("Downloads");
    let current_dir = std::env::current_dir().unwrap_or_default();

    #[cfg(windows)]
    let display_info = {
        let wa = crate::window_util::get_desktop_work_area();
        let wa_w = wa.right - wa.left;
        let wa_h = wa.bottom - wa.top;
        format!("Espace de travail : {}x{} (pos: {}, {})", wa_w, wa_h, wa.left, wa.top)
    };
    #[cfg(not(windows))]
    let display_info = {
        let desktop_env = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| "Inconnu".to_string());
        let display_server = std::env::var("XDG_SESSION_TYPE")
            .or_else(|_| std::env::var("DISPLAY"))
            .unwrap_or_else(|_| "X11".to_string());
        format!("Serveur d'affichage: {} (Bureau: {})", display_server, desktop_env)
    };

    format!(
        "- Date et heure courantes : {}\n\
         - Machine hôte et utilisateur : \"{}\" sur la machine \"{}\"\n\
         - Système d'exploitation et architecture : {} ({}, {} processeurs logiques)\n\
         - Mémoire vive (RAM) : {}\n\
         - Alimentation : {}\n\
         - Affichage bureau : {}\n\
         - Dossiers clés utilisateur :\n\
           * Bureau : \"{}\"\n\
           * Téléchargements : \"{}\"\n\
           * Dossier de travail courant : \"{}\"",
        datetime, username, computer_name, os_name, os_arch, cpus, ram, power, display_info,
        desktop.display(), downloads.display(), current_dir.display()
    )
}