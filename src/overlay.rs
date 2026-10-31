use eframe::egui;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use crate::{get_system_locale_language, parse_llm_response_text, send_llm_chat_completion};
use crate::automation::*;
use crate::fs_util::{read_env_var_from_file, write_env_var_to_file};
use crate::theme::ThemeColors;
use crate::types::*;
use crate::window_util::*;

#[cfg(windows)]
use windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY;

#[derive(Clone, Copy, Debug)]
pub enum Earcon {
    MicOpen,
    MicClose,
    Success,
    Emergency,
    Error,
    Navigation,
    Warning,
}

pub fn play_earcon(earcon: Earcon) {
    #[cfg(windows)]
    std::thread::spawn(move || unsafe {
        extern "system" {
            fn Beep(dwFreq: u32, dwDuration: u32) -> i32;
        }
        match earcon {
            Earcon::MicOpen => {
                Beep(600, 40);
                Beep(900, 50);
            }
            Earcon::MicClose => {
                Beep(750, 40);
                Beep(500, 50);
            }
            Earcon::Success => {
                Beep(1000, 50);
            }
            Earcon::Emergency => {
                for _ in 0..3 {
                    Beep(1200, 60);
                    Beep(800, 60);
                }
            }
            Earcon::Error => {
                Beep(450, 70);
                Beep(320, 100);
            }
            Earcon::Navigation => {
                Beep(1600, 18);
            }
            Earcon::Warning => {
                Beep(850, 50);
                Beep(850, 50);
            }
        }
    });
    #[cfg(target_os = "macos")]
    std::thread::spawn(move || {
        let sound_name = match earcon {
            Earcon::MicOpen => "Pop.aiff",
            Earcon::MicClose => "Tink.aiff",
            Earcon::Success => "Glass.aiff",
            Earcon::Emergency => "Sosumi.aiff",
            Earcon::Error => "Basso.aiff",
            Earcon::Navigation => "Tink.aiff",
            Earcon::Warning => "Frog.aiff",
        };
        let sound_path = format!("/System/Library/Sounds/{}", sound_name);
        if std::path::Path::new(&sound_path).exists() {
            let _ = std::process::Command::new("afplay").arg(&sound_path).status();
        } else {
            print!("\x07");
            use std::io::Write;
            let _ = std::io::stdout().flush();
        }
    });
    #[cfg(all(not(windows), not(target_os = "macos")))]
    std::thread::spawn(move || {
        let sound_id = match earcon {
            Earcon::MicOpen => "audio-volume-change",
            Earcon::MicClose => "dialog-information",
            Earcon::Success => "complete",
            Earcon::Emergency => "dialog-warning",
            Earcon::Error => "dialog-error",
            Earcon::Navigation => "button-pressed",
            Earcon::Warning => "dialog-warning",
        };
        let played = std::process::Command::new("canberra-gtk-play")
            .args(["-i", sound_id])
            .status()
            .map_or(false, |s| s.success());
        if !played {
            let _ = std::process::Command::new("paplay")
                .arg("/usr/share/sounds/freedesktop/stereo/complete.oga")
                .status();
            print!("\x07");
            use std::io::Write;
            let _ = std::io::stdout().flush();
        }
    });
}

static TTS_AUDIO_LEVEL: AtomicU32 = AtomicU32::new(0);
static MIC_AUDIO_LEVEL: AtomicU32 = AtomicU32::new(0);

pub fn set_tts_audio_level(level: f32) {
    TTS_AUDIO_LEVEL.store(level.to_bits(), Ordering::Relaxed);
}

pub fn get_tts_audio_level() -> f32 {
    f32::from_bits(TTS_AUDIO_LEVEL.load(Ordering::Relaxed))
}

pub fn set_mic_audio_level(level: f32) {
    MIC_AUDIO_LEVEL.store(level.to_bits(), Ordering::Relaxed);
}

pub fn get_mic_audio_level() -> f32 {
    f32::from_bits(MIC_AUDIO_LEVEL.load(Ordering::Relaxed))
}

// Constantes ergonomiques d'accessibilité motrice (WCAG 2.2 AAA Target Size)
pub const ACCESSIBILITY_BTN_SIZE: egui::Vec2 = egui::vec2(50.0, 50.0);
pub const ACCESSIBILITY_ICON_FONT: egui::FontId = egui::FontId::proportional(24.0);
pub const ACCESSIBILITY_SUB_FONT: egui::FontId = egui::FontId::proportional(10.0);
pub const ACCESSIBILITY_ITEM_SPACING: f32 = 8.0;

pub struct DwellState {
    pub hovered_id: Option<egui::Id>,
    pub hover_start: Option<Instant>,
    pub dwell_duration: Duration,
    pub activation_delay: Duration,
    pub cooldown_until: Option<Instant>,
    pub last_triggered_id: Option<egui::Id>,
    pub last_triggered_time: Option<Instant>,
    pub unhovered_since: Option<Instant>,
    pub locked_ids: std::collections::HashMap<egui::Id, Instant>,
}

impl Default for DwellState {
    fn default() -> Self {
        Self {
            hovered_id: None,
            hover_start: None,
            dwell_duration: Duration::ZERO,
            activation_delay: Duration::from_millis(200),
            cooldown_until: None,
            last_triggered_id: None,
            last_triggered_time: None,
            unhovered_since: None,
            locked_ids: std::collections::HashMap::new(),
        }
    }
}

impl DwellState {
    pub fn new(millis: u64) -> Self {
        Self::new_with_delay(millis, 200)
    }

    pub fn new_with_delay(millis: u64, delay_ms: u64) -> Self {
        Self {
            hovered_id: None,
            hover_start: None,
            dwell_duration: Duration::from_millis(millis),
            activation_delay: Duration::from_millis(delay_ms),
            cooldown_until: None,
            last_triggered_id: None,
            last_triggered_time: None,
            unhovered_since: None,
            locked_ids: std::collections::HashMap::new(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        !self.dwell_duration.is_zero()
    }

    pub fn effective_activation_delay(&self) -> Duration {
        if self.dwell_duration.is_zero() {
            Duration::ZERO
        } else {
            self.activation_delay.min(self.dwell_duration / 2)
        }
    }

    pub fn lock_id(&mut self, id: egui::Id, duration: Duration) {
        let until = Instant::now() + duration;
        self.locked_ids.insert(id, until);
        if self.hovered_id == Some(id) {
            self.hovered_id = None;
            self.hover_start = None;
        }
    }

    pub fn is_locked(&self, id: egui::Id) -> bool {
        if let Some(&until) = self.locked_ids.get(&id) {
            if Instant::now() < until {
                return true;
            }
        }
        false
    }

    pub fn reset_hover(&mut self, id: egui::Id) {
        if self.hovered_id == Some(id) {
            self.hovered_id = None;
            self.hover_start = None;
        }
    }

    pub fn get_progress(&self, id: egui::Id) -> Option<f32> {
        if !self.is_enabled() {
            return None;
        }
        if self.last_triggered_id == Some(id) || self.is_locked(id) {
            return None;
        }
        if self.hovered_id == Some(id) {
            if let Some(start) = self.hover_start {
                let elapsed = Instant::now().duration_since(start);
                let delay = self.effective_activation_delay();
                if elapsed < delay {
                    return None;
                }
                let active_elapsed = elapsed - delay;
                let active_total = self.dwell_duration.saturating_sub(delay);
                if active_total.is_zero() {
                    return Some(1.0);
                }
                let ratio = (active_elapsed.as_secs_f32() / active_total.as_secs_f32()).clamp(0.0, 1.0);
                return Some(ratio);
            }
        }
        None
    }

    pub fn update(&mut self, id: egui::Id, is_hovered: bool) -> (bool, f32) {
        if !self.is_enabled() {
            return (false, 0.0);
        }
        let now = Instant::now();

        // 1. Purge des verrous expirés
        self.locked_ids.retain(|_, until| *until > now);

        // 2. Si l'élément est sous verrouillage étendu
        if self.is_locked(id) {
            if self.hovered_id == Some(id) {
                self.hovered_id = None;
                self.hover_start = None;
            }
            return (false, 0.0);
        }

        // 3. Protection œil fixe : l'élément vient d'être déclenché
        if self.last_triggered_id == Some(id) {
            if is_hovered {
                self.unhovered_since = None;
                self.hovered_id = None;
                self.hover_start = None;
                return (false, 0.0);
            } else {
                if self.unhovered_since.is_none() {
                    self.unhovered_since = Some(now);
                }
                let unhovered_time = self.unhovered_since.unwrap();
                let unhovered_long_enough = now.duration_since(unhovered_time) >= Duration::from_millis(600);
                let refractory_passed = self.last_triggered_time.map_or(true, |t| now.duration_since(t) >= Duration::from_millis(1500));

                if unhovered_long_enough && refractory_passed {
                    self.last_triggered_id = None;
                    self.last_triggered_time = None;
                    self.unhovered_since = None;
                }
                return (false, 0.0);
            }
        }

        if is_hovered && self.last_triggered_id.is_some() && self.last_triggered_id != Some(id) {
            if self.last_triggered_time.map_or(true, |t| now.duration_since(t) >= Duration::from_millis(500)) {
                self.last_triggered_id = None;
                self.last_triggered_time = None;
                self.unhovered_since = None;
            }
        }

        if let Some(cooldown) = self.cooldown_until {
            if now < cooldown {
                if !is_hovered && self.hovered_id == Some(id) {
                    self.hovered_id = None;
                    self.hover_start = None;
                }
                return (false, 0.0);
            } else {
                self.cooldown_until = None;
            }
        }

        if !is_hovered {
            if self.hovered_id == Some(id) {
                self.hovered_id = None;
                self.hover_start = None;
            }
            return (false, 0.0);
        }

        if self.hovered_id != Some(id) {
            self.hovered_id = Some(id);
            self.hover_start = Some(now);
            return (false, 0.0);
        }

        if let Some(start) = self.hover_start {
            let elapsed = now.duration_since(start);
            let delay = self.effective_activation_delay();

            if elapsed < delay {
                return (false, 0.0);
            }

            let active_elapsed = elapsed - delay;
            let active_total = self.dwell_duration.saturating_sub(delay);
            let ratio = if active_total.is_zero() {
                1.0
            } else {
                (active_elapsed.as_secs_f32() / active_total.as_secs_f32()).clamp(0.0, 1.0)
            };

            if elapsed >= self.dwell_duration {
                self.hover_start = None;
                self.hovered_id = None;
                self.last_triggered_id = Some(id);
                self.last_triggered_time = Some(now);
                self.unhovered_since = None;
                self.cooldown_until = Some(now + Duration::from_millis(500));
                return (true, 1.0);
            }
            return (false, ratio);
        }

        (false, 0.0)
    }
}

pub fn draw_avatar(
    ui: &mut egui::Ui,
    center: egui::Pos2,
    base_radius: f32,
    time: f64,
    status: AgentStatus,
    smoothed_audio_level: f32,
    theme: &ThemeColors,
) {
    let painter = ui.painter();

    let pulse_speed = match status {
        AgentStatus::Idle => 1.6,
        AgentStatus::Thinking => 4.2,
        AgentStatus::Speaking | AgentStatus::Listening => 2.0 + 4.5 * (smoothed_audio_level as f64),
        AgentStatus::EmergencyStopped => 0.0,
    };

    let wave = if status == AgentStatus::Speaking || status == AgentStatus::Listening {
        smoothed_audio_level
    } else {
        ((time * pulse_speed).sin() as f32).max(0.0)
    };

    let (ring_base_color, core_color) = if status == AgentStatus::Listening {
        ([255, 65, 80], egui::Color32::from_rgb(255, 50, 70))
    } else {
        (theme.avatar_ring, theme.avatar_core)
    };

    let num_bars = 16;
    let activity_level = match status {
        AgentStatus::Speaking | AgentStatus::Listening => smoothed_audio_level,
        AgentStatus::Thinking => 0.5f32,
        AgentStatus::Idle => 0.18f32,
        AgentStatus::EmergencyStopped => 0.0f32,
    };

    for i in 0..num_bars {
        let angle = (i as f32 / num_bars as f32) * std::f32::consts::TAU + (time as f32 * 0.7);
        let harmonic1 = ((time * (7.5 + (i % 4) as f64 * 2.0) + (i as f64 * 1.4)).sin() as f32).abs();
        let harmonic2 = ((time * 13.0 + (i as f64 * 2.2)).cos() as f32).abs();
        let bar_len = (1.5 + (harmonic1 * 5.5 + harmonic2 * 4.0) * activity_level).clamp(1.5, 11.0);
        let dir = egui::vec2(angle.cos(), angle.sin());
        let start = center + dir * (base_radius + 3.0);
        let end = center + dir * (base_radius + 3.0 + bar_len);
        let alpha = ((40.0 + 195.0 * activity_level * (harmonic1 * 0.6 + 0.4)) as u8).min(235);
        let bar_color = egui::Color32::from_rgba_unmultiplied(ring_base_color[0], ring_base_color[1], ring_base_color[2], alpha);
        painter.line_segment([start, end], egui::Stroke::new(1.3, bar_color));
    }

    let ring_alpha_mult = if status == AgentStatus::Speaking || status == AgentStatus::Listening {
        0.2 + 0.8 * smoothed_audio_level
    } else {
        1.0
    };

    let rings = [
        (base_radius + 26.0 + (wave * 6.0), egui::Color32::from_rgba_unmultiplied(ring_base_color[0], ring_base_color[1], ring_base_color[2], (35.0 * ring_alpha_mult) as u8), 1.5),
        (base_radius + 18.0 + (wave * 4.0), egui::Color32::from_rgba_unmultiplied(ring_base_color[0], ring_base_color[1], ring_base_color[2], (60.0 * ring_alpha_mult) as u8), 1.8),
        (base_radius + 10.0 + (wave * 2.5), egui::Color32::from_rgba_unmultiplied(ring_base_color[0], ring_base_color[1], ring_base_color[2], (110.0 * ring_alpha_mult) as u8), 2.0),
        (base_radius + 2.0, egui::Color32::from_rgba_unmultiplied(ring_base_color[0].saturating_add(30), ring_base_color[1].saturating_add(30), ring_base_color[2], (180.0 * ring_alpha_mult) as u8), 2.2),
    ];

    for (radius, color, stroke_width) in rings {
        painter.circle_stroke(center, radius, egui::Stroke::new(stroke_width, color));
    }

    painter.circle_filled(
        center,
        base_radius * (0.40 + 0.12 * activity_level),
        core_color,
    );
    painter.circle_filled(center, base_radius * (0.18 + 0.06 * activity_level), egui::Color32::WHITE);
}

pub fn draw_mini_equalizer(ui: &mut egui::Ui, time: f64, status: AgentStatus, smoothed_audio_level: f32, theme: &ThemeColors) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(28.0, 12.0), egui::Sense::hover());
    let painter = ui.painter();

