use eframe::egui::Color32;
use crate::fs_util::read_env_var_from_file;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    LlmProvider,
    Theme,
    Language,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppTheme {
    CyberCyan,
    EmeraldGreen,
    NeonPurple,
    HighContrastYellow,
    HighContrastWhite,
    SoftAmber,
    ColorblindFriendly,
}

#[derive(Debug, Clone)]
pub struct ThemeColors {
    pub clear_color: [f32; 4],
    pub panel_bg: Color32,
    pub control_bg: Color32,
    pub border: Color32,
    pub border_subtle: Color32,
    pub accent: Color32,
    pub accent_text: Color32,
    pub text_primary: Color32,
    pub text_secondary: Color32,
    pub text_user: Color32,
    pub text_agent: Color32,
    pub avatar_ring: [u8; 3],
    pub avatar_core: Color32,
    pub modal_bg: Color32,
    pub tab_active_bg: Color32,
    pub tab_inactive_bg: Color32,
}

impl AppTheme {
    pub const ALL: &'static [Self] = &[
        Self::CyberCyan,
        Self::EmeraldGreen,
        Self::NeonPurple,
        Self::HighContrastYellow,
        Self::HighContrastWhite,
        Self::SoftAmber,
        Self::ColorblindFriendly,
    ];

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::CyberCyan => "Cyber Cyan (Sombre moderne)",
            Self::EmeraldGreen => "Émeraude Menthe (Vert apaisant)",
            Self::NeonPurple => "Améthyste Néon (Violet moderne)",
            Self::HighContrastYellow => "Contraste Élevé Jaune (WCAG AAA)",
            Self::HighContrastWhite => "Contraste Élevé Blanc & Noir",
            Self::SoftAmber => "Ambre Doux (Confort visuel / Nuit)",
            Self::ColorblindFriendly => "Bleu & Jaune (Daltonisme safe)",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Self::CyberCyan => "Thème sombre par défaut avec accents cyan néon.",
            Self::EmeraldGreen => "Teintes vert menthe et forêt sombre relaxantes pour la vue.",
            Self::NeonPurple => "Violet électrique et magenta sombre pour un style néo-synthwave.",
            Self::HighContrastYellow => "Noir pur et jaune d'or à fort contraste. Recommandé pour la malvoyance et la basse vision.",
            Self::HighContrastWhite => "Bordures et textes en blanc éclatant sur fond noir mat.",
            Self::SoftAmber => "Teintes chaudes réduisant la lumière bleue et la fatigue oculaire nocturne.",
            Self::ColorblindFriendly => "Bleu cobalt et jaune haute visibilité optimisés pour deutéranopie et protanopie.",
        }
    }

    pub fn key(&self) -> &'static str {
        match self {
            Self::CyberCyan => "cyber_cyan",
            Self::EmeraldGreen => "emerald_green",
            Self::NeonPurple => "neon_purple",
            Self::HighContrastYellow => "high_contrast_yellow",
            Self::HighContrastWhite => "high_contrast_white",
            Self::SoftAmber => "soft_amber",
            Self::ColorblindFriendly => "colorblind_friendly",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key.trim().to_lowercase().as_str() {
            "cyber_cyan" | "cyan" | "default" | "sombre" => Some(Self::CyberCyan),
            "emerald_green" | "emerald" | "green" | "vert" | "emeraude" | "émeraude" => Some(Self::EmeraldGreen),
            "neon_purple" | "purple" | "violet" | "amethyste" | "améthyste" => Some(Self::NeonPurple),
            "high_contrast_yellow" | "yellow" | "jaune" | "wcag" => Some(Self::HighContrastYellow),
            "high_contrast_white" | "white" | "blanc" => Some(Self::HighContrastWhite),
            "soft_amber" | "amber" | "ambre" | "nuit" => Some(Self::SoftAmber),
            "colorblind_friendly" | "colorblind" | "daltonisme" => Some(Self::ColorblindFriendly),
            _ => None,
        }
    }

    pub fn next(&self) -> Self {
        match self {
            Self::CyberCyan => Self::EmeraldGreen,
            Self::EmeraldGreen => Self::NeonPurple,
            Self::NeonPurple => Self::HighContrastYellow,
            Self::HighContrastYellow => Self::HighContrastWhite,
            Self::HighContrastWhite => Self::SoftAmber,
            Self::SoftAmber => Self::ColorblindFriendly,
            Self::ColorblindFriendly => Self::CyberCyan,
        }
    }

    pub fn resolve_theme() -> Self {
        if let Ok(val) = std::env::var("THEME") {
            if let Some(t) = Self::from_key(&val) {
                return t;
            }
        }
        if let Some(val) = read_env_var_from_file(".env", "THEME") {
            if let Some(t) = Self::from_key(&val) {
                return t;
            }
        }
        Self::CyberCyan
    }

    pub fn colors(&self) -> ThemeColors {
        match self {
            Self::CyberCyan => ThemeColors {
                clear_color: [0.0, 0.0, 0.0, 0.75],
                panel_bg: Color32::from_rgba_unmultiplied(0, 0, 0, 180),
                control_bg: Color32::from_rgb(18, 20, 26),
                border: Color32::from_rgba_unmultiplied(0, 180, 255, 75),
                border_subtle: Color32::from_rgba_unmultiplied(0, 180, 255, 45),
                accent: Color32::from_rgb(0, 200, 255),
                accent_text: Color32::from_rgb(220, 240, 255),
                text_primary: Color32::WHITE,
                text_secondary: Color32::from_rgb(170, 185, 205),
                text_user: Color32::from_rgb(255, 230, 150),
                text_agent: Color32::WHITE,
                avatar_ring: [0, 180, 255],
                avatar_core: Color32::from_rgba_unmultiplied(0, 180, 255, 220),
                modal_bg: Color32::from_rgb(12, 15, 20),
                tab_active_bg: Color32::from_rgba_unmultiplied(0, 180, 255, 55),
                tab_inactive_bg: Color32::from_rgba_unmultiplied(255, 255, 255, 10),
            },
            Self::EmeraldGreen => ThemeColors {
                clear_color: [0.03, 0.07, 0.05, 0.82],
                panel_bg: Color32::from_rgb(8, 20, 15),
                control_bg: Color32::from_rgb(12, 28, 20),
                border: Color32::from_rgb(0, 210, 140),
                border_subtle: Color32::from_rgba_unmultiplied(0, 210, 140, 55),
                accent: Color32::from_rgb(0, 230, 150),
                accent_text: Color32::from_rgb(220, 255, 240),
                text_primary: Color32::WHITE,
                text_secondary: Color32::from_rgb(160, 210, 190),
                text_user: Color32::from_rgb(160, 255, 210),
                text_agent: Color32::WHITE,
                avatar_ring: [0, 225, 145],
                avatar_core: Color32::from_rgb(0, 225, 145),
                modal_bg: Color32::from_rgb(10, 20, 16),
                tab_active_bg: Color32::from_rgba_unmultiplied(0, 220, 140, 60),
                tab_inactive_bg: Color32::from_rgba_unmultiplied(255, 255, 255, 10),
            },
            Self::NeonPurple => ThemeColors {
                clear_color: [0.06, 0.04, 0.09, 0.82],
                panel_bg: Color32::from_rgb(18, 12, 26),
                control_bg: Color32::from_rgb(26, 16, 38),
                border: Color32::from_rgb(180, 95, 255),
                border_subtle: Color32::from_rgba_unmultiplied(180, 95, 255, 55),
                accent: Color32::from_rgb(190, 105, 255),
                accent_text: Color32::from_rgb(245, 230, 255),
                text_primary: Color32::WHITE,
                text_secondary: Color32::from_rgb(195, 175, 225),
                text_user: Color32::from_rgb(255, 185, 245),
                text_agent: Color32::WHITE,
                avatar_ring: [190, 105, 255],
                avatar_core: Color32::from_rgb(190, 105, 255),
                modal_bg: Color32::from_rgb(16, 12, 24),
                tab_active_bg: Color32::from_rgba_unmultiplied(190, 105, 255, 60),
                tab_inactive_bg: Color32::from_rgba_unmultiplied(255, 255, 255, 10),
            },
            Self::HighContrastYellow => ThemeColors {
                clear_color: [0.0, 0.0, 0.0, 0.95],
                panel_bg: Color32::from_rgb(0, 0, 0),
                control_bg: Color32::from_rgb(10, 10, 10),
                border: Color32::from_rgb(255, 234, 0),
                border_subtle: Color32::from_rgba_unmultiplied(255, 234, 0, 120),
                accent: Color32::from_rgb(255, 234, 0),
                accent_text: Color32::from_rgb(255, 255, 0),
                text_primary: Color32::WHITE,
                text_secondary: Color32::from_rgb(255, 234, 0),
                text_user: Color32::from_rgb(255, 234, 0),
                text_agent: Color32::WHITE,
                avatar_ring: [255, 234, 0],
                avatar_core: Color32::from_rgb(255, 234, 0),
                modal_bg: Color32::from_rgb(0, 0, 0),
                tab_active_bg: Color32::from_rgba_unmultiplied(255, 234, 0, 70),
                tab_inactive_bg: Color32::from_rgb(20, 20, 20),
            },
            Self::HighContrastWhite => ThemeColors {
                clear_color: [0.0, 0.0, 0.0, 0.95],
                panel_bg: Color32::from_rgb(0, 0, 0),
                control_bg: Color32::from_rgb(12, 12, 12),
                border: Color32::WHITE,
                border_subtle: Color32::from_rgba_unmultiplied(255, 255, 255, 120),
                accent: Color32::WHITE,
                accent_text: Color32::WHITE,
                text_primary: Color32::WHITE,
                text_secondary: Color32::from_rgb(210, 210, 210),
                text_user: Color32::WHITE,
                text_agent: Color32::WHITE,
                avatar_ring: [255, 255, 255],
                avatar_core: Color32::WHITE,
                modal_bg: Color32::from_rgb(5, 5, 5),
                tab_active_bg: Color32::from_rgba_unmultiplied(255, 255, 255, 65),
                tab_inactive_bg: Color32::from_rgb(25, 25, 25),
            },
            Self::SoftAmber => ThemeColors {
                clear_color: [0.08, 0.06, 0.04, 0.85],
                panel_bg: Color32::from_rgb(24, 18, 14),
                control_bg: Color32::from_rgb(32, 22, 16),
                border: Color32::from_rgb(220, 140, 45),
                border_subtle: Color32::from_rgba_unmultiplied(220, 140, 45, 60),
                accent: Color32::from_rgb(230, 145, 45),
                accent_text: Color32::from_rgb(255, 225, 170),
                text_primary: Color32::from_rgb(250, 240, 230),
                text_secondary: Color32::from_rgb(210, 185, 155),
                text_user: Color32::from_rgb(255, 200, 110),
                text_agent: Color32::from_rgb(255, 245, 235),
                avatar_ring: [220, 140, 45],
                avatar_core: Color32::from_rgb(220, 140, 45),
                modal_bg: Color32::from_rgb(22, 16, 12),
                tab_active_bg: Color32::from_rgba_unmultiplied(220, 140, 45, 60),
                tab_inactive_bg: Color32::from_rgba_unmultiplied(255, 255, 255, 12),
            },
            Self::ColorblindFriendly => ThemeColors {
                clear_color: [0.03, 0.05, 0.10, 0.82],
                panel_bg: Color32::from_rgb(10, 18, 28),
                control_bg: Color32::from_rgb(14, 24, 38),
                border: Color32::from_rgb(40, 135, 255),
                border_subtle: Color32::from_rgba_unmultiplied(40, 135, 255, 75),
                accent: Color32::from_rgb(40, 135, 255),
                accent_text: Color32::WHITE,
                text_primary: Color32::from_rgb(240, 245, 255),
                text_secondary: Color32::from_rgb(160, 195, 235),
                text_user: Color32::from_rgb(255, 235, 20),
                text_agent: Color32::from_rgb(230, 245, 255),
                avatar_ring: [40, 135, 255],
                avatar_core: Color32::from_rgb(255, 235, 20),
                modal_bg: Color32::from_rgb(10, 16, 26),
                tab_active_bg: Color32::from_rgba_unmultiplied(40, 135, 255, 65),
                tab_inactive_bg: Color32::from_rgba_unmultiplied(255, 255, 255, 15),
            },
        }
    }
}