use std::path::PathBuf;

#[cfg(windows)]
#[repr(C)]
struct SystemTimeWin {
    year: u16,
    month: u16,
    day_of_week: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    milliseconds: u16,
}

#[cfg(windows)]
#[repr(C)]
struct SystemPowerStatus {
    ac_line_status: u8,
    battery_flag: u8,
    battery_life_percent: u8,
    system_status_flag: u8,
    battery_life_time: u32,
    battery_full_life_time: u32,
}

#[cfg(windows)]
#[repr(C)]
struct MemoryStatusEx {
    dw_length: u32,
    dw_memory_load: u32,
    ull_total_phys: u64,
    ull_avail_phys: u64,
    ull_total_page_file: u64,
    ull_avail_page_file: u64,
    ull_total_virtual: u64,
    ull_avail_virtual: u64,
    ull_avail_extended_virtual: u64,
}

#[cfg(windows)]
extern "system" {
    fn GetLocalTime(st: *mut SystemTimeWin);
    fn GetSystemPowerStatus(lp: *mut SystemPowerStatus) -> i32;
    fn GlobalMemoryStatusEx(lp: *mut MemoryStatusEx) -> i32;
}

/// Retourne la date et l'heure courante détaillée en français
pub fn current_datetime_full_str() -> String {
    #[cfg(windows)]
    unsafe {
        let mut st = SystemTimeWin {
            year: 0, month: 0, day_of_week: 0, day: 0,
            hour: 0, minute: 0, second: 0, milliseconds: 0,
        };
        GetLocalTime(&mut st);

        const DAYS: [&str; 7] = ["Dimanche", "Lundi", "Mardi", "Mercredi", "Jeudi", "Vendredi", "Samedi"];
        const MONTHS: [&str; 13] = [
            "", "janvier", "février", "mars", "avril", "mai", "juin",
            "juillet", "août", "septembre", "octobre", "novembre", "décembre",
        ];

        let day_name = DAYS.get(st.day_of_week as usize).unwrap_or(&"");
        let month_name = MONTHS.get(st.month as usize).unwrap_or(&"");

        format!(
            "{} {} {} {}, {:02}:{:02}:{:02}",
            day_name, st.day, month_name, st.year, st.hour, st.minute, st.second
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
        let mut sps = std::mem::zeroed::<SystemPowerStatus>();
        if GetSystemPowerStatus(&mut sps) != 0 {
            let ac = match sps.ac_line_status {
                0 => "Sur batterie",
                1 => "Branché sur secteur",
                _ => "Alimentation inconnue",
            };
            let percent = if sps.battery_life_percent <= 100 {
                format!("{}%", sps.battery_life_percent)
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
        let mut mem = std::mem::zeroed::<MemoryStatusEx>();
        mem.dw_length = std::mem::size_of::<MemoryStatusEx>() as u32;
        if GlobalMemoryStatusEx(&mut mem) != 0 {
            let total_gb = mem.ull_total_phys as f64 / (1024.0 * 1024.0 * 1024.0);
            let avail_gb = mem.ull_avail_phys as f64 / (1024.0 * 1024.0 * 1024.0);
            let used_gb = total_gb - avail_gb;
            return format!(
                "{:.1} Go utilisés / {:.1} Go totaux ({:.1} Go libres, charge : {}%)",
                used_gb, total_gb, avail_gb, mem.dw_memory_load
            );
        }
    }
    "Non disponible".to_string()
}

#[cfg(not(windows))]
pub fn get_memory_info() -> String {
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