    let num_bars = 5;
    let bar_width = 3.0;
    let spacing = 2.5;

    let (base_color, activity_mult) = match status {
        AgentStatus::Speaking => (theme.accent, smoothed_audio_level),
        AgentStatus::Listening => (egui::Color32::from_rgb(255, 75, 90), smoothed_audio_level),
        AgentStatus::Thinking => (egui::Color32::from_rgb(180, 120, 255), 0.55f32),
        AgentStatus::Idle => (theme.border, 0.15f32),
        AgentStatus::EmergencyStopped => (egui::Color32::from_rgb(220, 50, 50), 0.0f32),
    };

    for i in 0..num_bars {
        let x = rect.min.x + (i as f32) * (bar_width + spacing);
        let h = if status == AgentStatus::EmergencyStopped {
            2.0
        } else if status == AgentStatus::Speaking || status == AgentStatus::Listening {
            let freq_mod = ((time * (9.0 + (i as f64) * 3.2) + (i as f64 * 1.5)).sin() as f32).abs();
            let bar_scale = 0.35 + 0.65 * freq_mod;
            (2.5 + bar_scale * activity_mult * (rect.height() - 2.5)).clamp(2.5, rect.height())
        } else {
            let wave1 = ((time * 8.0 + (i as f64 * 1.6)).sin() as f32).abs();
            let wave2 = ((time * 15.0 + (i as f64 * 2.7)).cos() as f32).abs();
            let combined = (wave1 * 0.6 + wave2 * 0.4) * activity_mult;
            (3.0 + combined * (rect.height() - 3.0)).clamp(2.5, rect.height())
        };
        let bar_rect = egui::Rect::from_min_size(egui::pos2(x, rect.max.y - h), egui::vec2(bar_width, h));
        painter.rect_filled(bar_rect, egui::CornerRadius::same(1), base_color);
    }
}

pub fn normalize_for_command_match(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<&str>>()
        .join(" ")
}

pub fn is_emergency_stop_command(text: &str) -> bool {
    let norm = normalize_for_command_match(text);
    if norm == "stop" || norm == "stopp" || norm == "стоп" || norm == "停止" || norm == "ストップ" {
        return true;
    }
    const TRIGGERS: &[&str] = &[
        "arret d urgence", "arrêt d urgence", "arrete tout", "arrête tout", "stop tout", "urgence",
        "emergency stop", "stop all", "stop everything", "abort", "halt", "freeze",
        "parada de emergencia", "deten todo", "detén todo", "para todo", "emergencia", "alto ahi",
        "not aus", "notaus", "alles anhalten", "alles stoppen", "notfall", "abbrechen",
        "arresto d emergenza", "ferma tutto", "blocca tutto", "emergenza", "annulla tutto",
        "parada de emergencia", "parada de emergência", "pare tudo", "parar tudo", "emergencia", "abortar",
        "экстренная остановка", "останови все", "останови всё", "стоп все", "стоп всё", "аварийная остановка",
        "紧急停止", "全部停止", "立即停止", "停止运行", "中止",
        "緊急停止", "すべて停止", "すべて止めて", "中止", "直ちに停止",
    ];
    TRIGGERS.iter().any(|&trig| norm.contains(trig) || text.contains(trig))
}

pub fn is_stop_dictation_command(text: &str) -> bool {
    let norm = normalize_for_command_match(text);
    const TRIGGERS: &[&str] = &[
        "arrete la dictee", "arrête la dictée", "stop dictee", "stop dictée", "quitter la dictee", "quitter la dictée",
        "fin de la dictee", "fin de la dictée", "termine la dictee", "termine la dictée", "desactiver dictee", "désactiver dictée",
        "stop dictation", "exit dictation", "quit dictation", "end dictation", "finish dictation", "disable dictation",
        "detener dictado", "parar dictado", "salir del dictado", "terminar dictado", "desactivar dictado", "fin del dictado",
        "diktat beenden", "diktat stoppen", "diktat anhalten", "diktat abbrechen", "diktat deaktivieren",
        "interrompi dettatura", "ferma dettatura", "esci dalla dettatura", "termina dettatura", "disattiva dettatura",
        "parar ditado", "encerrar ditado", "sair do ditado", "terminar ditado", "desativar ditado", "fim do ditado",
        "остановить диктовку", "завершить диктовку", "выход из диктовки", "стоп диктовка", "отключить диктовку",
        "停止听写", "退出听写", "结束听写", "关闭听写",
        "ディクテーション終了", "ディクテーション停止", "入力終了", "音声入力停止",
    ];
    TRIGGERS.iter().any(|&trig| norm.contains(trig) || text.contains(trig))
}

pub fn is_start_dictation_command(text: &str) -> bool {
    let norm = normalize_for_command_match(text);
    const TRIGGERS: &[&str] = &[
        "active la dictee", "active la dictée", "activer la dictee", "activer la dictée",
        "lance la dictee", "lance la dictée", "lancer la dictee", "lancer la dictée",
        "mode dictee", "mode dictée", "demarre la dictee", "démarre la dictée",
        "demarrer la dictee", "démarrer la dictée", "passer en dictee", "passer en dictée",
        "start dictation", "enable dictation", "activate dictation", "dictation mode", "begin dictation",
        "activar dictado", "iniciar dictado", "modo dictado", "comenzar dictado",
        "diktat aktivieren", "diktat starten", "diktatmodus",
        "attiva dettatura", "avvia dettatura", "modalita dettatura", "modalità dettatura",
        "ativar ditado", "iniciar ditado", "modo ditado",
        "включить диктовку", "начать диктовку", "режим диктовки",
        "开启听写", "启动听写", "听写模式",
        "ディクテーション開始", "音声入力開始", "ディクテーションモード",
    ];
    TRIGGERS.iter().any(|&trig| norm.contains(trig) || text.contains(trig))
}

pub fn is_switch_access_command(text: &str) -> bool {
    let norm = normalize_for_command_match(text);
    const TRIGGERS: &[&str] = &[
        "mode contacteur", "switch access", "mode survol", "survol contacteur", "contacteur", "survol",
        "switch mode", "hover mode", "switch navigation",
        "acceso por conmutador", "modo conmutador", "modo sobrevuelo", "conmutador",
        "schalterzugriff", "schalter modus", "hover modus", "schalter",
        "accesso a interruttori", "modalita cursore", "modalita interruttore", "interruttore",
        "acesso por interruptor", "modo comutador", "comutador", "interruptor", "modo de sobrevoo",
        "переключатель", "режим переключателя", "режим наведения",
        "切换控制", "开关控制", "悬停控制", "悬停模式",
        "スイッチアクセス", "スイッチ操作", "ホバーモード",
    ];
    TRIGGERS.iter().any(|&trig| norm.contains(trig) || text.contains(trig))
}

