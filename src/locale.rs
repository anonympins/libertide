use crate::fs_util::read_env_var_from_file;

/// Détecte la langue principale du système d'exploitation de l'utilisateur
pub fn get_system_locale_language() -> (String, String) {
    if let Ok(l) = std::env::var("APP_LANG").or_else(|_| read_env_var_from_file(".env", "APP_LANG").ok_or(())) {
        let l_clean = l.trim().to_lowercase();
        if l_clean != "auto" && !l_clean.is_empty() {
            let name = match l_clean.as_str() {
                "fr" => "français".to_string(),
                "en" => "English".to_string(),
                "es" => "español".to_string(),
                "de" => "Deutsch".to_string(),
                "it" => "italiano".to_string(),
                "pt" => "português".to_string(),
                "ja" => "日本語 (Japanese)".to_string(),
                "zh" => "中文 (Chinese)".to_string(),
                "ru" => "русский".to_string(),
                _ => l_clean.clone(),
            };
            return (l_clean, name);
        }
    }
    #[cfg(windows)]
    {
        extern "system" {
            fn GetUserDefaultLocaleName(lpLocaleName: *mut u16, cchLocaleName: i32) -> i32;
        }
        let mut buf = [0u16; 85];
        let len = unsafe { GetUserDefaultLocaleName(buf.as_mut_ptr(), buf.len() as i32) };
        if len > 0 {
            let locale = String::from_utf16_lossy(&buf[..(len - 1) as usize]).to_lowercase();
            let lang_prefix = locale.split('-').next().unwrap_or(&locale);
            let name: String = match lang_prefix {
                "fr" => "français".to_string(),
                "en" => "English".to_string(),
                "es" => "español".to_string(),
                "de" => "Deutsch".to_string(),
                "it" => "italiano".to_string(),
                "pt" => "português".to_string(),
                "ja" => "日本語 (Japanese)".to_string(),
                "zh" => "中文 (Chinese)".to_string(),
                "ru" => "русский".to_string(),
                other => other.to_string(),
            };
            return (locale, name);
        }
        return ("en".to_string(), "English".to_string());
    }
    #[cfg(not(windows))]
    {
        let env_lang = std::env::var("LC_ALL").or_else(|_| std::env::var("LC_MESSAGES")).or_else(|_| std::env::var("LANG")).unwrap_or_else(|_| "fr_FR".to_string()).to_lowercase();
        let lang_prefix = env_lang.split(['_', '.']).next().unwrap_or("fr");
        let name: String = match lang_prefix {
            "fr" => "français".to_string(),
            "en" => "English".to_string(),
            "es" => "español".to_string(),
            "de" => "Deutsch".to_string(),
            "it" => "italiano".to_string(),
            "pt" => "português".to_string(),
            "ja" => "日本語 (Japanese)".to_string(),
            "zh" => "中文 (Chinese)".to_string(),
            "ru" => "русский".to_string(),
            other => other.to_string(),
        };
        return (env_lang, name);
    }
}