pub fn is_dismiss_hover_command(text: &str) -> bool {
    let norm = normalize_for_command_match(text);
    const TRIGGERS: &[&str] = &[
        "non merci", "pas maintenant", "plus tard", "refuser survol", "pas de survol",
        "no thanks", "not now", "later", "no thank you", "dismiss hover",
        "no gracias", "ahora no", "mas tarde", "más tarde",
        "nein danke", "nicht jetzt", "spater", "später",
        "no grazie", "non adesso", "piu tardi", "più tardi",
        "nao obrigado", "não obrigado", "agora nao", "agora não", "mais tarde",
        "нет спасибо", "не сейчас", "позже",
        "不用了", "谢谢不用", "稍后", "暂不", "不需要",
        "結構です", "あとで", "今はいい",
    ];
    if TRIGGERS.iter().any(|&trig| norm.contains(trig) || text.contains(trig)) {
        return true;
    }
    if !crate::is_hover_nav_asked() && (norm == "non" || norm == "no" || norm == "nein" || norm == "nao" || norm == "não" || norm == "нет" || norm == "いいえ") {
        return true;
    }
    false
}

pub fn is_hover_nav_asked() -> bool {
    std::env::var("HOVER_NAV_ASKED")
        .ok()
        .or_else(|| read_env_var_from_file(".env", "HOVER_NAV_ASKED"))
        .map(|s| {
            let t = s.trim();
            t.eq_ignore_ascii_case("true") || t == "1"
        })
        .unwrap_or(false)
}

pub fn mark_hover_nav_asked() {
    std::env::set_var("HOVER_NAV_ASKED", "true");
    let _ = write_env_var_to_file(".env", "HOVER_NAV_ASKED", "true");
}

pub fn is_hide_command(text: &str) -> bool {
    let norm = normalize_for_command_match(text);
    const DIRECT_PHRASES: &[&str] = &[
        "ferme toi", "cache toi", "masque toi", "disparais", "fermer overlay", "cacher overlay", "masquer overlay",
        "hide yourself", "close yourself", "hide overlay", "close overlay", "disappear", "minimize overlay",
        "ocultate", "ocúltate", "escondete", "escóndete", "ocultar overlay", "cerrar overlay", "desaparece",
        "versteck dich", "verstecke dich", "overlay schliessen", "overlay schließen", "overlay verstecken", "verschwinde",
        "nasconditi", "chiuditi", "nascondi overlay", "chiudi overlay", "sparisci",
        "esconda se", "esconda-se", "oculte se", "oculte-se", "esconder overlay", "fechar overlay", "desapareca", "desapareça",
        "скройся", "спрячься", "закройся", "скрыть оверлей", "исчезни",
        "隐藏", "隐藏自己", "退下", "关闭悬浮窗", "隐藏窗口",
        "隠れて", "非表示", "消えて", "オーバーレイを隠して",
    ];
    if DIRECT_PHRASES.iter().any(|&p| norm.contains(p) || text.contains(p)) {
        return true;
    }
    let has_verb = norm.contains("ferme") || norm.contains("cache") || norm.contains("masque") || norm.contains("hide") || norm.contains("close");
    let has_target = norm.contains("overlay") || norm.contains("deepseek") || norm.contains("libertide");
    has_verb && has_target
}

pub fn is_show_command(text: &str) -> bool {
    let norm = normalize_for_command_match(text);
    const DIRECT_PHRASES: &[&str] = &[
        "ouvre toi", "affiche toi", "montre toi", "reveille toi", "réveille toi", "ouvrir overlay", "afficher overlay",
        "show yourself", "open up", "wake up", "appear", "open overlay", "show overlay",
        "muestrate", "muéstrate", "abrete", "ábrete", "despierta", "aparece", "mostrar overlay", "abrir overlay",
        "zeige dich", "offne dich", "öffne dich", "aufwachen", "erscheine", "overlay zeigen", "overlay offnen", "overlay öffnen",
        "mostrati", "apriti", "svegliati", "mostra overlay", "apri overlay",
        "mostre se", "mostre-se", "abra se", "abra-se", "acorde", "apareca", "apareça", "mostrar overlay", "abrir overlay",
        "покажись", "откройся", "проснись", "открыть оверлей", "показать оверлей",
        "出来", "显示", "唤醒", "打开悬浮窗", "打开",
        "出てきて", "表示して", "起きて", "オーバーレイを表示",
    ];
    if DIRECT_PHRASES.iter().any(|&p| norm.contains(p) || text.contains(p)) {
        return true;
    }
    let has_verb = norm.contains("ouvre") || norm.contains("affiche") || norm.contains("montre") || norm.contains("show") || norm.contains("open");
    let has_target = norm.contains("overlay") || norm.contains("deepseek") || norm.contains("libertide");
    has_verb && has_target
}

pub fn is_api_key_command(text: &str) -> bool {
    let norm = normalize_for_command_match(text);
    const TRIGGERS: &[&str] = &[
        "api key", "apikey", "api cle", "api clé", "cle api", "clé api",
        "clave api", "llave api",
        "api schlussel", "api schlüssel",
        "chiave api",
        "chave api",
        "ключ апи", "апи ключ", "ключ api",
        "api密钥", "api 密钥",
        "apiキー", "api キー",
    ];
    TRIGGERS.iter().any(|&trig| norm.contains(trig) || text.contains(trig))
}

pub fn parse_agent_response(raw: &str) -> AgentResponsePayload {
    let clean = raw.trim();

    if clean.is_empty() || clean.eq_ignore_ascii_case("invalid_request") {
        return AgentResponsePayload {
            narration: String::new(),
            actions: Vec::new(),
            quick_suggestions: Vec::new(),
            invalid_request: true,
            next_passes: None,
        };
    }

    if let Ok(payload) = serde_json::from_str::<AgentResponsePayload>(clean) {
        return payload;
    }

    let unquoted = if let Some(start) = clean.find("```") {
        let after_ticks = clean[start + 3..].trim_start();
        let content_start = if after_ticks.len() >= 4 && after_ticks[..4].eq_ignore_ascii_case("json") {
            after_ticks[4..].trim_start()
        } else {
            after_ticks
        };
        if let Some(end) = content_start.rfind("```") {
            content_start[..end].trim()
        } else {
            content_start.trim()
        }
    } else {
        clean
    };

    if let Ok(mut payload) = serde_json::from_str::<AgentResponsePayload>(unquoted) {
        if payload.narration.trim().is_empty() && payload.actions.is_empty() {
            return AgentResponsePayload {
                narration: String::new(),
                actions: Vec::new(),
                quick_suggestions: Vec::new(),
                invalid_request: true,
                next_passes: None,
            };
        }
        return payload;
    }

    if let (Some(start), Some(end)) = (unquoted.find('{'), unquoted.rfind('}')) {
        if start <= end {
            let json_candidate = &unquoted[start..=end];
            if let Ok(payload) = serde_json::from_str::<AgentResponsePayload>(json_candidate) {
                if payload.narration.trim().is_empty() && payload.actions.is_empty() {
                    return AgentResponsePayload {
                        narration: String::new(),
                        actions: Vec::new(),
                        quick_suggestions: Vec::new(),
                        invalid_request: true,
                        next_passes: None,
                    };
                }
                return payload;
            }

            if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_candidate) {
                let narration = val
                    .get("narration")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                let invalid_request = val
                    .get("invalid_request")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let next_passes = val
                    .get("next_passes")
                    .or_else(|| val.get("max_passes"))
                    .or_else(|| val.get("reasoning_passes"))
                    .and_then(|v| v.as_u64())
                    .map(|n| (n as usize).clamp(1, 15));

                let mut actions = Vec::new();
                if let Some(arr) = val.get("actions").and_then(|v| v.as_array()) {
                    for item in arr {
                        if let Ok(act) = serde_json::from_value::<AgentAction>(item.clone()) {
                            actions.push(act);
                        }
                    }
                }

                let mut quick_suggestions = Vec::new();
                if let Some(arr) = val.get("quick_suggestions").and_then(|v| v.as_array()) {
                    for item in arr {
                        if let Ok(sug) = serde_json::from_value::<QuickSuggestionItem>(item.clone()) {
                            quick_suggestions.push(sug);
                        }
                    }
                }

                if !narration.is_empty() || !actions.is_empty() || invalid_request || next_passes.is_some() {
                    if narration.trim().is_empty() && actions.is_empty() {
                        return AgentResponsePayload {
                            narration: String::new(),
                            actions: Vec::new(),
                            quick_suggestions: Vec::new(),
                            invalid_request: true,
                            next_passes: None,
                        };
                    }
                    return AgentResponsePayload {
                        narration,
                        actions,
                        quick_suggestions,
                        invalid_request,
                        next_passes,
                    };
                }
            }
        }
    }

    if clean.contains("invalid_request") {
        return AgentResponsePayload {
            narration: String::new(),
            actions: Vec::new(),
            quick_suggestions: Vec::new(),
            invalid_request: true,
            next_passes: None,
        };
    }

    AgentResponsePayload {
        narration: clean.to_string(),
        actions: Vec::new(),
        quick_suggestions: Vec::new(),
        next_passes: None,
        invalid_request: false,
    }
}

pub fn resolve_llm_config() -> LlmConfig {
    let providers_cascade = [
        (LlmProvider::DeepSeek, &["DEEPSEEK_API_KEY"][..]),
        (LlmProvider::OpenAI, &["OPENAI_API_KEY"][..]),
        (LlmProvider::Anthropic, &["ANTHROPIC_API_KEY"][..]),
        (LlmProvider::Mistral, &["MISTRAL_API_KEY"][..]),
        (LlmProvider::Glm, &["GLM_API_KEY", "ZHIPU_API_KEY"][..]),
        (LlmProvider::Groq, &["GROQ_API_KEY"][..]),
        (LlmProvider::Local, &["LOCAL_LLAMA_MODEL"][..]),
    ];

    let forced_provider = std::env::var("LLM_PROVIDER")
        .ok()
        .or_else(|| read_env_var_from_file(".env", "LLM_PROVIDER"))
        .and_then(|s| LlmProvider::from_name(&s));

    let mut chosen_provider = LlmProvider::DeepSeek;
    let mut found_key = String::new();

    if let Some(forced) = forced_provider {
        chosen_provider = forced;
        let key_name = forced.env_key_name();
        found_key = std::env::var(key_name)
            .or_else(|_| read_env_var_from_file(".env", key_name).ok_or(()))
            .unwrap_or_default()
            .trim()
            .to_string();
    } else {
        for (prov, keys) in &providers_cascade {
            if !prov.is_enabled() {
                continue;
            }
            if *prov == LlmProvider::Local {
                chosen_provider = *prov;
                break;
            }
            let specific_endpoint_key = prov.env_endpoint_name();
            let has_custom_endpoint = std::env::var(specific_endpoint_key)
                .ok()
                .or_else(|| read_env_var_from_file(".env", specific_endpoint_key))
                .map(|s| !s.trim().is_empty() && s.trim() != prov.default_endpoint())
                .unwrap_or(false);
            if has_custom_endpoint {
                chosen_provider = *prov;
                break;
            }

            for &k in *keys {
                if let Ok(val) = std::env::var(k) {
                    let t = val.trim();
                    if !t.is_empty() {
                        found_key = t.to_string();
                        chosen_provider = *prov;
                        break;
                    }
                }
                if let Some(val) = read_env_var_from_file(".env", k) {
                    let t = val.trim();
                    if !t.is_empty() {
                        found_key = t.to_string();
                        chosen_provider = *prov;
                        break;
                    }
                }
            }
            if !found_key.is_empty() {
                break;
            }
        }
    }

    let specific_model_key = chosen_provider.env_model_name();
    let model_override = std::env::var(specific_model_key)
        .ok()
        .or_else(|| read_env_var_from_file(".env", specific_model_key))
        .or_else(|| std::env::var("LLM_MODEL").ok())
        .or_else(|| read_env_var_from_file(".env", "LLM_MODEL"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let specific_endpoint_key = chosen_provider.env_endpoint_name();
    let endpoint_override = std::env::var(specific_endpoint_key)
        .ok()
        .or_else(|| read_env_var_from_file(".env", specific_endpoint_key))
        .or_else(|| std::env::var("LLM_BASE_URL").ok())
        .or_else(|| std::env::var("LLM_ENDPOINT").ok())
        .or_else(|| read_env_var_from_file(".env", "LLM_BASE_URL"))
        .or_else(|| read_env_var_from_file(".env", "LLM_ENDPOINT"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let model = model_override.unwrap_or_else(|| chosen_provider.default_model().to_string());
    let endpoint = endpoint_override.unwrap_or_else(|| chosen_provider.default_endpoint().to_string());
    let enabled = chosen_provider.is_enabled();

    LlmConfig {
        provider: chosen_provider,
        api_key: found_key,
        model,
        endpoint,
        enabled,
    }
}

#[cfg(windows)]
pub fn restart_application() {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    if let Ok(exe) = std::env::current_exe() {
        let exe_str = exe.to_string_lossy().to_string();
        let exe_clean = exe_str.strip_prefix(r"\\?\").unwrap_or(&exe_str).to_string();
        let args: Vec<String> = std::env::args().skip(1).collect();
        let cwd = std::env::current_dir().ok();
        let working_dir_clean = cwd.as_ref().map(|d| {
            let s = d.to_string_lossy().to_string();
            s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
        });

        let arg_list = if args.is_empty() {
            String::new()
        } else {
            let formatted_args: Vec<String> = args
                .iter()
                .map(|a| format!("'{}'", a.replace('\'', "''")))
                .collect();
            format!(" -ArgumentList @({})", formatted_args.join(","))
        };

        let working_dir_arg = if let Some(ref d) = working_dir_clean {
            format!(" -WorkingDirectory '{}'", d.replace('\'', "''"))
        } else {
            String::new()
        };

        let ps_script = format!(
            "Start-Sleep -Milliseconds 800; Start-Process -FilePath '{}'{}{}",
            exe_clean.replace('\'', "''"),
            working_dir_arg,
            arg_list
        );

        let mut ps_cmd = std::process::Command::new("powershell");
        ps_cmd
            .args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &ps_script])
            .creation_flags(CREATE_NO_WINDOW);

        if let Some(ref d) = working_dir_clean {
            ps_cmd.current_dir(d);
        }

        if ps_cmd.spawn().is_ok() {
            std::process::exit(0);
        }

        let args_str = if args.is_empty() {
            String::new()
        } else {
            format!(" {}", args.join(" "))
        };
        let cmd_line = format!(r#"/C ping 127.0.0.1 -n 2 >nul & start "" "{}"{}"#, exe_clean, args_str);
        let mut cmd = std::process::Command::new("cmd.exe");
        cmd.raw_arg(cmd_line)
            .creation_flags(CREATE_NO_WINDOW);

        if let Some(ref d) = working_dir_clean {
            cmd.current_dir(d);
        }

        let _ = cmd.spawn();
        std::process::exit(0);
    }
}

#[cfg(not(windows))]
pub fn restart_application() {
    if let Ok(exe) = std::env::current_exe() {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c")
            .arg(format!("sleep 1 && \"{}\" \"$@\"", exe.display()))
            .arg("--")
            .args(&args);

        if let Ok(cwd) = std::env::current_dir() {
            cmd.current_dir(cwd);
        }

        let _ = cmd.spawn();
        std::process::exit(0);
    }
}

pub fn try_execute_local_navigation_shortcut(prompt: &str) -> Option<&'static str> {
    let norm = normalize_for_command_match(prompt);
    match norm.as_str() {
        "clic" | "clic gauche" | "click" | "left click"
        | "hacer clic" | "clic izquierdo" | "klick" | "linksklick"
        | "clic sinistro" | "clique esquerdo" | "левый клик" | "клик"
        | "点击" | "左键点击" | "左クリック" | "クリック" => {
            #[cfg(windows)]
            unsafe {
                windows::Win32::UI::Input::KeyboardAndMouse::mouse_event(
                    windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0,
                );
                std::thread::sleep(Duration::from_millis(25));
                windows::Win32::UI::Input::KeyboardAndMouse::mouse_event(
                    windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_LEFTUP, 0, 0, 0, 0,
                );
            }
            #[cfg(target_os = "macos")]
            {
                let _ = std::process::Command::new("osascript")
                    .args(["-e", "tell application \"System Events\" to click"])
                    .status();
            }
            #[cfg(all(not(windows), not(target_os = "macos")))]
            {
                let _ = std::process::Command::new("xdotool").args(["click", "1"]).status();
            }
            Some("Clic gauche.")
        }
        "clic droit" | "right click" | "clic derecho" | "rechtsklick"
        | "clic destro" | "clique direito" | "правый клик"
        | "右键点击" | "右键" | "右クリック" => {
            #[cfg(windows)]
            unsafe {
                windows::Win32::UI::Input::KeyboardAndMouse::mouse_event(
                    windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_RIGHTDOWN, 0, 0, 0, 0,
                );
                std::thread::sleep(Duration::from_millis(25));
                windows::Win32::UI::Input::KeyboardAndMouse::mouse_event(
                    windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_RIGHTUP, 0, 0, 0, 0,
                );
            }
            #[cfg(target_os = "macos")]
            {
                let _ = std::process::Command::new("osascript")
                    .args(["-e", "tell application \"System Events\" to click using control down"])
                    .status();
            }
            #[cfg(all(not(windows), not(target_os = "macos")))]
            {
                let _ = std::process::Command::new("xdotool").args(["click", "3"]).status();
            }
            Some("Clic droit.")
        }
        "double clic" | "double click" | "doble clic" | "doppelklick"
        | "doppio clic" | "duplo clique" | "двойной клик"
        | "双击" | "ダブルクリック" => {
            #[cfg(windows)]
            unsafe {
                use windows::Win32::UI::Input::KeyboardAndMouse::{mouse_event, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP};
                mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0);
                mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0);
                std::thread::sleep(Duration::from_millis(50));
                mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0);
                mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0);
            }
            #[cfg(target_os = "macos")]
            {
                let _ = std::process::Command::new("osascript")
                    .args(["-e", "tell application \"System Events\" to repeat 2 times\nclick\ndelay 0.05\nend repeat"])
                    .status();
            }
            #[cfg(all(not(windows), not(target_os = "macos")))]
            {
                let _ = std::process::Command::new("xdotool").args(["click", "--repeat", "2", "1"]).status();
            }
            Some("Double clic.")
        }
        "enregistre" | "enregistrer" | "sauvegarde" | "sauvegarder" | "save"
        | "guardar" | "speichern" | "salva" | "salvare" | "salvar"
        | "сохранить" | "保存" => {
            #[cfg(windows)]
            send_hotkey(&[VIRTUAL_KEY(0x11)], VIRTUAL_KEY(0x53));
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("osascript")
                .args(["-e", "tell application \"System Events\" to keystroke \"s\" using command down"]).status();
            #[cfg(all(not(windows), not(target_os = "macos")))]
            let _ = std::process::Command::new("xdotool").args(["key", "ctrl+s"]).status();
            Some("Sauvegardé (Ctrl+S).")
        }
        "annule" | "annuler" | "undo" | "deshacer" | "rückgängig"
        | "ruckgangig" | "annulla" | "desfazer" | "отменить"
        | "отмена" | "撤销" | "元に戻す" => {
            if let Some(msg) = undo_last_system_action() {
                println!("[Undo] Action compensatoire exécutée : {msg}");
            } else {
                #[cfg(windows)]
                send_hotkey(&[VIRTUAL_KEY(0x11)], VIRTUAL_KEY(0x5A));
                #[cfg(target_os = "macos")]
                let _ = std::process::Command::new("osascript").args(["-e", "tell application \"System Events\" to keystroke \"z\" using command down"]).status();
                #[cfg(all(not(windows), not(target_os = "macos")))]
                let _ = std::process::Command::new("xdotool").args(["key", "ctrl+z"]).status();
            }
            Some("Annulé.")
        }
        "selectionne tout" | "sélectionne tout" | "select all" | "tout selectionner"
        | "seleccionar todo" | "alles auswählen" | "alles auswahlen"
        | "seleziona tutto" | "selecionar tudo" | "выбрать все" | "выбрать всё"
        | "全选" | "すべて選択" => {
            #[cfg(windows)]
            send_hotkey(&[VIRTUAL_KEY(0x11)], VIRTUAL_KEY(0x41));
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("osascript").args(["-e", "tell application \"System Events\" to keystroke \"a\" using command down"]).status();
            #[cfg(all(not(windows), not(target_os = "macos")))]
            let _ = std::process::Command::new("xdotool").args(["key", "ctrl+a"]).status();
            Some("Tout sélectionné.")
        }
        "entree" | "entrée" | "valide" | "valider" | "enter"
        | "intro" | "eingabe" | "bestätigen" | "bestatigen" | "invio"
        | "conferma" | "confirmar" | "ввод" | "подтвердить"
        | "回车" | "确认" | "決定" | "確定" => {
            #[cfg(windows)]
            send_hotkey(&[], VIRTUAL_KEY(0x0D));
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("osascript").args(["-e", "tell application \"System Events\" to key code 36"]).status();
            #[cfg(all(not(windows), not(target_os = "macos")))]
            let _ = std::process::Command::new("xdotool").args(["key", "Return"]).status();
            Some("Touche Entrée validée.")
        }
        "tab" | "tabulation" | "suivant" | "champ suivant" | "next"
        | "next field" | "siguiente" | "campo siguiente" | "weiter"
        | "nächstes feld" | "successivo" | "próximo" | "следующий"
        | "制表" | "下一个" | "次へ" => {
            #[cfg(windows)]
            send_hotkey(&[], VIRTUAL_KEY(0x09));
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("osascript").args(["-e", "tell application \"System Events\" to key code 48"]).status();
            #[cfg(all(not(windows), not(target_os = "macos")))]
            let _ = std::process::Command::new("xdotool").args(["key", "Tab"]).status();
            Some("Tabulation.")
        }
        "precedent" | "précédent" | "champ precedent" | "champ précédent"
        | "previous" | "previous field" | "anterior" | "campo anterior"
        | "zurück feld" | "precedente" | "предыдущий"
        | "上一个" | "前へ" => {
            #[cfg(windows)]
            send_hotkey(&[VIRTUAL_KEY(0x10)], VIRTUAL_KEY(0x09));
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("osascript").args(["-e", "tell application \"System Events\" to key code 48 using shift down"]).status();
            #[cfg(all(not(windows), not(target_os = "macos")))]
            let _ = std::process::Command::new("xdotool").args(["key", "shift+Tab"]).status();
            Some("Champ précédent.")
        }
        "descends" | "descendre" | "page suivante" | "scroll down"
        | "bajar" | "nach unten" | "nach unten scrollen" | "scorri giù"
        | "rolar para baixo" | "прокрутить вниз" | "вниз"
        | "向下滚动" | "下翻" | "下にスクロール" => {
            #[cfg(windows)]
            send_hotkey(&[], VIRTUAL_KEY(0x22));
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("osascript").args(["-e", "tell application \"System Events\" to key code 121"]).status();
            #[cfg(all(not(windows), not(target_os = "macos")))]
            let _ = std::process::Command::new("xdotool").args(["key", "Page_Down"]).status();
            Some("Défilement vers le bas.")
        }
        "monte" | "monter" | "page precedente" | "page précédente" | "scroll up"
        | "subir" | "nach oben" | "nach oben scrollen" | "scorri su"
        | "rolar para cima" | "прокрутить вверх" | "вверх"
        | "向上滚动" | "上翻" | "上にスクロール" => {
            #[cfg(windows)]
            send_hotkey(&[], VIRTUAL_KEY(0x21));
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("osascript").args(["-e", "tell application \"System Events\" to key code 116"]).status();
            #[cfg(all(not(windows), not(target_os = "macos")))]
            let _ = std::process::Command::new("xdotool").args(["key", "Page_Up"]).status();
            Some("Défilement vers le haut.")
        }
        "copie" | "copier" | "copy" | "copiar" | "kopieren"
        | "copia" | "копировать"
        | "复制" | "コピー" => {
            #[cfg(windows)]
            send_hotkey(&[VIRTUAL_KEY(0x11)], VIRTUAL_KEY(0x43));
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("osascript").args(["-e", "tell application \"System Events\" to keystroke \"c\" using command down"]).status();
            #[cfg(all(not(windows), not(target_os = "macos")))]
            let _ = std::process::Command::new("xdotool").args(["key", "ctrl+c"]).status();
            Some("Copié.")
        }
        "colle" | "coller" | "paste" | "pegar" | "einfügen"
        | "einfugen" | "incolla" | "colar" | "вставить"
        | "粘贴" | "貼り付け" => {
            #[cfg(windows)]
            send_hotkey(&[VIRTUAL_KEY(0x11)], VIRTUAL_KEY(0x56));
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("osascript").args(["-e", "tell application \"System Events\" to keystroke \"v\" using command down"]).status();
            #[cfg(all(not(windows), not(target_os = "macos")))]
            let _ = std::process::Command::new("xdotool").args(["key", "ctrl+v"]).status();
            Some("Collé.")
        }
        "retour" | "retour arriere" | "retour arrière" | "reculer" | "back"
        | "browser back" | "atrás" | "atras" | "volver" | "zurück"
        | "indietro" | "voltar" | "назад"
        | "后退" | "戻る" => {
            #[cfg(windows)]
            send_hotkey(&[VIRTUAL_KEY(0x12)], VIRTUAL_KEY(0x25));
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("osascript").args(["-e", "tell application \"System Events\" to keystroke \"[\" using command down"]).status();
            #[cfg(all(not(windows), not(target_os = "macos")))]
            let _ = std::process::Command::new("xdotool").args(["key", "alt+Left"]).status();
            Some("Page précédente (Retour arrière).")
        }
        "avance" | "avancer" | "forward" | "browser forward" | "adelante"
        | "avanzar" | "vorwärts" | "avanti" | "avançar" | "вперед"
        | "вперёд" | "前进" | "進む" => {
            #[cfg(windows)]
            send_hotkey(&[VIRTUAL_KEY(0x12)], VIRTUAL_KEY(0x27));
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("osascript").args(["-e", "tell application \"System Events\" to keystroke \"]\" using command down"]).status();
            #[cfg(all(not(windows), not(target_os = "macos")))]
            let _ = std::process::Command::new("xdotool").args(["key", "alt+Right"]).status();
            Some("Page suivante.")
        }
        "omnibox" | "l omnibox" | "omnibar" | "l omnibar" | "barre d adresse" | "barre adresse" | "barre de recherche" | "adresse" | "address bar"
        | "barra de direcciones" | "adressleiste" | "barra degli indirizzi"
        | "barra de endereços" | "адресная строка" | "地址栏" | "アドレスバー"
        | "ouvre l omnibox" | "ouvrir l omnibox" | "va sur l omnibox" | "selectionne l omnibox" | "selectionner l omnibox"
        | "focus omnibox" | "focus barre d adresse" => {
            #[cfg(windows)]
            {
                let user_windows = list_user_windows();
                if let Some(hwnd) = find_best_browser_window(&user_windows, None) {
                    focus_browser_address_bar(hwnd);
                    Some("Barre d'adresse (omnibox) sélectionnée.")
                } else {
                    launch_browser_new_window("https://www.google.com");
                    Some("Navigateur ouvert avec focus sur l'omnibox.")
                }
            }
            #[cfg(not(windows))]
            {
                #[cfg(target_os = "macos")]
                let _ = std::process::Command::new("osascript")
                    .args(["-e", "tell application \"System Events\" to keystroke \"l\" using command down"]).status();
                #[cfg(all(not(windows), not(target_os = "macos")))]
                let _ = std::process::Command::new("xdotool").args(["key", "ctrl+l"]).status();
                Some("Barre d'adresse (omnibox) sélectionnée.")
            }
        }
        _ => None,
    }
}

#[cfg(windows)]
pub fn get_window_title(hwnd: windows::Win32::Foundation::HWND) -> String {
    let mut buf = [0u16; 512];
    let len = unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowTextW(hwnd, &mut buf) };
    if len > 0 {
        String::from_utf16_lossy(&buf[..len as usize]).trim().to_string()
    } else {
        String::new()
    }
}

pub fn focus_element_target(target: &NumberedElementTarget) {
    if let Ok(mut guard) = LAST_FOCUSED_RECT.lock() {
        *guard = Some(target.rect);
    }

    set_global_cursor_pos(target.click_x, target.click_y);

    #[cfg(windows)]
    unsafe {
        let top_hwnd = if let Some(wid) = target.window_id {
            windows::Win32::Foundation::HWND(wid as _)
        } else {
            let pt = windows::Win32::Foundation::POINT { x: target.click_x, y: target.click_y };
            let target_child = windows::Win32::UI::WindowsAndMessaging::WindowFromPoint(pt);
            if !target_child.is_invalid() {
                let root = windows::Win32::UI::WindowsAndMessaging::GetAncestor(
                    target_child,
                    windows::Win32::UI::WindowsAndMessaging::GA_ROOT,
                );
                if !root.is_invalid() { root } else { target_child }
            } else {
                windows::Win32::Foundation::HWND(std::ptr::null_mut())
            }
        };

        if !top_hwnd.0.is_null() {
            let cur_fg = windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow();
            if cur_fg != top_hwnd {
                force_foreground_window(top_hwnd);
                restore_if_iconic(top_hwnd);
            }
        }

        let pt = windows::Win32::Foundation::POINT { x: target.click_x, y: target.click_y };
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        if let Ok(automation) = windows::Win32::System::Com::CoCreateInstance::<_, windows::Win32::UI::Accessibility::IUIAutomation>(
            &windows::Win32::UI::Accessibility::CUIAutomation,
            None,
            windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
        ) {
            if let Ok(elem) = automation.ElementFromPoint(pt) {
                let is_focusable = elem.CurrentIsKeyboardFocusable().map(|b| b.as_bool()).unwrap_or(false);
                if is_focusable {
                    let _ = elem.SetFocus();
                }
            }
        }
    }
}

pub fn build_fallback_navigation_zones(window_title: &str, targets: &[NumberedElementTarget]) -> (String, Vec<NavigationZone>) {
    if targets.is_empty() {
        return (format!("Fenêtre {window_title}. Aucun contrôle interactif direct détecté."), Vec::new());
    }

    // Regroupement par "features" / intentions fonctionnelles (façon mini-site accessible)
    // plutôt qu'un découpage spatial aveugle (haut/bas/gauche/droite).
    let mut search_input_targets = Vec::new();
    let mut nav_tab_targets = Vec::new();
    let mut action_button_targets = Vec::new();
    let mut content_reader_targets = Vec::new();
    let mut other_targets = Vec::new();

    for t in targets {
        let role_low = t.role.to_lowercase();
        let name_low = t.name.to_lowercase();

        if t.is_editable()
            || role_low.contains("champ")
            || role_low.contains("saisie")
            || role_low.contains("texte")
            || name_low.contains("recherche")
            || name_low.contains("search")
        {
            search_input_targets.push(t.clone());
        } else if role_low.contains("onglet")
            || role_low.contains("tab")
            || (role_low.contains("lien") && (name_low.contains("accueil") || name_low.contains("menu") || name_low.contains("page")))
        {
            nav_tab_targets.push(t.clone());
        } else if t.is_text_reading() || t.is_label() || t.is_ocr() {
            content_reader_targets.push(t.clone());
        } else if role_low.contains("bouton") || role_low.contains("case") || role_low.contains("curseur") {
            action_button_targets.push(t.clone());
        } else {
            other_targets.push(t.clone());
        }
    }

    let mut fallback_zones = Vec::new();

    let mut add_feature_zone = |zones: &mut Vec<NavigationZone>, title: &str, desc: &str, mut group: Vec<NumberedElementTarget>| {
        if group.is_empty() {
            return;
        }
        // Tri des contrôles par ordre visuel de lecture naturelle
        group.sort_by_key(|t| (t.rect[1] / 24) * 10000 + t.rect[0]);

        let z_min_x = group.iter().map(|t| t.rect[0]).min().unwrap_or(0);
        let z_min_y = group.iter().map(|t| t.rect[1]).min().unwrap_or(0);
        let z_max_x = group.iter().map(|t| t.rect[2]).max().unwrap_or(0);
        let z_max_y = group.iter().map(|t| t.rect[3]).max().unwrap_or(0);
        let pad = 8;
        for (i, elem) in group.iter_mut().enumerate() {
            elem.index = i + 1;
        }
        zones.push(NavigationZone {
            title: title.to_string(),
            description: desc.to_string(),
            click_x: z_min_x + (z_max_x - z_min_x) / 2,
            click_y: z_min_y + (z_max_y - z_min_y) / 2,
            rect: [z_min_x - pad, z_min_y - pad, z_max_x + pad, z_max_y + pad],
            targets: group,
        });
    };

    add_feature_zone(&mut fallback_zones, "Recherche & Formulaire de saisie", &format!("Champs de texte et saisie pour {window_title}"), search_input_targets);
    add_feature_zone(&mut fallback_zones, "Navigation & Rubriques", "Onglets et liens de navigation", nav_tab_targets);
    add_feature_zone(&mut fallback_zones, "Contenu & Informations clés", "Données textuelles et visualisations", content_reader_targets);
    add_feature_zone(&mut fallback_zones, "Actions & Commandes", "Boutons d'exécution et de validation", action_button_targets);

    if !other_targets.is_empty() {
        if fallback_zones.is_empty() {
            add_feature_zone(&mut fallback_zones, "Panneau d'interaction", &format!("Contrôles principaux de {window_title}"), other_targets);
        } else {
            // Rattaché à la zone d'action ou de contenu
            fallback_zones.last_mut().unwrap().targets.extend(other_targets);
        }
    }

    // Si aucune feature séparée n'a pu émerger (ex. uniquement des boutons d'une même liste)
    if fallback_zones.len() <= 1 {
        fallback_zones.clear();
        let mut all_sorted = targets.to_vec();
        all_sorted.sort_by_key(|t| (t.rect[1] / 30) * 10000 + t.rect[0]);
        if all_sorted.len() > 12 {
            let half = all_sorted.len() / 2;
            let first_half = all_sorted[..half].to_vec();
            let second_half = all_sorted[half..].to_vec();
            add_feature_zone(&mut fallback_zones, "Options principales", &format!("Première série d'options de {window_title}"), first_half);
            add_feature_zone(&mut fallback_zones, "Options complémentaires", &format!("Seconde série d'options de {window_title}"), second_half);
        } else {
            add_feature_zone(&mut fallback_zones, "Panneau principal", &format!("Fonctionnalités de {window_title}"), all_sorted);
        }
    }

    for z in fallback_zones.iter_mut() {
        inject_text_reader_target(z);
    }

    let notable_items: Vec<&str> = targets
        .iter()
        .filter(|t| !t.name.trim().is_empty() && t.role != "lecture textuelle")
        .take(4)
        .map(|t| t.name.trim())
        .collect();

    let summary = if !notable_items.is_empty() {
        format!("Fenêtre {}. {} zones d'interaction disponibles : {}.", window_title, fallback_zones.len(), notable_items.join(", "))
    } else {
        format!("Fenêtre {}. {} zones d'interaction identifiées.", window_title, fallback_zones.len())
    };
    (summary, fallback_zones)
}


/// Dessine les zones de navigation sous forme de cibles numérotées sur l'écran
pub fn draw_screen_zones(zones: &[ZoneOverlayInfo], zoom_factor: f32) {
    crate::window_util::draw_screen_zones_and_targets(zones, &[], zoom_factor, None);
}

#[cfg(windows)]
pub fn extract_window_context_meta(hwnd: windows::Win32::Foundation::HWND, title: &str) -> Option<String> {
    if hwnd.0.is_null() || is_window_hung(hwnd) {
        return None;
    }
    let mut meta = Vec::new();
    let (_pid, exe_name) = get_window_process_info(hwnd);
    meta.push(format!("Application hôte : \"{}\"", exe_name));

    if let Some(file) = extract_filename_from_title(title) {
        meta.push(format!("Document ou fichier actif : \"{}\"", file));
    }

    if is_browser_hwnd(hwnd, title) {
        unsafe {
            if let Ok(uia) = windows::Win32::System::Com::CoCreateInstance::<_, windows::Win32::UI::Accessibility::IUIAutomation>(
                &windows::Win32::UI::Accessibility::CUIAutomation,
                None,
                windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
            ) {
                if let Ok(win_elem) = uia.ElementFromHandle(hwnd) {
                    if let Ok(cond_doc) = uia.CreatePropertyCondition(
                        windows::Win32::UI::Accessibility::UIA_ControlTypePropertyId,
                        &windows::core::VARIANT::from(windows::Win32::UI::Accessibility::UIA_DocumentControlTypeId.0),
                    ) {
                        if let Ok(doc_elements) = win_elem.FindAll(windows::Win32::UI::Accessibility::TreeScope_Descendants, &cond_doc) {
                            if let Ok(doc_elem) = doc_elements.GetElement(0) {
                                if let Some(doc_text) = crate::ui::extract_web_document_text(&doc_elem, 1500) {
                                    let trimmed = doc_text.trim();
                                    if !trimmed.is_empty() {
                                        meta.push(format!("Extrait du corps de page :\n\"\"\"\n{}\n\"\"\"", truncate_with_notice(trimmed, 800)));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    } else if is_terminal_window(hwnd, title) {
        if let Some(term_text) = read_terminal_window_content(hwnd, title) {
            meta.push(format!("Dernières lignes console :\n\"\"\"\n{}\n\"\"\"", truncate_with_notice(&term_text, 600)));
        }
    }

    if meta.is_empty() { None } else { Some(meta.join("\n")) }
}

pub async fn analyze_navigation_zones_with_llm(
    client: &reqwest::Client,
    config: &LlmConfig,
    window_title: &str,
    targets: &[NumberedElementTarget],
    context_meta: Option<&str>,
) -> (String, Vec<NavigationZone>) {
    if targets.is_empty() {
        return (format!("Fenêtre {window_title}. Aucun contrôle interactif direct détecté."), Vec::new());
    }
    let mut prompt_content = format!("Fenêtre analysée : \"{}\"\nContrôles et éléments de contenu détectés :\n", window_title);
    if let Some(meta) = context_meta {
        prompt_content.push_str(&format!("[Méta-informations et extrait du document] :\n{}\n\n", meta));
    }
    prompt_content.push_str("Contrôles et éléments interactifs détectés :\n");
    for t in targets.iter().take(60) {
        let val_part = if !t.value.is_empty() {
            format!(" (valeur/état: \"{}\")", t.value)
        } else {
            String::new()
        };
        prompt_content.push_str(&format!("- ID #{}: \"{}\" (rôle: {}{}, raccourci: {}) [x:{}, y:{}, w:{}, h:{}]\n", t.index, t.name, t.role, val_part, t.shortcut, t.rect[0], t.rect[1], t.rect[2] - t.rect[0], t.rect[3] - t.rect[1]));
    }
    let (_, lang_name) = get_system_locale_language();
    let sys_prompt = format!(r#"Tu es un expert d'ergonomie et d'accessibilité numérique pour personnes malvoyantes et non-voyantes (Screen Reader & Switch Access).
Tu reçois la liste des éléments interactifs et textuels d'une fenêtre applicative ou page web.

RÈGLES D'OR DE DÉCOUPAGE EN MINI-SITE ACCESSIBLE (FEATURE PAR FEATURE) :
1. PHILOSOPHIE : Tu dois réinventer l'écran sous la forme d'un MINI-SITE ACCESSIBLE structuré autour des VRAIES FONCTIONNALITÉS MÉTIER (features / intentions de l'utilisateur).
2. INTERDICTION FORMELLE DU DÉCOUPAGE GÉOMÉTRIQUE SPATIAL NAÏF :
   - Ne crée JAMAIS de zones nommées d'après leur position : INTERDIT d'écrire "Haut", "Bas", "Gauche", "Droite", "Milieu", "En-tête", "Panneau latéral", "Zone centrale", "Pied de page".
3. DÉCOUPAGE EN 2 À 4 FEATURES PARLANTES :
   Chaque zone bleue ("title") doit représenter une fonctionnalité concrète que l'utilisateur veut accomplir, par exemple :
   - "Recherche & Filtres catalogue" (au lieu de "Haut")
   - "Lecteur vidéo & Commandes de lecture"
   - "Formulaire de saisie & Coordonnées"
   - "Sélection des articles & Panier"
   - "Résultats & Fiches produits"
   - "Actions & Validation de commande"
   - "Navigation & Onglets de travail"
4. "summary" : Présentation vocale (2 à 4 phrases) en {lang_name} résumant l'objectif de la fenêtre et les fonctionnalités disponibles.
5. "revisited_name" : Rend chaque libellé de contrôle limpide, explicite et facile à comprendre pour un utilisateur naviguant au contacteur ou à la voix.

Réponds STRICTEMENT en JSON :
{{
  "summary": "Synthèse vocale des fonctionnalités et du contenu réel de l'écran.",
  "zones": [
    {{
      "title": "Nom fonctionnel de la feature (ex: Recherche & Filtres, Panier & Validation, Lecteur & Contrôles)",
      "description": "Contenu sémantique concret présent dans ce bloc",
      "controls": [
        {{ "index": 1, "revisited_name": "Libellé sémantique révisé" }}
      ]
    }}
  ]
}}"#);

    let messages = vec![ChatMessage {
        role: "user".to_string(),
        content: prompt_content,
    }];

    let response = send_llm_chat_completion(client, config, &sys_prompt, &messages, 0.3, 1200).await;

    if let Ok(res) = response {
        if res.status().is_success() {
            if let Ok(raw) = parse_llm_response_text(res, config.provider).await {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw) {
                    let summary = val.get("summary").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let mut parsed_zones = Vec::new();

                    if let Some(arr) = val.get("zones").and_then(|v| v.as_array()) {
                        for (z_idx, z_val) in arr.iter().enumerate() {
                            let title = z_val.get("title").and_then(|v| v.as_str()).unwrap_or(&format!("Zone {}", z_idx + 1)).to_string();
                            let desc = z_val.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string();
                            let mut group_targets = Vec::new();

                            if let Some(controls) = z_val.get("controls").and_then(|v| v.as_array()) {
                                for c in controls {
                                    let c_idx = c.get("index").and_then(|v| v.as_u64()).map(|n| n as usize);
                                    if let Some(ci) = c_idx {
                                        if let Some(orig) = targets.iter().find(|t| t.index == ci) {
                                            let mut cloned = orig.clone();
                                            if let Some(rev) = c.get("revisited_name").and_then(|v| v.as_str()) {
                                                if !rev.trim().is_empty() {
                                                    cloned.name = rev.trim().to_string();
                                                }
                                            }
                                            group_targets.push(cloned);
                                        }
                                    }
                                }
                            }

                            if !group_targets.is_empty() {
                                let z_min_x = group_targets.iter().map(|t| t.rect[0]).min().unwrap_or(0);
                                let z_min_y = group_targets.iter().map(|t| t.rect[1]).min().unwrap_or(0);
                                let z_max_x = group_targets.iter().map(|t| t.rect[2]).max().unwrap_or(0);
                                let z_max_y = group_targets.iter().map(|t| t.rect[3]).max().unwrap_or(0);
                                let pad = 8;
                                parsed_zones.push(NavigationZone {
                                    title,
                                    description: desc,
                                    click_x: z_min_x + (z_max_x - z_min_x) / 2,
                                    click_y: z_min_y + (z_max_y - z_min_y) / 2,
                                    rect: [z_min_x - pad, z_min_y - pad, z_max_x + pad, z_max_y + pad],
                                    targets: group_targets,
                                });
                            }
                        }
                    }

                    if !parsed_zones.is_empty() {
                        for z in parsed_zones.iter_mut() {
                            inject_text_reader_target(z);
                        }
                        return (summary, parsed_zones);
                    }
                }
            }
        }
    }

    build_fallback_navigation_zones(window_title, targets)
}
pub fn resolve_initial_voice() -> VoiceGender {
    if let Ok(key) = std::env::var("VOICE_PROFILE") {
        if let Some(v) = VoiceProfile::from_id_or_name(&key) {
            return v;
        }
    }

    if let Some(val) = read_env_var_from_file(".env", "VOICE_PROFILE") {
        if let Some(v) = VoiceProfile::from_id_or_name(&val) {
            return v;
        }
    }

    VoiceGender::Denise
}

pub fn render_accessible_icon_button(
    ui: &mut egui::Ui,
    icon: &str,
    sub_label: &str,
    tooltip: &str,
    active: bool,
    dwell_progress: Option<f32>,
    active_color: Option<egui::Color32>,
    hover_color: Option<egui::Color32>,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(ACCESSIBILITY_BTN_SIZE, egui::Sense::click());

    let default_active = egui::Color32::from_rgb(0, 120, 215);
    let chosen_active = active_color.unwrap_or(default_active);
    let default_hover = egui::Color32::from_gray(65);
    let chosen_hover = hover_color.unwrap_or(default_hover);

    let bg_color = if active {
        chosen_active
    } else if response.hovered() {
        chosen_hover
    } else {
        egui::Color32::from_gray(38)
    };

    ui.painter().rect_filled(rect, egui::CornerRadius::same(10), bg_color);

    if let Some(progress) = dwell_progress {
        if progress > 0.0 && progress <= 1.0 {
            let bar_height = 4.0;
            let progress_rect = egui::Rect::from_min_max(
                egui::pos2(rect.left(), rect.bottom() - bar_height),
                egui::pos2(rect.left() + rect.width() * progress, rect.bottom()),
            );
            ui.painter().rect_filled(
                progress_rect,
                egui::CornerRadius::same(2),
                egui::Color32::from_rgb(76, 217, 100),
            );
            ui.ctx().request_repaint();
        }
    }

    let stroke_color = if active {
        egui::Color32::WHITE
    } else if response.hovered() {
        egui::Color32::from_rgb(180, 220, 255)
    } else {
        egui::Color32::from_gray(80)
    };
    ui.painter().rect_stroke(
        rect,
        egui::CornerRadius::same(10),
        egui::Stroke::new(if active { 2.0 } else { 1.2 }, stroke_color),
        egui::StrokeKind::Inside,
    );

    let icon_pos = if sub_label.is_empty() {
        rect.center()
    } else {
        egui::pos2(rect.center().x, rect.center().y - 6.0)
    };

    ui.painter().text(
        icon_pos,
        egui::Align2::CENTER_CENTER,
        icon,
        ACCESSIBILITY_ICON_FONT,
        if active { egui::Color32::WHITE } else { egui::Color32::from_gray(235) },
    );

    if !sub_label.is_empty() {
        ui.painter().text(
            egui::pos2(rect.center().x, rect.bottom() - 9.0),
            egui::Align2::CENTER_CENTER,
            sub_label,
            ACCESSIBILITY_SUB_FONT,
            if active { egui::Color32::from_rgb(240, 240, 255) } else { egui::Color32::from_gray(180) },
        );
    }

    response.on_hover_text(tooltip)
}

#[derive(Clone, Copy, Debug)]
pub struct WorkArea {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub taskbar_top: f32,
}

pub fn get_desktop_work_area_scaled(ctx: &egui::Context) -> WorkArea {
    let ppp = ctx.pixels_per_point().max(0.5);
    #[cfg(windows)]
    {
        let wa = get_desktop_work_area();
        let left = wa.left as f32 / ppp;
        let top = wa.top as f32 / ppp;
        let right = wa.right as f32 / ppp;
        let bottom = wa.bottom as f32 / ppp;
        WorkArea {
            x: left,
            y: top,
            w: (right - left).max(380.0),
            h: (bottom - top).max(200.0),
            taskbar_top: bottom,
        }
    }
    #[cfg(not(windows))]
    {
        let mon = ctx.input(|i| i.viewport().monitor_size).unwrap_or(egui::vec2(1920.0, 1080.0));
        WorkArea {
            x: 0.0,
            y: 0.0,
            w: mon.x,
            h: mon.y,
            taskbar_top: mon.y,
        }
    }
}

pub struct OverlayLayout {
    pub win_w: f32,
    pub active_win_h: f32,
    pub min_chat_h: f32,
    pub max_chat_h: f32,
    pub control_rect: egui::Rect,
    pub response_rect: egui::Rect,
    pub padding: f32,
}

pub fn compute_and_sync_overlay_geometry(
    ctx: &egui::Context,
    work_area: &WorkArea,
    position_initialized: &mut bool,
    window_pos: &mut Option<egui::Pos2>,
    window_bottom_y: &mut Option<f32>,
    current_win_w: &mut f32,
    current_win_h: &mut f32,
    chat_height: f32,
    has_suggestions: bool,
    show_api_key_modal: bool,
    is_maximized: bool,
) -> OverlayLayout {
    let min_win_w = 380.0f32.min(work_area.w);
    let min_win_h = 196.0f32.min(work_area.h);
    ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(egui::vec2(min_win_w, min_win_h)));

    let inner_rect_opt = ctx.input(|i| i.viewport().inner_rect);
    let raw_win_w = if *position_initialized {
        inner_rect_opt.map_or(*current_win_w, |r| r.width())
    } else {
        *current_win_w
    };
    let win_w = raw_win_w.clamp(min_win_w, work_area.w);
    *current_win_w = win_w;

    let padding: f32 = 2.0;
    let control_h: f32 = if has_suggestions { 162.0 } else { 136.0 };

    let min_chat_h: f32 = 50.0;
    let max_chat_h = (work_area.h - control_h - 40.0).clamp(min_chat_h, 560.0);
    let current_chat_h = chat_height.clamp(min_chat_h, max_chat_h);

    let modal_min_h = if show_api_key_modal { 380.0f32.min(work_area.h) } else { min_win_h };
    let target_win_h = (padding + current_chat_h + padding + control_h + padding)
        .max(modal_min_h)
        .min(work_area.h)
        .round();
    let active_win_h = if *position_initialized {
        inner_rect_opt.map_or(*current_win_h, |r| r.height().max(min_win_h))
    } else {
        target_win_h
    };

    // Ancrage initial ferré à la barre des tâches
    if !*position_initialized {
        let margin_x = 16.0;
        let init_h = target_win_h;
        let bot_y = work_area.taskbar_top;
        let max_x = (work_area.x + work_area.w - win_w).max(work_area.x);
        let target_x = (work_area.x + work_area.w - win_w - margin_x).clamp(work_area.x, max_x);
        let max_y = (work_area.taskbar_top - init_h).max(work_area.y);
        let target_pos = egui::pos2(target_x, max_y);
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(win_w, init_h)));
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(target_pos));
        *window_pos = Some(target_pos);
        *window_bottom_y = Some(bot_y);
        *current_win_h = init_h;
        *position_initialized = true;
    }

    // Redimensionnement dynamique avec hystérésis (seuil >= 3.0 px)
    if *position_initialized && !is_maximized && (target_win_h - *current_win_h).abs() >= 3.0 {
        *current_win_h = target_win_h;
        let max_x = (work_area.x + work_area.w - win_w).max(work_area.x);
        let was_anchored_right = window_pos.map_or(true, |p| {
            (p.x + win_w - (work_area.x + work_area.w)).abs() <= 32.0 || p.x + win_w >= work_area.x + work_area.w
        });
        let cur_x = if was_anchored_right {
            (work_area.x + work_area.w - win_w - 16.0).clamp(work_area.x, max_x)
        } else {
            window_pos.map_or(max_x - 16.0, |p| p.x).clamp(work_area.x, max_x)
        };
        let max_y = (work_area.taskbar_top - target_win_h).max(work_area.y);
        let bot_y = window_bottom_y.unwrap_or(work_area.taskbar_top);
        let is_anchored_bottom = (bot_y - work_area.taskbar_top).abs() <= 32.0 || bot_y >= work_area.taskbar_top;
        let new_y = if is_anchored_bottom {
            max_y
        } else {
            (bot_y - target_win_h).clamp(work_area.y, max_y)
        };
        let new_pos = egui::pos2(cur_x, new_y);
        *window_pos = Some(new_pos);
        *window_bottom_y = Some(new_y + target_win_h);
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(win_w, target_win_h)));
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(new_pos));
    }

    let control_top = (active_win_h - padding - control_h).max(padding + min_chat_h);
    let control_rect = egui::Rect::from_min_size(
        egui::pos2(padding, control_top),
        egui::vec2(win_w - 2.0 * padding, control_h),
    );
    let response_rect = egui::Rect::from_min_max(
        egui::pos2(padding, padding),
        egui::pos2(win_w - padding, (control_rect.min.y - padding).max(padding + min_chat_h)),
    );

    OverlayLayout {
        win_w,
        active_win_h,
        min_chat_h,
        max_chat_h,
        control_rect,
        response_rect,
        padding,
    }
}

pub fn handle_overlay_drag(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    work_area: &WorkArea,
    layout: &OverlayLayout,
    tab_bar_max_x: f32,
    window_pos: &mut Option<egui::Pos2>,
    window_bottom_y: &mut Option<f32>,
    drag_offset: &mut Option<egui::Vec2>,
    is_maximized: bool,
) {
    let window_buttons_width = 84.0;
    let drag_rect_top = egui::Rect::from_min_max(
        egui::pos2(tab_bar_max_x + 6.0, layout.response_rect.min.y),
        egui::pos2(
            (layout.win_w - layout.padding - window_buttons_width).max(tab_bar_max_x + 6.0),
            layout.response_rect.min.y + 30.0,
        ),
    );
    let drag_rect_bot = egui::Rect::from_min_max(
        egui::pos2(layout.control_rect.min.x + 8.0, layout.control_rect.min.y + 4.0),
        egui::pos2(layout.control_rect.max.x - 48.0, layout.control_rect.min.y + 24.0),
    );
    let resp_top = ui.interact(drag_rect_top, egui::Id::new("capsule_drag_top"), egui::Sense::drag());
    let resp_bot = ui.interact(drag_rect_bot, egui::Id::new("capsule_drag_bot"), egui::Sense::drag());

    let is_hovered = resp_top.hovered() || resp_bot.hovered();
    let is_dragged = resp_top.dragged() || resp_bot.dragged();
    let drag_started = resp_top.drag_started() || resp_bot.drag_started();
    let drag_stopped = resp_top.drag_stopped() || resp_bot.drag_stopped();

    if is_hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }

    if drag_started {
        if let Some(cursor) = get_screen_cursor_pos(ctx) {
            let win_p = window_pos.unwrap_or_default();
            *drag_offset = Some(egui::vec2(cursor.x - win_p.x, cursor.y - win_p.y));
        }
    }

    if is_dragged && !is_maximized {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        let offset = drag_offset.unwrap_or(egui::vec2(layout.win_w / 2.0, 16.0));

        if let Some(cursor) = get_screen_cursor_pos(ctx) {
            let max_x = (work_area.x + work_area.w - layout.win_w).max(work_area.x);
            let max_y = (work_area.taskbar_top - layout.active_win_h).max(work_area.y);

            let raw_x = cursor.x - offset.x;
            let raw_y = cursor.y - offset.y;

            let target_y = if (raw_y - max_y).abs() < 24.0 || raw_y >= max_y {
                max_y
            } else {
                raw_y.clamp(work_area.y, max_y)
            };
            let target_x = raw_x.clamp(work_area.x, max_x);

            let new_pos = egui::pos2(target_x, target_y);
            *window_pos = Some(new_pos);
            *window_bottom_y = Some(new_pos.y + layout.active_win_h);
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(new_pos));
        }
    }
    if drag_stopped {
        *drag_offset = None;
    }
}

pub fn handle_overlay_resize_zones(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    win_w: f32,
    active_win_h: f32,
) {
    let border_thickness = 6.0;
    let total_w = win_w;
    let total_h = active_win_h;

    let resize_zones = [
        (egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(border_thickness, border_thickness)), egui::viewport::ResizeDirection::NorthWest, egui::CursorIcon::ResizeNorthWest),
        (egui::Rect::from_min_size(egui::pos2(total_w - border_thickness, 0.0), egui::vec2(border_thickness, border_thickness)), egui::viewport::ResizeDirection::NorthEast, egui::CursorIcon::ResizeNorthEast),
        (egui::Rect::from_min_size(egui::pos2(border_thickness, 0.0), egui::vec2((total_w - 2.0 * border_thickness).max(0.0), border_thickness)), egui::viewport::ResizeDirection::North, egui::CursorIcon::ResizeNorth),

        (egui::Rect::from_min_size(egui::pos2(0.0, border_thickness), egui::vec2(border_thickness, (total_h - 2.0 * border_thickness).max(0.0))), egui::viewport::ResizeDirection::West, egui::CursorIcon::ResizeWest),
        (egui::Rect::from_min_size(egui::pos2(total_w - border_thickness, border_thickness), egui::vec2(border_thickness, (total_h - 2.0 * border_thickness).max(0.0))), egui::viewport::ResizeDirection::East, egui::CursorIcon::ResizeEast),

        (egui::Rect::from_min_size(egui::pos2(0.0, total_h - border_thickness), egui::vec2(border_thickness, border_thickness)), egui::viewport::ResizeDirection::SouthWest, egui::CursorIcon::ResizeSouthWest),
        (egui::Rect::from_min_size(egui::pos2(total_w - border_thickness, total_h - border_thickness), egui::vec2(border_thickness, border_thickness)), egui::viewport::ResizeDirection::SouthEast, egui::CursorIcon::ResizeSouthEast),
        (egui::Rect::from_min_size(egui::pos2(border_thickness, total_h - border_thickness), egui::vec2((total_w - 2.0 * border_thickness).max(0.0), border_thickness)), egui::viewport::ResizeDirection::South, egui::CursorIcon::ResizeSouth),
    ];

    for (rect, direction, cursor_icon) in resize_zones {
        let resp = ui.interact(rect, egui::Id::new(format!("resize_{:?}", direction)), egui::Sense::drag());
        if resp.hovered() {
            ui.ctx().set_cursor_icon(cursor_icon);
        }
        if resp.drag_started() {
            ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
        }
    }
}

pub fn draw_corner_grips(painter: &egui::Painter, win_w: f32, active_win_h: f32, padding: f32) {
    let grip_nw = egui::Rect::from_min_size(egui::pos2(padding + 2.0, padding + 2.0), egui::vec2(10.0, 10.0));
    painter.line_segment(
        [egui::pos2(grip_nw.min.x, grip_nw.max.y), egui::pos2(grip_nw.max.x, grip_nw.min.y)],
        egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(0, 180, 255, 90)),
    );
    painter.line_segment(
        [egui::pos2(grip_nw.min.x + 3.0, grip_nw.max.y), egui::pos2(grip_nw.max.x, grip_nw.min.y + 3.0)],
        egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(0, 180, 255, 50)),
    );

    let grip_se = egui::Rect::from_min_size(egui::pos2(win_w - padding - 12.0, active_win_h - padding - 12.0), egui::vec2(10.0, 10.0));
    painter.line_segment(
        [egui::pos2(grip_se.min.x, grip_se.max.y), egui::pos2(grip_se.max.x, grip_se.min.y)],
        egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(0, 180, 255, 90)),
    );
    painter.line_segment(
        [egui::pos2(grip_se.min.x + 3.0, grip_se.max.y), egui::pos2(grip_se.max.x, grip_se.min.y + 3.0)],
        egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(0, 180, 255, 50)),
    );
}

pub fn apply_dwell_to_response(
    ui: &egui::Ui,
    dwell_state: &mut DwellState,
    id: egui::Id,
    response: &egui::Response,
) -> bool {
    if !dwell_state.is_enabled() {
        return response.clicked();
    }
    if response.clicked() {
        dwell_state.reset_hover(id);
        dwell_state.last_triggered_id = Some(id);
        dwell_state.last_triggered_time = Some(Instant::now());
        dwell_state.unhovered_since = None;
        return true;
    }
    let (triggered, progress) = dwell_state.update(id, response.hovered());
    if progress > 0.0 && progress <= 1.0 {
        let rect = response.rect;
        let bar_height = 3.5;
        let progress_rect = egui::Rect::from_min_max(
            egui::pos2(rect.left(), rect.bottom() - bar_height),
            egui::pos2(rect.left() + rect.width() * progress, rect.bottom()),
        );
        ui.painter().rect_filled(progress_rect, egui::CornerRadius::same(2), egui::Color32::from_rgb(76, 217, 100));
        ui.ctx().request_repaint();
    }
    triggered
}

pub fn get_screen_cursor_pos(ctx: &egui::Context) -> Option<egui::Pos2> {
    let (x, y) = get_global_cursor_pos()?;
    let ppp = ctx.pixels_per_point().max(0.5);
    Some(egui::pos2(x as f32 / ppp, y as f32 / ppp))
}

pub fn localized_speech(
    lang_code: &str,
    fr: &str,
    en: &str,
    es: &str,
    de: &str,
    it: &str,
    pt: &str,
    ru: &str,
    zh: &str,
    ja: &str,
) -> String {
    let prefix = lang_code.split(['-', '_', '.']).next().unwrap_or(lang_code).to_lowercase();
    match prefix.as_str() {
        "en" => en.to_string(),
        "es" => es.to_string(),
        "de" => de.to_string(),
        "it" => it.to_string(),
        "pt" => pt.to_string(),
        "ru" => ru.to_string(),
        "zh" => zh.to_string(),
        "ja" => ja.to_string(),
        _ => fr.to_string(),
    }
}

/// Met en forme la liste des suggestions rapides pour une annonce vocale fluide par le narrateur
pub fn format_suggestions_for_speech(suggestions: &[QuickSuggestionItem]) -> Option<String> {
    let valid_suggestions: Vec<String> = suggestions
        .iter()
        .filter_map(|sug| sug.to_display_string())
        .map(|s| {
            s.trim()
                .trim_start_matches(|c: char| !c.is_alphanumeric() && c != '(' && c != '[' && c != '#')
                .trim()
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .collect();

    if valid_suggestions.is_empty() {
        return None;
    }

    let locale = rust_i18n::locale();
    let intro = if valid_suggestions.len() == 1 {
        localized_speech(
            &locale,
            "Suggestion :",
            "Suggestion:",
            "Sugerencia:",
            "Vorschlag:",
            "Suggerimento:",
            "Sugestão:",
            "Предложение:",
            "建议：",
            "提案：",
        )
    } else {
        localized_speech(
            &locale,
            "Suggestions :",
            "Suggestions:",
            "Sugerencias:",
            "Vorschläge:",
            "Suggerimenti:",
            "Sugestões:",
            "Предложения:",
            "建议：",
            "提案：",
        )
    };

    let mut out = String::new();
    out.push_str(&intro);
    for item in valid_suggestions {
        out.push(' ');
        out.push_str(&item);
        if !item.ends_with('.') && !item.ends_with('!') && !item.ends_with('?') {
            out.push('.');
        }
    }
    Some(out)
}

pub fn setup_custom_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    #[cfg(windows)]
    {
        let sys_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
        let font_candidates = [
            format!(r"{sys_root}\Fonts\seguiemj.ttf"),
            format!(r"{sys_root}\Fonts\seguisym.ttf"),
            format!(r"{sys_root}\Fonts\msyh.ttc"),
            format!(r"{sys_root}\Fonts\msyh.ttf"),
            format!(r"{sys_root}\Fonts\yu-gothic.ttc"),
            format!(r"{sys_root}\Fonts\yugothm.ttc"),
            format!(r"{sys_root}\Fonts\meiryo.ttc"),
            format!(r"{sys_root}\Fonts\msgothic.ttc"),
            format!(r"{sys_root}\Fonts\malgun.ttf"),
            format!(r"{sys_root}\Fonts\simsun.ttc"),
        ];

        for (idx, path) in font_candidates.iter().enumerate() {
            if let Ok(font_data) = std::fs::read(path) {
                let font_name = format!("system_fallback_font_{idx}");
                fonts.font_data.insert(
                    font_name.clone(),
                    Arc::new(egui::FontData::from_owned(font_data)),
                );

                fonts
                    .families
                    .entry(egui::FontFamily::Proportional)
                    .or_default()
                    .push(font_name.clone());

                fonts
                    .families
                    .entry(egui::FontFamily::Monospace)
                    .or_default()
                    .push(font_name);
            }
        }
    }

    #[cfg(not(windows))]
    {
        let font_candidates = [
            "/System/Library/Fonts/PingFang.ttc",
            "/System/Library/Fonts/Hiragino Sans GB.ttc",
            "/System/Library/Fonts/Hiragino Sans.ttc",
            "/System/Library/Fonts/Supplemental/Songti.ttc",
            "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
            "/System/Library/Fonts/Apple Color Emoji.ttc",
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/System/Library/Fonts/Helvetica.ttc",
            "/System/Library/Fonts/SFNS.ttf",
            "/usr/share/fonts/truetype/noto/NotoColorEmoji.ttf",
            "/usr/share/fonts/noto/NotoColorEmoji.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
            "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
        ];
        for (idx, path) in font_candidates.iter().enumerate() {
            if let Ok(font_data) = std::fs::read(path) {
                let font_name = format!("system_fallback_font_{idx}");
                fonts.font_data.insert(font_name.clone(), Arc::new(egui::FontData::from_owned(font_data)));
                fonts.families.entry(egui::FontFamily::Proportional).or_default().push(font_name.clone());
                fonts.families.entry(egui::FontFamily::Monospace).or_default().push(font_name);
            }
        }
    }

    ctx.set_fonts(fonts);
}

pub fn current_time_str() -> String {
    #[cfg(windows)]
    unsafe {
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
        extern "system" {
            fn GetLocalTime(st: *mut SystemTimeWin);
        }
        let mut st = SystemTimeWin {
            year: 0, month: 0, day_of_week: 0, day: 0,
            hour: 0, minute: 0, second: 0, milliseconds: 0,
        };
        GetLocalTime(&mut st);
        format!("{:02}:{:02}", st.hour, st.minute)
    }
    #[cfg(not(windows))]
    {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
        format!("{:02}:{:02}", (secs / 3600) % 24, (secs / 60) % 60)
    }
}