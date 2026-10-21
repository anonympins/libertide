mod autostart;
mod audio;
mod automation;
mod floating_window;
mod fs_util;
mod html_util;
mod pattern;
mod scheduler;
mod theme;
mod tts;
mod twitch;
mod types;
mod window_util;
mod commands;
mod ui;

use audio::spawn_audio_worker;
use floating_window::{update_open_floating_cards, FloatingCard, OPEN_FLOATING_CARDS};
use automation::*;
use fs_util::*;
use pattern::clean_words;
use theme::{AppTheme, SettingsTab, ThemeColors};
use tts::{sanitize_for_tts, spawn_tts_worker};
use twitch::{search_twitch_channels, spawn_twitch_worker};
use window_util::*;
use eframe::egui;
use types::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender as StdSender};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use std::time::{Duration, Instant};

#[cfg(windows)]
use windows::Win32::Foundation::POINT;
#[cfg(windows)]
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

#[cfg(windows)]
extern "system" {
    fn Beep(dwFreq: u32, dwDuration: u32) -> i32;
}

#[derive(Clone, Copy, Debug)]
pub enum Earcon {
    MicOpen,
    MicClose,
    Success,
    Emergency,
}

pub fn play_earcon(earcon: Earcon) {
    #[cfg(windows)]
    std::thread::spawn(move || unsafe {
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
        }
    });
    #[cfg(target_os = "macos")]
    std::thread::spawn(move || {
        let sound_name = match earcon {
            Earcon::MicOpen => "Pop.aiff",
            Earcon::MicClose => "Tink.aiff",
            Earcon::Success => "Glass.aiff",
            Earcon::Emergency => "Sosumi.aiff",
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

pub struct OverlayApp {
    status: AgentStatus,
    chat_history: Vec<ChatEntry>,
    active_tab: ActiveTab,
    twitch_messages: Vec<TwitchMessage>,
    live_transcript: String,
    input_text: String,
    is_recording: bool,
    smoothed_audio_level: f32,
    continuous_mode: bool,
    dictation_mode: bool,
    zoom_factor: f32,
    voice_gender: VoiceGender,
    llm_config: LlmConfig,
    theme: AppTheme,
    settings_tab: SettingsTab,

    // Accessibilité : Ciblage par numéros & Mode contacteur unique (Switch Access)
    numbered_targets: Vec<NumberedElementTarget>,
    show_numbered_targets: bool,
    switch_access_mode: bool,
    switch_step: SwitchAccessStep,
    switch_windows: Vec<SwitchWindowTarget>,
    switch_zones: Vec<NavigationZone>,
    last_switch_input: Instant,
    switch_focus_index: usize,
    switch_last_tick: Instant,

    // Canaux de communication asynchrones
    event_receiver: Receiver<AgentEvent>,
    command_sender: UnboundedSender<AgentCommand>,
    tts_sender: StdSender<TtsCommand>,
    audio_sender: StdSender<AudioCommand>,
    twitch_channel_sender: StdSender<String>,
    current_twitch_channel: String,
    twitch_search_query: String,
    twitch_search_results: Vec<TwitchChannelItem>,
    position_initialized: bool,
    floating_cards: Vec<FloatingCard>,
    window_pos: Option<egui::Pos2>,
    window_bottom_y: Option<f32>,
    drag_offset: Option<egui::Vec2>,
    chat_height: f32,
    current_win_w: f32,
    current_win_h: f32,
    is_hidden: bool,
    mouse_passthrough: bool,

    // Boîte modale de configuration de la clé API
    show_api_key_modal: bool,
    api_key_input: String,
    model_input: String,
    show_api_key_plain: bool,
    selected_provider_modal: LlmProvider,
    api_key_error: Option<String>,

    // Suggestions d'applications en direct (PATH & Levenshtein)
    input_suggestions: Vec<String>,
    last_suggestion_query: String,
}

#[cfg(windows)]
fn get_screen_cursor_pos(ctx: &egui::Context) -> Option<egui::Pos2> {
    let mut pt = POINT::default();
    unsafe {
        if GetCursorPos(&mut pt).is_ok() {
            let ppp = ctx.pixels_per_point();
            if ppp > 0.0 {
                return Some(egui::pos2(pt.x as f32 / ppp, pt.y as f32 / ppp));
            }
        }
    }
    None
}

#[cfg(not(windows))]
fn get_screen_cursor_pos(ctx: &egui::Context) -> Option<egui::Pos2> {
    ctx.input(|i| i.pointer.latest_pos())
}

fn normalize_for_command_match(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<&str>>()
        .join(" ")
}
fn is_emergency_stop_command(text: &str) -> bool {
    let norm = normalize_for_command_match(text);
    if norm == "stop" || norm == "stopp" || norm == "стоп" || norm == "停止" || norm == "ストップ" {
        return true;
    }
    const TRIGGERS: &[&str] = &[
        // FR
        "arret d urgence", "arrêt d urgence", "arrete tout", "arrête tout", "stop tout", "urgence",
        // EN
        "emergency stop", "stop all", "stop everything", "abort", "halt", "freeze",
        // ES
        "parada de emergencia", "deten todo", "detén todo", "para todo", "emergencia", "alto ahi",
        // DE
        "not aus", "notaus", "alles anhalten", "alles stoppen", "notfall", "abbrechen",
        // IT
        "arresto d emergenza", "ferma tutto", "blocca tutto", "emergenza", "annulla tutto",
        // PT
        "parada de emergencia", "parada de emergência", "pare tudo", "parar tudo", "emergencia", "abortar",
        // RU
        "экстренная остановка", "останови все", "останови всё", "стоп все", "стоп всё", "аварийная остановка",
        // ZH
        "紧急停止", "全部停止", "立即停止", "停止运行", "中止",
        // JA
        "緊急停止", "すべて停止", "すべて止めて", "中止", "直ちに停止",
    ];
    TRIGGERS.iter().any(|&trig| norm.contains(trig) || text.contains(trig))
}

fn is_stop_dictation_command(text: &str) -> bool {
    let norm = normalize_for_command_match(text);
    const TRIGGERS: &[&str] = &[
        // FR
        "arrete la dictee", "arrête la dictée", "stop dictee", "stop dictée", "quitter la dictee", "quitter la dictée",
        "fin de la dictee", "fin de la dictée", "termine la dictee", "termine la dictée", "desactiver dictee", "désactiver dictée",
        // EN
        "stop dictation", "exit dictation", "quit dictation", "end dictation", "finish dictation", "disable dictation",
        // ES
        "detener dictado", "parar dictado", "salir del dictado", "terminar dictado", "desactivar dictado", "fin del dictado",
        // DE
        "diktat beenden", "diktat stoppen", "diktat anhalten", "diktat abbrechen", "diktat deaktivieren",
        // IT
        "interrompi dettatura", "ferma dettatura", "esci dalla dettatura", "termina dettatura", "disattiva dettatura",
        // PT
        "parar ditado", "encerrar ditado", "sair do ditado", "terminar ditado", "desativar ditado", "fim do ditado",
        // RU
        "остановить диктовку", "завершить диктовку", "выход из диктовки", "стоп диктовка", "отключить диктовку",
        // ZH
        "停止听写", "退出听写", "结束听写", "关闭听写",
        // JA
        "ディクテーション終了", "ディクテーション停止", "入力終了", "音声入力停止",
    ];
    TRIGGERS.iter().any(|&trig| norm.contains(trig) || text.contains(trig))
}

fn is_switch_access_command(text: &str) -> bool {
    let norm = normalize_for_command_match(text);
    const TRIGGERS: &[&str] = &[
        // FR
        "mode contacteur", "switch access", "mode survol", "survol contacteur", "contacteur", "survol",
        // EN
        "switch mode", "hover mode", "switch navigation",
        // ES
        "acceso por conmutador", "modo conmutador", "modo sobrevuelo", "conmutador",
        // DE
        "schalterzugriff", "schalter modus", "hover modus", "schalter",
        // IT
        "accesso a interruttori", "modalita cursore", "modalita interruttore", "interruttore",
        // PT
        "acesso por interruptor", "modo comutador", "comutador", "interruptor",
        // RU
        "переключатель", "режим переключателя",
        // ZH
        "切换控制", "开关控制", "悬停控制",
        // JA
        "スイッチアクセス", "スイッチ操作", "ホバーモード",
    ];
    TRIGGERS.iter().any(|&trig| norm.contains(trig) || text.contains(trig))
}

fn try_execute_local_navigation_shortcut(prompt: &str) -> Option<&'static str> {
    let norm = normalize_for_command_match(prompt);
    #[cfg(windows)]
    use windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY;

    match norm.as_str() {
        "clic" | "clic gauche" | "click" | "left click" => {
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
        "clic droit" | "right click" => {
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
        "double clic" | "double click" => {
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
                    .args([
                        "-e",
                        "tell application \"System Events\" to repeat 2 times\nclick\ndelay 0.05\nend repeat",
                    ])
                    .status();
            }
            #[cfg(all(not(windows), not(target_os = "macos")))]
            {
                let _ = std::process::Command::new("xdotool")
                    .args(["click", "--repeat", "2", "1"])
                    .status();
            }
            Some("Double clic.")
        }
        "enregistre" | "enregistrer" | "sauvegarde" | "sauvegarder" | "save" => {
            #[cfg(windows)]
            send_hotkey(&[VIRTUAL_KEY(0x11)], VIRTUAL_KEY(0x53)); // Ctrl + S
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("osascript")
                .args(["-e", "tell application \"System Events\" to keystroke \"s\" using command down"])
                .status();
            #[cfg(all(not(windows), not(target_os = "macos")))]
            let _ = std::process::Command::new("xdotool").args(["key", "ctrl+s"]).status();
            Some("Sauvegardé (Ctrl+S).")
        }
        "annule" | "annuler" | "undo" => {
            if let Some(msg) = undo_last_system_action() {
                println!("[Undo] Action compensatoire exécutée : {msg}");
            } else {
                #[cfg(windows)]
                send_hotkey(&[VIRTUAL_KEY(0x11)], VIRTUAL_KEY(0x5A)); // Ctrl + Z
                #[cfg(target_os = "macos")]
                let _ = std::process::Command::new("osascript").args(["-e", "tell application \"System Events\" to keystroke \"z\" using command down"]).status();
                #[cfg(all(not(windows), not(target_os = "macos")))]
                let _ = std::process::Command::new("xdotool").args(["key", "ctrl+z"]).status();
            }
            Some("Annulé.")
        }
        "selectionne tout" | "sélectionne tout" | "select all" | "tout selectionner" => {
            #[cfg(windows)]
            send_hotkey(&[VIRTUAL_KEY(0x11)], VIRTUAL_KEY(0x41)); // Ctrl + A
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("osascript")
                .args(["-e", "tell application \"System Events\" to keystroke \"a\" using command down"])
                .status();
            #[cfg(all(not(windows), not(target_os = "macos")))]
            let _ = std::process::Command::new("xdotool").args(["key", "ctrl+a"]).status();
            Some("Tout sélectionné.")
        }
        "entree" | "entrée" | "valide" | "valider" | "enter" => {
            #[cfg(windows)]
            send_hotkey(&[], VIRTUAL_KEY(0x0D));
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("osascript")
                .args(["-e", "tell application \"System Events\" to key code 36"])
                .status();
            #[cfg(all(not(windows), not(target_os = "macos")))]
            let _ = std::process::Command::new("xdotool").args(["key", "Return"]).status();
            Some("Touche Entrée validée.")
        }
        "tab" | "tabulation" | "suivant" | "champ suivant" => {
            #[cfg(windows)]
            send_hotkey(&[], VIRTUAL_KEY(0x09));
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("osascript")
                .args(["-e", "tell application \"System Events\" to key code 48"])
                .status();
            #[cfg(all(not(windows), not(target_os = "macos")))]
            let _ = std::process::Command::new("xdotool").args(["key", "Tab"]).status();
            Some("Tabulation.")
        }
        "precedent" | "précédent" | "champ precedent" | "champ précédent" => {
            #[cfg(windows)]
            send_hotkey(&[VIRTUAL_KEY(0x10)], VIRTUAL_KEY(0x09));
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("osascript")
                .args(["-e", "tell application \"System Events\" to key code 48 using shift down"])
                .status();
            #[cfg(all(not(windows), not(target_os = "macos")))]
            let _ = std::process::Command::new("xdotool").args(["key", "shift+Tab"]).status();
            Some("Champ précédent.")
        }
        "descends" | "descendre" | "page suivante" | "scroll down" => {
            #[cfg(windows)]
            send_hotkey(&[], VIRTUAL_KEY(0x22)); // VK_NEXT (Page Down)
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("osascript")
                .args(["-e", "tell application \"System Events\" to key code 121"])
                .status();
            #[cfg(all(not(windows), not(target_os = "macos")))]
            let _ = std::process::Command::new("xdotool").args(["key", "Page_Down"]).status();
            Some("Défilement vers le bas.")
        }
        "monte" | "monter" | "page precedente" | "page précédente" | "scroll up" => {
            #[cfg(windows)]
            send_hotkey(&[], VIRTUAL_KEY(0x21)); // VK_PRIOR (Page Up)
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("osascript")
                .args(["-e", "tell application \"System Events\" to key code 116"])
                .status();
            #[cfg(all(not(windows), not(target_os = "macos")))]
            let _ = std::process::Command::new("xdotool").args(["key", "Page_Up"]).status();
            Some("Défilement vers le haut.")
        }
        "copie" | "copier" | "copy" => {
            #[cfg(windows)]
            send_hotkey(&[VIRTUAL_KEY(0x11)], VIRTUAL_KEY(0x43)); // Ctrl + C
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("osascript")
                .args(["-e", "tell application \"System Events\" to keystroke \"c\" using command down"])
                .status();
            #[cfg(all(not(windows), not(target_os = "macos")))]
            let _ = std::process::Command::new("xdotool").args(["key", "ctrl+c"]).status();
            Some("Copié.")
        }
        "colle" | "coller" | "paste" => {
            #[cfg(windows)]
            send_hotkey(&[VIRTUAL_KEY(0x11)], VIRTUAL_KEY(0x56)); // Ctrl + V
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("osascript")
                .args(["-e", "tell application \"System Events\" to keystroke \"v\" using command down"])
                .status();
            #[cfg(all(not(windows), not(target_os = "macos")))]
            let _ = std::process::Command::new("xdotool").args(["key", "ctrl+v"]).status();
            Some("Collé.")
        }
        "retour" | "retour arriere" | "retour arrière" | "reculer" | "back" | "browser back" => {
            #[cfg(windows)]
            send_hotkey(&[VIRTUAL_KEY(0x12)], VIRTUAL_KEY(0x25)); // Alt + Gauche
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("osascript").args(["-e", "tell application \"System Events\" to keystroke \"[\" using command down"]).status();
            #[cfg(all(not(windows), not(target_os = "macos")))]
            let _ = std::process::Command::new("xdotool").args(["key", "alt+Left"]).status();
            Some("Page précédente (Retour arrière).")
        }
        "avance" | "avancer" | "forward" | "browser forward" => {
            #[cfg(windows)]
            send_hotkey(&[VIRTUAL_KEY(0x12)], VIRTUAL_KEY(0x27)); // Alt + Droite
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("osascript").args(["-e", "tell application \"System Events\" to keystroke \"]\" using command down"]).status();
            #[cfg(all(not(windows), not(target_os = "macos")))]
            let _ = std::process::Command::new("xdotool").args(["key", "alt+Right"]).status();
            Some("Page suivante.")
        }
        "omnibox" | "l omnibox" | "omnibar" | "l omnibar" | "barre d adresse" | "barre adresse" | "barre de recherche" | "adresse" | "address bar"
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

fn is_hide_command(text: &str) -> bool {
    let norm = normalize_for_command_match(text);
    const DIRECT_PHRASES: &[&str] = &[
        // FR
        "ferme toi", "cache toi", "masque toi", "disparais", "fermer overlay", "cacher overlay", "masquer overlay",
        // EN
        "hide yourself", "close yourself", "hide overlay", "close overlay", "disappear", "minimize overlay",
        // ES
        "ocultate", "ocúltate", "escondete", "escóndete", "ocultar overlay", "cerrar overlay", "desaparece",
        // DE
        "versteck dich", "verstecke dich", "overlay schliessen", "overlay schließen", "overlay verstecken", "verschwinde",
        // IT
        "nasconditi", "chiuditi", "nascondi overlay", "chiudi overlay", "sparisci",
        // PT
        "esconda se", "esconda-se", "oculte se", "oculte-se", "esconder overlay", "fechar overlay", "desapareca", "desapareça",
        // RU
        "скройся", "спрячься", "закройся", "скрыть оверлей", "исчезни",
        // ZH
        "隐藏", "隐藏自己", "退下", "关闭悬浮窗", "隐藏窗口",
        // JA
        "隠れて", "非表示", "消えて", "オーバーレイを隠して",
    ];
    if DIRECT_PHRASES.iter().any(|&p| norm.contains(p) || text.contains(p)) {
        return true;
    }
    let has_verb = norm.contains("ferme") || norm.contains("cache") || norm.contains("masque") || norm.contains("hide") || norm.contains("close");
    let has_target = norm.contains("overlay") || norm.contains("deepseek") || norm.contains("libertide");
    has_verb && has_target
}

fn is_show_command(text: &str) -> bool {
    let norm = normalize_for_command_match(text);
    const DIRECT_PHRASES: &[&str] = &[
        // FR
        "ouvre toi", "affiche toi", "montre toi", "reveille toi", "réveille toi", "ouvrir overlay", "afficher overlay",
        // EN
        "show yourself", "open up", "wake up", "appear", "open overlay", "show overlay",
        // ES
        "muestrate", "muéstrate", "abrete", "ábrete", "despierta", "aparece", "mostrar overlay", "abrir overlay",
        // DE
        "zeige dich", "offne dich", "öffne dich", "aufwachen", "erscheine", "overlay zeigen", "overlay offnen", "overlay öffnen",
        // IT
        "mostrati", "apriti", "svegliati", "mostra overlay", "apri overlay",
        // PT
        "mostre se", "mostre-se", "abra se", "abra-se", "acorde", "apareca", "apareça", "mostrar overlay", "abrir overlay",
        // RU
        "покажись", "откройся", "проснись", "открыть оверлей", "показать оверлей",
        // ZH
        "出来", "显示", "唤醒", "打开悬浮窗", "打开",
        // JA
        "出てきて", "表示して", "起きて", "オーバーレイを表示",
    ];
    if DIRECT_PHRASES.iter().any(|&p| norm.contains(p) || text.contains(p)) {
        return true;
    }
    let has_verb = norm.contains("ouvre") || norm.contains("affiche") || norm.contains("montre") || norm.contains("show") || norm.contains("open");
    let has_target = norm.contains("overlay") || norm.contains("deepseek") || norm.contains("libertide");
    has_verb && has_target
}

fn is_api_key_command(text: &str) -> bool {
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

fn localized_speech(lang_code: &str, fr: &str, en: &str, es: &str, de: &str, it: &str, pt: &str, ru: &str, zh: &str, ja: &str) -> String {
    match lang_code {
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

#[cfg(windows)]
fn restart_application() {
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

        // 1. Lancement via PowerShell : évite le conflit de dé-échappement de guillemets de cmd.exe
        // et détache proprement le nouveau processus tout en respectant le temps de libération du GPU/micro.
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

        // 2. Repli de secours via cmd.exe en utilisant raw_arg (sans ré-échappement de guillemets par Rust)
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
fn restart_application() {
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

/// Détecte la langue principale du système d'exploitation de l'utilisateur
pub fn get_system_locale_language() -> (String, String) {
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
    ("fr".to_string(), "français (French)".to_string())
}

fn setup_custom_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    #[cfg(windows)]
    {
        let sys_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
        let font_candidates = [
            format!(r"{sys_root}\Fonts\seguiemj.ttf"),
            format!(r"{sys_root}\Fonts\seguisym.ttf"),
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
            // Chemins macOS
            "/System/Library/Fonts/Apple Color Emoji.ttc",
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/System/Library/Fonts/Helvetica.ttc",
            "/System/Library/Fonts/SFNS.ttf",
            // Chemins Linux
            "/usr/share/fonts/truetype/noto/NotoColorEmoji.ttf",
            "/usr/share/fonts/noto/NotoColorEmoji.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        ];
        for (idx, path) in font_candidates.iter().enumerate() {
            if let Ok(font_data) = std::fs::read(path) {
                let font_name = format!("system_fallback_font_{idx}");
                fonts.font_data.insert(font_name.clone(), Arc::new(egui::FontData::from_owned(font_data)));
                fonts.families.entry(egui::FontFamily::Proportional).or_default().push(font_name.clone());
                fonts.families.entry(egui::FontFamily::Monospace).or_default().push(font_name);
                break;
            }
        }
    }

    ctx.set_fonts(fonts);
}

pub(crate) fn current_time_str() -> String {
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

impl OverlayApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        event_receiver: Receiver<AgentEvent>,
        command_sender: UnboundedSender<AgentCommand>,
        tts_sender: StdSender<TtsCommand>,
        audio_sender: StdSender<AudioCommand>,
        twitch_channel_sender: StdSender<String>,
        initial_twitch_channel: String,
        initial_voice: VoiceGender,
    ) -> Self {
        setup_custom_fonts(&cc.egui_ctx);

        let llm_config = resolve_llm_config();
        let theme = AppTheme::resolve_theme();
        let has_no_key = llm_config.api_key.is_empty();
        let initial_subtitle = if has_no_key {
            "Veuillez renseigner votre clé API LLM pour activer l'assistant.".to_string()
        } else {
            "En attente d'instructions d'exploration...".to_string()
        };
        let _ = tts_sender.send(TtsCommand::Speak(initial_subtitle.clone()));

        let mut app = Self {
            status: AgentStatus::Idle,
            chat_history: vec![ChatEntry {
                role: ChatRole::Agent,
                text: initial_subtitle,
                timestamp: current_time_str(),
                quick_suggestions: Vec::new(),
                screen_size_kb: None,
            }],
            active_tab: ActiveTab::Assistance,
            twitch_messages: Vec::new(),
            live_transcript: String::new(),
            input_text: String::new(),
            smoothed_audio_level: 0.0,
            is_recording: false,
            continuous_mode: true,
            dictation_mode: false,
            zoom_factor: 1.0,
            voice_gender: initial_voice,
            llm_config: llm_config.clone(),
            theme,
            settings_tab: SettingsTab::LlmProvider,

            numbered_targets: Vec::new(),
            show_numbered_targets: false,
            switch_access_mode: false,
            switch_step: SwitchAccessStep::Windows,
            switch_windows: Vec::new(),
            switch_zones: Vec::new(),
            last_switch_input: Instant::now() - Duration::from_secs(1),
            switch_focus_index: 0,
            switch_last_tick: Instant::now(),

            event_receiver,
            command_sender,
            tts_sender,
            audio_sender,
            twitch_channel_sender,
            current_twitch_channel: initial_twitch_channel,
            twitch_search_query: String::new(),
            twitch_search_results: Vec::new(),
            position_initialized: false,
            floating_cards: Vec::new(),
            window_pos: None,
            window_bottom_y: None,
            drag_offset: None,
            current_win_w: 560.0,
            chat_height: 70.0,
            current_win_h: 178.0,
            is_hidden: false,
            mouse_passthrough: false,
            show_api_key_modal: has_no_key,
            api_key_input: llm_config.api_key.clone(),
            model_input: llm_config.model.clone(),
            selected_provider_modal: llm_config.provider,
            show_api_key_plain: false,
            api_key_error: None,
            input_suggestions: Vec::new(),
            last_suggestion_query: String::new(),
        };
        app.sync_modal_inputs_for_provider(llm_config.provider);
        update_open_floating_cards(&app.floating_cards);
        app
    }

    pub fn sync_modal_inputs_for_provider(&mut self, provider: LlmProvider) {
        self.selected_provider_modal = provider;
        let key_name = provider.env_key_name();
        self.api_key_input = std::env::var(key_name)
            .or_else(|_| read_env_var_from_file(".env", key_name).ok_or(()))
            .unwrap_or_default();

        let model_var = provider.env_model_name();
        self.model_input = std::env::var(model_var)
            .ok()
            .or_else(|| read_env_var_from_file(".env", model_var))
            .or_else(|| std::env::var("LLM_MODEL").ok())
            .or_else(|| read_env_var_from_file(".env", "LLM_MODEL"))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| provider.default_model().to_string());
        self.api_key_error = None;
    }

    pub fn set_theme(&mut self, theme: AppTheme) {
        self.theme = theme;
        let _ = write_env_var_to_file(".env", "THEME", theme.key());
        std::env::set_var("THEME", theme.key());
    }

    fn draw_avatar(&self, ui: &mut egui::Ui, center: egui::Pos2, base_radius: f32, time: f64) {
        let painter = ui.painter();

        let pulse_speed = match self.status {
            AgentStatus::Idle => 1.6,
            AgentStatus::Thinking => 4.2,
            AgentStatus::Speaking | AgentStatus::Listening => 2.0 + 4.5 * (self.smoothed_audio_level as f64),
            AgentStatus::EmergencyStopped => 0.0,
        };

        let wave = if self.status == AgentStatus::Speaking || self.status == AgentStatus::Listening {
            self.smoothed_audio_level
        } else {
            ((time * pulse_speed).sin() as f32).max(0.0)
        };

        // Teinte rouge/corail en écoute micro, couleur d'accent du thème le reste du temps
        let theme = self.theme.colors();
        let (ring_base_color, core_color) = if self.status == AgentStatus::Listening {
            ([255, 65, 80], egui::Color32::from_rgb(255, 50, 70))
        } else {
            (theme.avatar_ring, theme.avatar_core)
        };

        // Empreinte sonore radiale (audio voiceprint spectrum)
        let num_bars = 16;
        let activity_level = match self.status {
            AgentStatus::Speaking | AgentStatus::Listening => self.smoothed_audio_level,
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

        let ring_alpha_mult = if self.status == AgentStatus::Speaking || self.status == AgentStatus::Listening {
            0.2 + 0.8 * self.smoothed_audio_level
        } else {
            1.0
        };

        // Cercles concentriques
        let rings = [
            (base_radius + 26.0 + (wave * 6.0), egui::Color32::from_rgba_unmultiplied(ring_base_color[0], ring_base_color[1], ring_base_color[2], (35.0 * ring_alpha_mult) as u8), 1.5),
            (base_radius + 18.0 + (wave * 4.0), egui::Color32::from_rgba_unmultiplied(ring_base_color[0], ring_base_color[1], ring_base_color[2], (60.0 * ring_alpha_mult) as u8), 1.8),
            (base_radius + 10.0 + (wave * 2.5), egui::Color32::from_rgba_unmultiplied(ring_base_color[0], ring_base_color[1], ring_base_color[2], (110.0 * ring_alpha_mult) as u8), 2.0),
            (base_radius + 2.0, egui::Color32::from_rgba_unmultiplied(ring_base_color[0].saturating_add(30), ring_base_color[1].saturating_add(30), ring_base_color[2], (180.0 * ring_alpha_mult) as u8), 2.2),
        ];

        for (radius, color, stroke_width) in rings {
            painter.circle_stroke(center, radius, egui::Stroke::new(stroke_width, color));
        }

        // Noyau central bleu
        painter.circle_filled(
            center,
            base_radius * (0.40 + 0.12 * activity_level),
            core_color,
        );
        painter.circle_filled(center, base_radius * (0.18 + 0.06 * activity_level), egui::Color32::WHITE);
    }

    fn draw_mini_equalizer(&self, ui: &mut egui::Ui, time: f64) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(28.0, 12.0), egui::Sense::hover());
        let painter = ui.painter();

        let num_bars = 5;
        let bar_width = 3.0;
        let spacing = 2.5;

        let theme = self.theme.colors();
        let (base_color, activity_mult) = match self.status {
            AgentStatus::Speaking => (theme.accent, self.smoothed_audio_level),
            AgentStatus::Listening => (egui::Color32::from_rgb(255, 75, 90), self.smoothed_audio_level),
            AgentStatus::Thinking => (egui::Color32::from_rgb(180, 120, 255), 0.55f32),
            AgentStatus::Idle => (theme.border, 0.15f32),
            AgentStatus::EmergencyStopped => (egui::Color32::from_rgb(220, 50, 50), 0.0f32),
        };

        for i in 0..num_bars {
            let x = rect.min.x + (i as f32) * (bar_width + spacing);
            let h = if self.status == AgentStatus::EmergencyStopped {
                2.0
            } else if self.status == AgentStatus::Speaking || self.status == AgentStatus::Listening {
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

    pub fn set_voice_gender(&mut self, gender: VoiceGender) {
        self.voice_gender = gender;
        let _ = self.tts_sender.send(TtsCommand::SetGender(gender));
        std::env::set_var("VOICE_PROFILE", gender.to_key_str());
        let _ = write_env_var_to_file(".env", "VOICE_PROFILE", gender.to_key_str());
        let confirmation = match gender {
            VoiceProfile::Denise => "Voix de Denise activée.",
            VoiceProfile::Henri => "Voix d'Henri activée.",
            VoiceProfile::Hortense => "Voix d'Hortense activée.",
            VoiceProfile::Paul => "Voix de Paul activée.",
            VoiceProfile::Sylvie => "Voix de Sylvie activée.",
            VoiceProfile::Jean => "Voix de Jean activée.",
        };
        self.chat_history.push(ChatEntry {
            role: ChatRole::Agent,
            text: confirmation.to_string(),
            timestamp: current_time_str(),
            quick_suggestions: Vec::new(),
            screen_size_kb: None,
        });
        let _ = self.tts_sender.send(TtsCommand::Speak(confirmation.to_string()));
    }

    pub fn stop_speaking(&mut self) {
        let _ = self.tts_sender.send(TtsCommand::Stop);
        set_tts_audio_level(0.0);
        self.smoothed_audio_level = 0.0;
        if self.status == AgentStatus::Speaking {
            self.status = AgentStatus::Idle;
        }
    }

    fn trigger_emergency_stop(&mut self, _ctx: &egui::Context) {
        self.continuous_mode = false;
        self.dictation_mode = false;
        IS_DICTATION_ACTIVE.store(false, Ordering::SeqCst);
        cancel_all_scheduled_tasks();
        IS_EMERGENCY_STOPPED.store(true, Ordering::SeqCst);
        TARGET_REASONING_PASSES.store(5, Ordering::SeqCst);
        let _ = CURRENT_REQUEST_ID.fetch_add(1, Ordering::SeqCst);
        if self.is_recording {
            self.stop_recording();
        }
        self.live_transcript.clear();
        self.is_hidden = false;
        self.status = AgentStatus::EmergencyStopped;
        play_earcon(Earcon::Emergency);
        let (locale, _) = get_system_locale_language();
        let lang_prefix = locale.split(['-', '_']).next().unwrap_or("fr");
        let stop_msg = localized_speech(
            lang_prefix,
            "Arrêt d'urgence : contrôle rendu à l'utilisateur.",
            "Emergency stop: control returned to the user.",
            "Parada de emergencia: control devuelto al usuario.",
            "Not-Aus: Kontrolle an den Benutzer zurückgegeben.",
            "Arresto di emergenza: controllo restituito all'utente.",
            "Parada de emergência: controle retornado ao usuário.",
            "Экстренная остановка: управление возвращено пользователю.",
            "紧急停止：控制权已交还用户。",
            "緊急停止：操作権がユーザーに返却されました。",
        );
        self.chat_history.push(ChatEntry {
            role: ChatRole::Agent,
            text: stop_msg.clone(),
            timestamp: current_time_str(),
            quick_suggestions: Vec::new(),
            screen_size_kb: None,
        });
        let _ = self.tts_sender.send(TtsCommand::Stop);
        let _ = self.tts_sender.send(TtsCommand::Speak(stop_msg));
    }

    fn exit_dictation_mode(&mut self) {
        self.dictation_mode = false;
        self.continuous_mode = false;
        IS_DICTATION_ACTIVE.store(false, Ordering::SeqCst);
        play_earcon(Earcon::Success);
        let _ = CURRENT_REQUEST_ID.fetch_add(1, Ordering::SeqCst);
        if self.is_recording {
            self.stop_recording();
        }
        self.live_transcript.clear();
        self.status = AgentStatus::Idle;
        let _ = self.tts_sender.send(TtsCommand::Stop);
        let (locale, _) = get_system_locale_language();
        let lang_prefix = locale.split(['-', '_']).next().unwrap_or("fr");
        let msg = localized_speech(
            lang_prefix,
            "Mode dictée désactivé.",
            "Dictation mode disabled.",
            "Modo dictado desactivado.",
            "Diktatmodus deaktiviert.",
            "Modalità dettatura disattivata.",
            "Modo ditado desativado.",
            "Режим диктовки отключен.",
            "听写模式已停用。",
            "音声入力モードを解除しました。",
        );
        self.chat_history.push(ChatEntry {
            role: ChatRole::Agent,
            text: msg.clone(),
            timestamp: current_time_str(),
            quick_suggestions: Vec::new(),
            screen_size_kb: None,
        });
        let _ = self.tts_sender.send(TtsCommand::Speak(msg));
    }

    pub fn toggle_switch_access_mode(&mut self) {
        self.switch_access_mode = !self.switch_access_mode;
        let (locale, _) = get_system_locale_language();
        let lang_prefix = locale.split(['-', '_']).next().unwrap_or("fr");

        if self.switch_access_mode {
            self.switch_step = SwitchAccessStep::Windows;
            self.switch_windows = collect_switch_candidate_windows();
            self.numbered_targets.clear();
            self.switch_focus_index = 0;
            self.switch_last_tick = Instant::now() - Duration::from_millis(1500);
            play_earcon(Earcon::Success);
            let count = self.switch_windows.len();
            let msg = if count > 0 {
                localized_speech(
                    lang_prefix,
                    &format!("Mode contacteur hiérarchique activé. Étape 1 : {} fenêtres détectées. Appuyez sur Entrée ou Espace pour sélectionner.", count),
                    &format!("Hierarchical switch access enabled. Step 1: {} windows detected. Press Enter or Space to select.", count),
                    &format!("Acceso por conmutador jerárquico activado. Paso 1: {} ventanas detectadas. Presione Entrar o Espacio para seleccionar.", count),
                    &format!("Hierarchischer Schalterzugriff aktiviert. Schritt 1: {} Fenster erkannt. Drücken Sie Eingabe oder Leertaste zur Auswahl.", count),
                    &format!("Accesso a interruttori gerarchico attivato. Passaggio 1: {} finestre rilevate. Premi Invio o Spazio per selezionare.", count),
                    &format!("Acesso por interruptor hierárquico ativado. Etapa 1: {} janelas detectadas. Pressione Enter ou Espaço para selecionar.", count),
                    &format!("Иерархический режим переключателя включен. Шаг 1: обнаружено окон: {}. Нажмите Enter или Пробел для выбора.", count),
                    &format!("分层开关控制已启用。第1步：检测到 {} 个窗口。按 Enter 或空格键选择。", count),
                    &format!("階層スイッチアクセスが有効になりました。ステップ1：{}個のウィンドウを検出。EnterまたはSpaceで選択。", count),
                )
            } else {
                localized_speech(
                    lang_prefix,
                    "Mode contacteur hiérarchique activé. Aucune fenêtre candidate détectée.",
                    "Hierarchical switch access enabled. No candidate windows detected.",
                    "Acceso por conmutador jerárquico activado. No se detectaron ventanas candidatas.",
                    "Hierarchischer Schalterzugriff aktiviert. Keine Zielfenster erkannt.",
                    "Accesso a interruttori gerarchico attivato. Nessuna finestra rilevata.",
                    "Acesso por interruptor hierárquico ativado. Nenhuma janela detectada.",
                    "Иерархический режим переключателя включен. Подходящих окон не найдено.",
                    "分层开关控制已启用。未检测到候选窗口。",
                    "階層スイッチアクセスが有効になりました。対象ウィンドウが見つかりません。",
                )
            };
            self.chat_history.push(ChatEntry {
                role: ChatRole::Agent,
                text: msg.clone(),
                timestamp: current_time_str(),
                quick_suggestions: Vec::new(),
                screen_size_kb: None,
            });
            let _ = self.tts_sender.send(TtsCommand::Speak(msg));
        } else {
            self.numbered_targets.clear();
            self.switch_windows.clear();
            self.switch_zones.clear();
            self.switch_step = SwitchAccessStep::Windows;
            play_earcon(Earcon::MicClose);
            let msg = localized_speech(
                lang_prefix,
                "Mode survol et contacteur désactivé.",
                "Hover and switch access mode disabled.",
                "Modo sobrevuelo y conmutador desactivado.",
                "Hover- und Schalterzugriffsmodus deaktiviert.",
                "Modalità cursore e interruttore disattivata.",
                "Modo de sobrevoo e interruptor desativado.",
                "Режим наведения и переключателя отключен.",
                "悬停与开关控制模式已停用。",
                "ホバーおよびスイッチアクセスモードを解除しました。",
            );
            self.chat_history.push(ChatEntry {
                role: ChatRole::Agent,
                text: msg.clone(),
                timestamp: current_time_str(),
                quick_suggestions: Vec::new(),
                screen_size_kb: None,
            });
            let _ = self.tts_sender.send(TtsCommand::Speak(msg));
        }
    }

    fn start_recording(&mut self, _ctx: &egui::Context, play_sound: bool) {
        if self.status == AgentStatus::EmergencyStopped {
            return;
        }
        self.stop_speaking();
        IS_EMERGENCY_STOPPED.store(false, Ordering::SeqCst);
        self.continuous_mode = true;
        self.is_recording = true;
        self.status = AgentStatus::Listening;
        if play_sound {
            play_earcon(Earcon::MicOpen);
        }
        self.live_transcript.clear();
        if self.dictation_mode {
            restore_external_focus();
        }
        let _ = self.audio_sender.send(AudioCommand::Start);
    }

    fn stop_recording(&mut self) {
        self.is_recording = false;
        self.continuous_mode = false;
        let _ = self.audio_sender.send(AudioCommand::Stop);
        play_earcon(Earcon::MicClose);
        set_mic_audio_level(0.0);
        self.live_transcript.clear();
        self.status = AgentStatus::Idle;
    }

    fn toggle_recording(&mut self, ctx: &egui::Context) {
        if self.is_recording {
            self.stop_recording();
        } else {
            self.start_recording(ctx, true);
        }
    }
}

impl eframe::App for OverlayApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        if self.is_hidden {
            [0.0, 0.0, 0.0, 0.0]
        } else {
            self.theme.colors().clear_color
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        ctx.set_zoom_factor(self.zoom_factor);

        update_last_external_focus();

        self.dictation_mode = IS_DICTATION_ACTIVE.load(Ordering::SeqCst);

        let target_level = match self.status {
            AgentStatus::Speaking => get_tts_audio_level(),
            AgentStatus::Listening => get_mic_audio_level(),
            _ => 0.0,
        };

        if self.status == AgentStatus::Speaking || self.status == AgentStatus::Listening {
            if target_level > self.smoothed_audio_level {
                self.smoothed_audio_level += (target_level - self.smoothed_audio_level) * 0.65;
            } else {
                self.smoothed_audio_level += (target_level - self.smoothed_audio_level) * 0.22;
            }
        } else {
            self.smoothed_audio_level += (0.0 - self.smoothed_audio_level) * 0.25;
        }

        let theme = self.theme.colors();

        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = egui::Color32::TRANSPARENT;
        visuals.window_fill = egui::Color32::TRANSPARENT;
        visuals.extreme_bg_color = theme.control_bg;
        visuals.selection.bg_fill = theme.tab_active_bg;
        visuals.selection.stroke = egui::Stroke::new(1.0, theme.accent);
        visuals.widgets.active.bg_fill = theme.tab_active_bg;
        visuals.widgets.active.weak_bg_fill = theme.tab_active_bg;
        visuals.widgets.hovered.weak_bg_fill = theme.tab_inactive_bg;
        visuals.widgets.inactive.weak_bg_fill = theme.tab_inactive_bg;
        ctx.set_visuals(visuals);

        let ppp = ctx.pixels_per_point().max(0.5);
        #[cfg(windows)]
        let (work_x, work_y, work_w, work_h, taskbar_top) = {
            let wa = get_desktop_work_area();
            let left = wa.left as f32 / ppp;
            let top = wa.top as f32 / ppp;
            let right = wa.right as f32 / ppp;
            let bottom = wa.bottom as f32 / ppp;
            (left, top, (right - left).max(380.0), (bottom - top).max(200.0), bottom)
        };
        #[cfg(not(windows))]
        let (work_x, work_y, work_w, work_h, taskbar_top) = {
            let mon = ctx.input(|i| i.viewport().monitor_size).unwrap_or(egui::vec2(1920.0, 1080.0));
            (0.0, 0.0, mon.x, mon.y, mon.y)
        };

        let min_win_w: f32 = 380.0;
        let min_win_h: f32 = 158.0;
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(egui::vec2(min_win_w, min_win_h)));

        let inner_rect_opt = ctx.input(|i| i.viewport().inner_rect);
        let win_w: f32 = inner_rect_opt.map_or(self.current_win_w, |r| r.width().max(min_win_w));
        self.current_win_w = win_w;

        let trimmed_input = self.input_text.trim();
        if trimmed_input != self.last_suggestion_query {
            self.last_suggestion_query = trimmed_input.to_string();
            self.input_suggestions = if trimmed_input.len() >= 2 {
                find_matching_applications(trimmed_input, 5)
            } else {
                Vec::new()
            };
        }

        let padding: f32 = 2.0;
        let has_suggestions = !self.input_suggestions.is_empty();
        let control_h: f32 = if has_suggestions { 128.0 } else { 102.0 };

        let max_chat_h = (work_h - control_h - 60.0).clamp(180.0, 560.0);
        let min_chat_h: f32 = 50.0;
        let current_chat_h = self.chat_height.clamp(min_chat_h, max_chat_h);

        // Arrondi pour éviter les oscillations de conversion DPI sub-pixel
        let modal_min_h = if self.show_api_key_modal { 320.0 } else { min_win_h };
        let target_win_h = (padding + current_chat_h + padding + control_h + padding).max(modal_min_h).round();
        let active_win_h = inner_rect_opt.map_or(self.current_win_h, |r| r.height().max(min_win_h));

        // Ancrage automatique de la fenêtre ferrée à la barre des tâches en bas à droite
        if !self.position_initialized {
            let margin_x = 16.0;
            let init_h = target_win_h;
            let bot_y = taskbar_top; // Ferrée directement au sommet de la barre des tâches
            let target_pos = egui::pos2(
                (work_x + work_w - win_w - margin_x).max(work_x),
                (bot_y - init_h).max(work_y),
            );
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(win_w, init_h)));
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(target_pos));
            self.window_pos = Some(target_pos);
            self.window_bottom_y = Some(bot_y);
            self.current_win_h = init_h;
            self.position_initialized = true;
        }

        // Redimensionnement dynamique avec hystérésis (seuil >= 3.0 px)
        let is_maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        if self.position_initialized && !is_maximized && (target_win_h - self.current_win_h).abs() >= 3.0 {
            self.current_win_h = target_win_h;
            let bot_y = self.window_bottom_y.unwrap_or(taskbar_top);
            let cur_x = self.window_pos.map_or(work_x + work_w - win_w - 16.0, |p| p.x);
            let new_y = (bot_y - target_win_h).clamp(work_y, taskbar_top - min_win_h);
            let new_pos = egui::pos2(cur_x, new_y);
            self.window_pos = Some(new_pos);
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(win_w, target_win_h)));
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(new_pos));
        }

        // Alignement strict : panneau de commandes calé en bas, boîte de discussion au-dessus
        let control_top = (active_win_h - padding - control_h).max(padding + min_chat_h);
        let control_rect = egui::Rect::from_min_size(
            egui::pos2(padding, control_top),
            egui::vec2(win_w - 2.0 * padding, control_h),
        );
        let response_rect = egui::Rect::from_min_max(
            egui::pos2(padding, padding),
            egui::pos2(win_w - padding, (control_rect.min.y - padding).max(padding + min_chat_h)),
        );

        let should_passthrough = self.is_hidden;
        if should_passthrough != self.mouse_passthrough {
            self.mouse_passthrough = should_passthrough;
            ctx.send_viewport_cmd(egui::ViewportCommand::MousePassthrough(should_passthrough));
        }

        // Dépouillement des événements asynchrones
        while let Ok(event) = self.event_receiver.try_recv() {
            match event {
                AgentEvent::StatusChanged(new_status) => {
                    if self.status != AgentStatus::EmergencyStopped || new_status == AgentStatus::EmergencyStopped {
                        let prev_status = self.status;
                        self.status = new_status;
                        if new_status == AgentStatus::Idle {
                            self.is_recording = false;
                            if (self.dictation_mode
                                || (prev_status == AgentStatus::Thinking && self.continuous_mode))
                                && self.status != AgentStatus::EmergencyStopped
                                && !self.is_hidden
                                && !AGENT_BUSY.load(Ordering::SeqCst)
                            {
                                self.start_recording(&ctx, false);
                            }
                        }
                    }
                }
                AgentEvent::ScreenPayloadSize(kb) => {
                    if let Some(entry) = self.chat_history.iter_mut().rev().find(|e| e.role == ChatRole::User) {
                        entry.screen_size_kb = Some(kb);
                    }
                }
                AgentEvent::NarrationChunk(chunk) => {
                    if let Some(last) = self.chat_history.last_mut() {
                        if matches!(last.role, ChatRole::Agent) {
                            last.text.push_str(&chunk);
                        } else {
                            self.chat_history.push(ChatEntry {
                                role: ChatRole::Agent,
                                text: chunk,
                                timestamp: current_time_str(),
                                quick_suggestions: Vec::new(),
                                screen_size_kb: None,
                            });
                        }
                    } else {
                        self.chat_history.push(ChatEntry {
                            role: ChatRole::Agent,
                            text: chunk,
                            timestamp: current_time_str(),
                            quick_suggestions: Vec::new(),
                            screen_size_kb: None,
                        });
                    }
                }
                AgentEvent::ReplaceNarration { text, quick_suggestions } => {
                    if text.trim().is_empty() && quick_suggestions.is_empty() {
                        continue;
                    }
                    if !sanitize_for_tts(&text).trim().is_empty() {
                        self.status = AgentStatus::Speaking;
                    }
                    self.chat_history.push(ChatEntry {
                        role: ChatRole::Agent,
                        text: text.clone(),
                        timestamp: current_time_str(),
                        quick_suggestions,
                        screen_size_kb: None,
                    });
                    let _ = self.tts_sender.send(TtsCommand::Speak(text));
                }
                AgentEvent::SilentNarration(full_text) => {
                    self.chat_history.push(ChatEntry {
                        role: ChatRole::Agent,
                        text: full_text,
                        timestamp: current_time_str(),
                        quick_suggestions: Vec::new(),
                        screen_size_kb: None,
                    });
                }
                AgentEvent::TranscriptionPartial(text) => {
                    self.stop_speaking();
                    if self.is_recording && !self.is_hidden {
                        self.live_transcript = text;
                    }
                }
                AgentEvent::VoicePromptReady(prompt) => {
                    self.is_recording = false;
                    self.live_transcript.clear();
                    let is_user_typing = ctx.memory(|m| m.has_focus(egui::Id::new("prompt_input_text")))
                        && !self.input_text.trim().is_empty();
                    if is_user_typing {
                        // L'utilisateur est activement en train de taper au clavier : on préserve son texte
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }
                    self.stop_speaking();

                    let norm_prompt = normalize_for_command_match(&prompt);
                    if norm_prompt.trim().is_empty() {
                        println!("[Voice] Bruit parasite ignoré (aucun mot détecté) : aucune requête LLM.");
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }

                    // Changement de thème d'accessibilité à la voix
                    if norm_prompt.contains("theme suivant") || norm_prompt.contains("thème suivant") || norm_prompt.contains("change de theme") || norm_prompt.contains("change de thème") || norm_prompt.contains("changer de theme") || norm_prompt.contains("changer de thème") {
                        play_earcon(Earcon::Success);
                        self.set_theme(self.theme.next());
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }
                    if norm_prompt.contains("contraste eleve") || norm_prompt.contains("contraste élevé") || norm_prompt.contains("high contrast") {
                        play_earcon(Earcon::Success);
                        self.set_theme(AppTheme::HighContrastYellow);
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }
                    if norm_prompt.contains("theme sombre") || norm_prompt.contains("thème sombre") || norm_prompt.contains("theme par defaut") || norm_prompt.contains("thème par défaut") {
                        play_earcon(Earcon::Success);
                        self.set_theme(AppTheme::CyberCyan);
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }

                    // Changement de voix à la voix
                    if norm_prompt.contains("voix suivante") || norm_prompt.contains("autre voix") || norm_prompt.contains("change de voix") || norm_prompt.contains("changer voix") || norm_prompt.contains("changer de voix") {
                        play_earcon(Earcon::Success);
                        self.set_voice_gender(self.voice_gender.next());
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }
                    if norm_prompt.contains("voix de denise") || norm_prompt == "denise" || norm_prompt.contains("voix feminine") || norm_prompt.contains("voix féminine") {
                        play_earcon(Earcon::Success);
                        self.set_voice_gender(VoiceProfile::Denise);
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }
                    if norm_prompt.contains("voix d henri") || norm_prompt.contains("voix d'henri") || norm_prompt == "henri" || norm_prompt.contains("voix masculine") {
                        play_earcon(Earcon::Success);
                        self.set_voice_gender(VoiceProfile::Henri);
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }
                    if norm_prompt.contains("voix d hortense") || norm_prompt.contains("voix d'hortense") || norm_prompt.contains("voix de hortense") || norm_prompt == "hortense" {
                        play_earcon(Earcon::Success);
                        self.set_voice_gender(VoiceProfile::Hortense);
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }
                    if norm_prompt.contains("voix de paul") || norm_prompt == "paul" {
                        play_earcon(Earcon::Success);
                        self.set_voice_gender(VoiceProfile::Paul);
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }
                    if norm_prompt.contains("voix de sylvie") || norm_prompt == "sylvie" || norm_prompt.contains("voix quebecoise") || norm_prompt.contains("voix québécoise") || norm_prompt.contains("voix canadienne") {
                        play_earcon(Earcon::Success);
                        self.set_voice_gender(VoiceProfile::Sylvie);
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }
                    if norm_prompt.contains("voix de jean") || norm_prompt == "jean" {
                        play_earcon(Earcon::Success);
                        self.set_voice_gender(VoiceProfile::Jean);
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }

                    // Traitement du zoom adaptatif vocal
                    if norm_prompt.contains("grossis") || norm_prompt.contains("zoom plus") || norm_prompt.contains("agrandis") || norm_prompt.contains("zoom in") {
                        self.zoom_factor = (self.zoom_factor + 0.15).clamp(0.7, 2.5);
                        play_earcon(Earcon::Success);
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }
                    if norm_prompt.contains("zoom moins") || norm_prompt.contains("retrecis") || norm_prompt.contains("rétrécis") || norm_prompt.contains("zoom out") {
                        self.zoom_factor = (self.zoom_factor - 0.15).clamp(0.7, 2.5);
                        play_earcon(Earcon::Success);
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }
                    if norm_prompt.contains("zoom normal") || norm_prompt.contains("taille normale") || norm_prompt.contains("reset zoom") {
                        self.zoom_factor = 1.0;
                        play_earcon(Earcon::Success);
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }

                    // Bascule / activation du ciblage par numéros
                    if norm_prompt == "numeros" || norm_prompt == "numéros" || norm_prompt == "badges" || norm_prompt == "afficher les numeros" || norm_prompt == "click by number" {
                        self.numbered_targets = collect_numbered_targets_for_active_window();
                        self.show_numbered_targets = !self.numbered_targets.is_empty();
                        play_earcon(Earcon::Success);
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }

                    // Sélection directe d'un badge numéroté
                    if self.show_numbered_targets {
                        let parsed_idx = match norm_prompt.as_str() {
                            "un" | "1" => Some(1),
                            "deux" | "2" => Some(2),
                            "trois" | "3" => Some(3),
                            "quatre" | "4" => Some(4),
                            "cinq" | "5" => Some(5),
                            "six" | "6" => Some(6),
                            "sept" | "7" => Some(7),
                            "huit" | "8" => Some(8),
                            "neuf" | "9" => Some(9),
                            "dix" | "10" => Some(10),
                            other => other.split_whitespace().find_map(|s| s.parse::<usize>().ok()),
                        };
                        if let Some(idx) = parsed_idx {
                            if let Some(target) = self.numbered_targets.iter().find(|t| t.index == idx) {
                                #[cfg(windows)]
                                unsafe {
                                    windows::Win32::UI::WindowsAndMessaging::SetCursorPos(target.click_x, target.click_y);
                                    std::thread::sleep(Duration::from_millis(30));
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
                                    let script = format!(
                                        r#"tell application "System Events" to click at {{{}, {}}}"#,
                                        target.click_x, target.click_y
                                    );
                                    let _ = std::process::Command::new("osascript").args(["-e", &script]).status();
                                }
                                #[cfg(all(not(windows), not(target_os = "macos")))]
                                {
                                    let _ = std::process::Command::new("xdotool")
                                        .args(["mousemove", &target.click_x.to_string(), &target.click_y.to_string(), "click", "1"])
                                        .status();
                                }
                                play_earcon(Earcon::Success);
                                self.show_numbered_targets = false;
                                self.numbered_targets.clear();
                                if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                                    self.start_recording(&ctx, false);
                                }
                                return;
                            }
                        }
                    }

                    if is_switch_access_command(&prompt) {
                        self.toggle_switch_access_mode();
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }

                    let (locale, _) = get_system_locale_language();
                    let lang_prefix = locale.split(['-', '_']).next().unwrap_or("fr");

                    // 1. Arrêt d'urgence local universel (<50ms, sans requête LLM)
                    if is_emergency_stop_command(&prompt) {
                        self.trigger_emergency_stop(&ctx);
                        return;
                    }

                    // Annulation instantanée des commandes différées
                    if is_cancel_scheduled_command(&prompt) {
                        let cancelled = cancel_all_scheduled_tasks();
                        play_earcon(Earcon::Success);
                        let reply = if cancelled > 0 {
                            format!("Toutes les commandes différées ont été annulées ({cancelled} annulée{}).", if cancelled > 1 { "s" } else { "" })
                        } else {
                            "Toutes les commandes différées ont été annulées.".to_string()
                        };
                        self.chat_history.push(ChatEntry {
                            role: ChatRole::User,
                            text: prompt,
                            timestamp: current_time_str(),
                            quick_suggestions: Vec::new(),
                            screen_size_kb: None,
                        });
                        self.chat_history.push(ChatEntry {
                            role: ChatRole::Agent,
                            text: reply.clone(),
                            timestamp: current_time_str(),
                            quick_suggestions: Vec::new(),
                            screen_size_kb: None,
                        });
                        let _ = self.tts_sender.send(TtsCommand::Speak(reply));
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }

                // Détection vocale d'annulation prioritaire (Arrêt du job en cours + Rollback)
                if norm_prompt == "annule" || norm_prompt == "annuler" || norm_prompt == "undo" || norm_prompt == "reviens en arriere" || norm_prompt == "reviens en arrière" {
                    let _ = CURRENT_REQUEST_ID.fetch_add(1, Ordering::SeqCst);
                    let undo_feedback = if let Some(fb) = undo_last_system_action() {
                        fb
                    } else {
                        #[cfg(windows)]
                        send_hotkey(&[windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0x11)], windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0x5A));
                        #[cfg(not(windows))]
                        let _ = std::process::Command::new("xdotool").args(["key", "ctrl+z"]).status();
                        "Dernière action annulée.".to_string()
                    };
                    play_earcon(Earcon::Success);
                    self.chat_history.push(ChatEntry {
                        role: ChatRole::User,
                        text: prompt,
                        timestamp: current_time_str(),
                        quick_suggestions: Vec::new(),
                        screen_size_kb: None,
                    });
                    self.chat_history.push(ChatEntry {
                        role: ChatRole::Agent,
                        text: undo_feedback.clone(),
                        timestamp: current_time_str(),
                        quick_suggestions: Vec::new(),
                        screen_size_kb: None,
                    });
                    let _ = self.tts_sender.send(TtsCommand::Speak(undo_feedback));
                    self.status = AgentStatus::Idle;
                    return;
                }

                    // 2. Sortie déterministe immédiate du mode dictée
                    if self.dictation_mode && is_stop_dictation_command(&prompt) {
                        self.exit_dictation_mode();
                        return;
                    }

                    // 2 bis. Exécution déterministe locale instantanée des commandes de base (< 20 ms, 0 token LLM)
                    if let Some(feedback) = try_execute_local_navigation_shortcut(&prompt) {
                        play_earcon(Earcon::Success);
                        self.chat_history.push(ChatEntry {
                            role: ChatRole::User,
                            text: prompt,
                            timestamp: current_time_str(),
                            quick_suggestions: Vec::new(),
                            screen_size_kb: None,
                        });
                        if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                        return;
                    }

                    // 3. Configuration de la clé API
                    if is_api_key_command(&prompt) {
                        self.sync_modal_inputs_for_provider(self.llm_config.provider);
                        self.show_api_key_modal = true;
                        self.is_hidden = false;
                        let reply = localized_speech(
                            lang_prefix,
                            "Boîte de configuration de la clé API ouverte.",
                            "API key configuration dialog opened.",
                            "Ventana de configuración de clave API abierta.",
                            "API-Schlüssel-Konfigurationsdialog geöffnet.",
                            "Finestra di configurazione chiave API aperta.",
                            "Caixa de configuração da chave de API aberta.",
                            "Диалог настройки API-ключа открыт.",
                            "API密钥配置窗口已打开。",
                            "APIキー設定ダイアログを開きました。",
                        );
                        self.chat_history.push(ChatEntry {
                            role: ChatRole::Agent,
                            text: reply.clone(),
                            timestamp: current_time_str(),
                            quick_suggestions: Vec::new(),
                            screen_size_kb: None,
                        });
                        let _ = self.tts_sender.send(TtsCommand::Speak(reply));
                        self.continuous_mode = false;
                        continue;
                    }

                    // 4. Masquage / Réaffichage de l'overlay
                    if self.is_hidden {
                        if is_show_command(&prompt) {
                            self.is_hidden = false;
                            self.status = AgentStatus::Idle;
                            self.chat_history.push(ChatEntry {
                                role: ChatRole::User,
                                text: prompt,
                                timestamp: current_time_str(),
                                quick_suggestions: Vec::new(),
                                screen_size_kb: None,
                            });
                            let reply = localized_speech(
                                lang_prefix,
                                "Me revoilà, overlay réaffiché.",
                                "I'm back, overlay restored.",
                                "Aquí estoy de nuevo, interfaz mostrada.",
                                "Hier bin ich wieder, Overlay eingeblendet.",
                                "Eccomi di nuovo, interfaccia ripristinata.",
                                "Estou de volta, overlay restaurado.",
                                "Снова здесь, оверлей отображён.",
                                "我回来了，悬浮窗已恢复显示。",
                                "お待たせしました、オーバーレイを再表示しました。",
                            );
                            self.chat_history.push(ChatEntry {
                                role: ChatRole::Agent,
                                text: reply.clone(),
                                timestamp: current_time_str(),
                                quick_suggestions: Vec::new(),
                                screen_size_kb: None,
                            });
                            let _ = self.tts_sender.send(TtsCommand::Speak(reply));
                            self.continuous_mode = true;
                        } else if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            // Tout autre message est ignoré lorsque l'overlay est masqué
                            self.start_recording(&ctx, false);
                        }
                    } else {
                        if is_hide_command(&prompt) {
                            self.is_hidden = true;
                            self.status = AgentStatus::Idle;
                            self.chat_history.push(ChatEntry {
                                role: ChatRole::User,
                                text: prompt,
                                timestamp: current_time_str(),
                                quick_suggestions: Vec::new(),
                                screen_size_kb: None,
                            });
                            let reply = localized_speech(
                                lang_prefix,
                                "Overlay masqué. Je reste à l'écoute.",
                                "Overlay hidden. Still listening in the background.",
                                "Interfaz oculta. Sigo escuchando en segundo plano.",
                                "Overlay ausgeblendet. Ich höre im Hintergrund weiter zu.",
                                "Interfaccia nascosta. Resto in ascolto in background.",
                                "Overlay oculto. Continuo ouvindo em segundo plano.",
                                "Оверлей скрыт. Продолжаю слушать в фоновом режиме.",
                                "悬浮窗已隐藏，仍在后台监听指令。",
                                "オーバーレイを非表示にしました。バックグラウンドで待機中です。",
                            );
                            self.chat_history.push(ChatEntry {
                                role: ChatRole::Agent,
                                text: reply.clone(),
                                timestamp: current_time_str(),
                                quick_suggestions: Vec::new(),
                                screen_size_kb: None,
                            });
                            let _ = self.tts_sender.send(TtsCommand::Speak(reply));
                            self.continuous_mode = true;
                        } else if is_show_command(&prompt) {
                            let reply = localized_speech(
                                lang_prefix,
                                "L'overlay est déjà actif et visible.",
                                "The overlay is already active and visible.",
                                "La interfaz ya está activa y visible.",
                                "Das Overlay ist bereits aktiv und sichtbar.",
                                "L'interfaccia è già attiva e visibile.",
                                "O overlay já está ativo e visível.",
                                "Оверлей уже активен и виден.",
                                "悬浮窗已经处于激活和可见状态。",
                                "オーバーレイは既に表示されています。",
                            );
                            self.chat_history.push(ChatEntry {
                                role: ChatRole::Agent,
                                text: reply.clone(),
                                timestamp: current_time_str(),
                                quick_suggestions: Vec::new(),
                                screen_size_kb: None,
                            });
                            let _ = self.tts_sender.send(TtsCommand::Speak(reply));
                            self.continuous_mode = true;
                        } else if !prompt.trim().is_empty() {
                            self.status = AgentStatus::Thinking;
                            self.continuous_mode = true;
                            self.input_text.clear();
                            self.chat_history.push(ChatEntry {
                                role: ChatRole::User,
                                text: prompt.clone(),
                                timestamp: current_time_str(),
                                quick_suggestions: Vec::new(),
                                screen_size_kb: None,
                            });
                            let _ = self.command_sender.send(AgentCommand::Prompt(prompt));
                        } else if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                            self.start_recording(&ctx, false);
                        }
                    }
                }
                AgentEvent::TtsFinished => {
                    if self.continuous_mode
                        && self.status != AgentStatus::EmergencyStopped
                        && !AGENT_BUSY.load(Ordering::SeqCst)
                    {
                        self.start_recording(&ctx, false);
                    }
                }
                AgentEvent::WhisperReady => {
                    println!("[Whisper] Notification WhisperReady reçue.");
                    if self.continuous_mode
                        && self.status != AgentStatus::EmergencyStopped
                        && !AGENT_BUSY.load(Ordering::SeqCst)
                        && !self.is_recording
                    {
                        self.start_recording(&ctx, false);
                    }
                }
                AgentEvent::TwitchChatReceived(twitch_msg) => {
                    self.twitch_messages.push(twitch_msg);
                    if self.twitch_messages.len() > 300 {
                        self.twitch_messages.drain(0..self.twitch_messages.len() - 300);
                    }
                }
                AgentEvent::TwitchSearchResults(results) => {
                    self.twitch_search_results = results;
                }
                AgentEvent::RequestIgnored => {
                    if let Some(last) = self.chat_history.last() {
                        if last.role == ChatRole::User {
                            self.chat_history.pop();
                        }
                    }
                    self.live_transcript.clear();
                    if self.continuous_mode && self.status != AgentStatus::EmergencyStopped {
                        self.start_recording(&ctx, false);
                    }
                }
                AgentEvent::RequestApiKey => {
                    self.sync_modal_inputs_for_provider(self.llm_config.provider);
                    self.show_api_key_modal = true;
                    self.is_hidden = false;
                }
                AgentEvent::ShowFloatingCard(card) => {
                    let vp_id = egui::ViewportId::from_hash_of(&card.id);
                    if let Some(existing) = self.floating_cards.iter_mut().find(|c| c.id == card.id) {
                        *existing = card;
                    } else {
                        self.floating_cards.push(card);
                    }
                    update_open_floating_cards(&self.floating_cards);
                    ctx.request_repaint();
                    ctx.request_repaint_of(vp_id);
                }
                AgentEvent::CloseFloatingCard(id) => {
                    let vp_id = egui::ViewportId::from_hash_of(&id);
                    self.floating_cards.retain(|c| c.id != id);
                    update_open_floating_cards(&self.floating_cards);
                    ctx.request_repaint();
                    ctx.request_repaint_of(vp_id);
                }   
                AgentEvent::NavigationZonesReady { window_title, zones } => {
                    if self.switch_access_mode {
                        self.switch_zones = zones;
                        self.switch_step = SwitchAccessStep::Zones;
                        self.switch_focus_index = 0;
                        self.switch_last_tick = Instant::now() + Duration::from_millis(1500);

                        let count = self.switch_zones.len();
                        let msg = if count > 0 {
                            let first = &self.switch_zones[0];
                            #[cfg(windows)]
                            unsafe {
                                let _ = windows::Win32::UI::WindowsAndMessaging::SetCursorPos(
                                    first.click_x,
                                    first.click_y,
                                );
                            }
                            format!("{} zones détectées pour {}. {}. Entrée pour explorer.", count, window_title, first.title)
                        } else {
                            format!("Navigation directe activée pour {}.", window_title)
                        };
                        self.chat_history.push(ChatEntry {
                            role: ChatRole::Agent,
                            text: msg.clone(),
                            timestamp: current_time_str(),
                            quick_suggestions: Vec::new(),
                            screen_size_kb: None,
                        });
                        let _ = self.tts_sender.send(TtsCommand::Stop);
                        let _ = self.tts_sender.send(TtsCommand::Speak(msg));
                        ctx.request_repaint();
                    }
                }
            }
        }

        let is_typing = ctx.memory(|m| {
            m.has_focus(egui::Id::new("prompt_input_text"))
                || m.has_focus(egui::Id::new("api_key_input_field"))
                || m.has_focus(egui::Id::new("model_input_field"))
        });

        // Gestion de la touche Échap globale et locale
        #[cfg(windows)]
        let global_escape_hit = unsafe { (GetAsyncKeyState(0x1B) as u16 & 0x8000) != 0 };
        #[cfg(not(windows))]
        let global_escape_hit = false;

        let egui_escape_hit = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let escape_triggered = (egui_escape_hit || (global_escape_hit && (self.dictation_mode || self.switch_access_mode)))
            && self.last_switch_input.elapsed() >= Duration::from_millis(250);

        if escape_triggered {
            self.last_switch_input = Instant::now();
            if self.status == AgentStatus::Speaking {
                self.stop_speaking();
            } else if self.dictation_mode {
                self.exit_dictation_mode();
            } else if self.switch_access_mode {
                if self.switch_step == SwitchAccessStep::Elements {
                    if !self.switch_zones.is_empty() {
                        self.switch_step = SwitchAccessStep::Zones;
                        self.switch_focus_index = 0;
                        self.switch_last_tick = Instant::now() + Duration::from_millis(1500);
                        let _ = self.tts_sender.send(TtsCommand::Stop);
                        let cur_zone = &self.switch_zones[0];
                        let msg = format!("Retour aux blocs. {}", cur_zone.title);
                        let _ = self.tts_sender.send(TtsCommand::Speak(msg));
                        play_earcon(Earcon::Success);
                    } else {
                        self.switch_step = SwitchAccessStep::Windows;
                        self.numbered_targets.clear();
                        self.switch_windows = collect_switch_candidate_windows();
                        self.switch_focus_index = 0;
                        let _ = self.tts_sender.send(TtsCommand::Stop);
                        let (locale, _) = get_system_locale_language();
                        let lang_prefix = locale.split(['-', '_']).next().unwrap_or("fr");
                        let base_msg = localized_speech(
                            lang_prefix,
                            "Retour aux fenêtres",
                            "Back to windows",
                            "Volver a las ventanas",
                            "Zurück zu den Fenstern",
                            "Torna alle finestre",
                            "Voltar para as janelas",
                            "Возврат к окнам",
                            "返回窗口列表",
                            "ウィンドウ選択に戻る",
                        );
                        let msg = if let Some(first_win) = self.switch_windows.first() {
                            format!("{base_msg} : {}", first_win.title)
                        } else {
                            base_msg
                        };
                        let _ = self.tts_sender.send(TtsCommand::Speak(msg));
                        play_earcon(Earcon::Success);
                    }
                } else if self.switch_step == SwitchAccessStep::Zones {
                    self.switch_step = SwitchAccessStep::Windows;
                    self.numbered_targets.clear();
                    self.switch_zones.clear();
                    self.switch_windows = collect_switch_candidate_windows();
                    self.switch_focus_index = 0;
                    let _ = self.tts_sender.send(TtsCommand::Stop);
                    let (locale, _) = get_system_locale_language();
                    let lang_prefix = locale.split(['-', '_']).next().unwrap_or("fr");
                    let base_msg = localized_speech(
                        lang_prefix,
                        "Retour aux fenêtres",
                        "Back to windows",
                        "Volver a las ventanas",
                        "Zurück zu den Fenstern",
                        "Torna alle finestre",
                        "Voltar para as janelas",
                        "Возврат к окнам",
                        "返回窗口列表",
                        "ウィンドウ選択に戻る",
                    );
                    let msg = if let Some(first_win) = self.switch_windows.first() {
                        format!("{base_msg} : {}", first_win.title)
                    } else {
                        base_msg
                    };
                    let _ = self.tts_sender.send(TtsCommand::Speak(msg));
                    play_earcon(Earcon::Success);
                } else {
                    self.toggle_switch_access_mode();
                }
            } else {
                self.trigger_emergency_stop(&ctx);
            }
        }

        // Gestion du Switch Access & Survol : défilement séquentiel périodique et validation sur Entrée / Espace
        if self.switch_access_mode && !is_typing {
            #[cfg(windows)]
            let global_enter = unsafe { (GetAsyncKeyState(0x0D) as u16 & 0x8000) != 0 };
            #[cfg(not(windows))]
            let global_enter = false;

            #[cfg(windows)]
            let global_space = unsafe { (GetAsyncKeyState(0x20) as u16 & 0x8000) != 0 };
            #[cfg(not(windows))]
            let global_space = false;

            #[cfg(windows)]
            let global_nav_next = unsafe {
                (GetAsyncKeyState(0x27) as u16 & 0x8000) != 0 // Flèche droite
                    || (GetAsyncKeyState(0x28) as u16 & 0x8000) != 0 // Flèche bas
                    || (GetAsyncKeyState(0x66) as u16 & 0x8000) != 0 // Pavé numérique 6
                    || (GetAsyncKeyState(0x36) as u16 & 0x8000) != 0 // Touche 6
            };
            #[cfg(not(windows))]
            let global_nav_next = false;

            #[cfg(windows)]
            let global_nav_prev = unsafe {
                (GetAsyncKeyState(0x25) as u16 & 0x8000) != 0 // Flèche gauche
                    || (GetAsyncKeyState(0x26) as u16 & 0x8000) != 0 // Flèche haut
                    || (GetAsyncKeyState(0x64) as u16 & 0x8000) != 0 // Pavé numérique 4
                    || (GetAsyncKeyState(0x34) as u16 & 0x8000) != 0 // Touche 4
            };
            #[cfg(not(windows))]
            let global_nav_prev = false;

            let egui_enter = ctx.input(|i| i.key_pressed(egui::Key::Enter));
            let egui_space = ctx.input(|i| i.key_pressed(egui::Key::Space));
            let egui_nav_next = ctx.input(|i| {
                i.key_pressed(egui::Key::ArrowRight)
                    || i.key_pressed(egui::Key::ArrowDown)
                    || i.key_pressed(egui::Key::Num6)
            });
            let egui_nav_prev = ctx.input(|i| {
                i.key_pressed(egui::Key::ArrowLeft)
                    || i.key_pressed(egui::Key::ArrowUp)
                    || i.key_pressed(egui::Key::Num4)
            });

            let switch_input_hit = (egui_enter || egui_space || global_enter || global_space)
                && self.last_switch_input.elapsed() >= Duration::from_millis(300);

            let nav_next_hit = (egui_nav_next || global_nav_next)
                && self.last_switch_input.elapsed() >= Duration::from_millis(220);
            let nav_prev_hit = (egui_nav_prev || global_nav_prev)
                && self.last_switch_input.elapsed() >= Duration::from_millis(220);

            if egui_enter || egui_space || egui_nav_next || egui_nav_prev {
                ctx.input_mut(|i| {
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Space);
                    i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight);
                    i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown);
                    i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft);
                    i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp);
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Num6);
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Num4);
                });
            }

            match self.switch_step {
                SwitchAccessStep::Windows => {
                    if self.switch_windows.is_empty() {
                        self.switch_windows = collect_switch_candidate_windows();
                    }
                    let num_windows = self.switch_windows.len();
                    if num_windows > 0 {
                        let auto_tick = self.switch_last_tick.elapsed() >= Duration::from_millis(1500);
                        let step_next = nav_next_hit || auto_tick;
                        let step_prev = nav_prev_hit;

                        if step_next || step_prev {
                            if step_prev {
                                self.switch_focus_index = if self.switch_focus_index == 0 {
                                    num_windows - 1
                                } else {
                                    self.switch_focus_index - 1
                                };
                            } else {
                                self.switch_focus_index = (self.switch_focus_index + 1) % num_windows;
                            }
                            self.switch_last_tick = Instant::now();
                            if nav_next_hit || nav_prev_hit {
                                self.last_switch_input = Instant::now();
                            }

                            let cur_win = &self.switch_windows[self.switch_focus_index];
                            #[cfg(windows)]
                            unsafe {
                                let _ = windows::Win32::UI::WindowsAndMessaging::SetCursorPos(
                                    cur_win.click_x,
                                    cur_win.click_y,
                                );
                            }
                            #[cfg(target_os = "macos")]
                            {
                                let script = format!(
                                    r#"tell application "System Events" to set position of (every process whose frontmost is true) to {{{}, {}}}"#,
                                    cur_win.click_x, cur_win.click_y
                                );
                                let _ = std::process::Command::new("osascript").args(["-e", &script]).status();
                            }
                            #[cfg(all(not(windows), not(target_os = "macos")))]
                            {
                                let _ = std::process::Command::new("xdotool")
                                    .args(["mousemove", &cur_win.click_x.to_string(), &cur_win.click_y.to_string()])
                                    .status();
                            }
                            // Le narrateur lit le nom de la fenêtre activement ciblée
                            let _ = self.tts_sender.send(TtsCommand::Stop);
                            let _ = self.tts_sender.send(TtsCommand::Speak(cur_win.title.clone()));
                            ctx.request_repaint();
                        }

                        if switch_input_hit {
                            self.last_switch_input = Instant::now();
                            play_earcon(Earcon::Success);

                            let cur_win = &self.switch_windows[self.switch_focus_index % num_windows];
                            #[cfg(windows)]
                            let target_hwnd = windows::Win32::Foundation::HWND(cur_win.id as _);
                            #[cfg(windows)]
                            {
                                force_foreground_window(target_hwnd);
                                restore_if_iconic(target_hwnd);
                                std::thread::sleep(Duration::from_millis(60));
                                self.numbered_targets = collect_numbered_targets_for_window(target_hwnd);
                            }
                            #[cfg(not(windows))]
                            {
                                force_foreground_window(&cur_win.title);
                                self.numbered_targets = collect_numbered_targets_for_window(&cur_win.title);
                            }

                            let count = self.numbered_targets.len();
                            if count > 0 {
                                let status_text = format!("Analyse intelligente des blocs de la fenêtre {}...", cur_win.title);
                                let _ = self.tts_sender.send(TtsCommand::Stop);
                                let _ = self.tts_sender.send(TtsCommand::Speak(status_text.clone()));
                                let _ = self.command_sender.send(AgentCommand::AnalyzeNavigationZones {
                                    window_title: cur_win.title.clone(),
                                    targets: self.numbered_targets.clone(),
                                });
                            } else {
                                let msg = format!("Fenêtre sélectionnée : {}. Aucun contrôle interactif direct détecté. Échap pour revenir.", cur_win.title);
                                let _ = self.tts_sender.send(TtsCommand::Speak(msg));
                            };
                            ctx.request_repaint();
                        }
                    }
                }
                SwitchAccessStep::Zones => {
                    let num_zones = self.switch_zones.len();
                    if num_zones > 0 {
                        let auto_tick = self.switch_last_tick.elapsed() >= Duration::from_millis(2200);
                        let step_next = nav_next_hit || auto_tick;
                        let step_prev = nav_prev_hit;

                        if step_next || step_prev {
                            if step_prev {
                                self.switch_focus_index = if self.switch_focus_index == 0 {
                                    num_zones - 1
                                } else {
                                    self.switch_focus_index - 1
                                };
                            } else {
                                self.switch_focus_index = (self.switch_focus_index + 1) % num_zones;
                            }
                            self.switch_last_tick = Instant::now();
                            if nav_next_hit || nav_prev_hit {
                                self.last_switch_input = Instant::now();
                            }

                            let cur_zone = &self.switch_zones[self.switch_focus_index];
                            #[cfg(windows)]
                            unsafe {
                                let _ = windows::Win32::UI::WindowsAndMessaging::SetCursorPos(
                                    cur_zone.click_x,
                                    cur_zone.click_y,
                                );
                            }
                            #[cfg(target_os = "macos")]
                            {
                                let script = format!(
                                    r#"tell application "System Events" to set position of (every process whose frontmost is true) to {{{}, {}}}"#,
                                    cur_zone.click_x, cur_zone.click_y
                                );
                                let _ = std::process::Command::new("osascript").args(["-e", &script]).status();
                            }
                            #[cfg(all(not(windows), not(target_os = "macos")))]
                            {
                                let _ = std::process::Command::new("xdotool")
                                    .args(["mousemove", &cur_zone.click_x.to_string(), &cur_zone.click_y.to_string()])
                                    .status();
                            }
                            let vocal_label = if !cur_zone.title.trim().is_empty() {
                                if !cur_zone.description.trim().is_empty() {
                                    format!("{}, {}", cur_zone.title.trim(), cur_zone.description.trim())
                                } else {
                                    cur_zone.title.trim().to_string()
                                }
                            } else if !cur_zone.description.trim().is_empty() {
                                cur_zone.description.trim().to_string()
                            } else {
                                format!("Zone {} sur {}", self.switch_focus_index + 1, num_zones)
                            };
                            let _ = self.tts_sender.send(TtsCommand::Stop);
                            let _ = self.tts_sender.send(TtsCommand::Speak(vocal_label));
                            ctx.request_repaint();
                        }

                        if switch_input_hit {
                            self.last_switch_input = Instant::now();
                            play_earcon(Earcon::Success);

                            let cur_zone = &self.switch_zones[self.switch_focus_index % num_zones];
                            self.numbered_targets = cur_zone.targets.clone();
                            self.switch_step = SwitchAccessStep::Elements;
                            self.switch_focus_index = 0;
                            self.switch_last_tick = Instant::now() + Duration::from_millis(1500);

                            let count = self.numbered_targets.len();
                            let msg = format!("{} sélectionné, {} contrôles disponibles. Entrée pour valider.", cur_zone.title, count);
                            let _ = self.tts_sender.send(TtsCommand::Stop);
                            let _ = self.tts_sender.send(TtsCommand::Speak(msg));
                            ctx.request_repaint();
                        }
                    }
                }
                SwitchAccessStep::Elements => {
                    let num_targets = self.numbered_targets.len();
                    if num_targets > 0 {
                        // Défilement automatique périodique avec déplacement physique de la souris
                        let auto_tick = self.switch_last_tick.elapsed() >= Duration::from_millis(1300);
                        let step_next = nav_next_hit || auto_tick;
                        let step_prev = nav_prev_hit;

                        if step_next || step_prev {
                            if step_prev {
                                self.switch_focus_index = if self.switch_focus_index == 0 {
                                    num_targets - 1
                                } else {
                                    self.switch_focus_index - 1
                                };
                            } else {
                                self.switch_focus_index = (self.switch_focus_index + 1) % num_targets;
                            }
                            self.switch_last_tick = Instant::now();
                            if nav_next_hit || nav_prev_hit {
                                self.last_switch_input = Instant::now();
                            }

                            let cur_target = &self.numbered_targets[self.switch_focus_index];
                            #[cfg(windows)]
                            unsafe {
                                let _ = windows::Win32::UI::WindowsAndMessaging::SetCursorPos(
                                    cur_target.click_x,
                                    cur_target.click_y,
                                );
                            }
                            #[cfg(target_os = "macos")]
                            {
                                let script = format!(
                                    r#"tell application "System Events" to set position of (every process whose frontmost is true) to {{{}, {}}}"#,
                                    cur_target.click_x, cur_target.click_y
                                );
                                let _ = std::process::Command::new("osascript").args(["-e", &script]).status();
                            }
                            #[cfg(all(not(windows), not(target_os = "macos")))]
                            {
                                let _ = std::process::Command::new("xdotool")
                                    .args(["mousemove", &cur_target.click_x.to_string(), &cur_target.click_y.to_string()])
                                    .status();
                            }
                            let (locale, _) = get_system_locale_language();
                            let lang_prefix = locale.split(['-', '_']).next().unwrap_or("fr");
                            let fallback_label = localized_speech(
                                lang_prefix,
                                "Contrôle",
                                "Control",
                                "Control",
                                "Steuerelement",
                                "Controllo",
                                "Controle",
                                "Элемент",
                                "控件",
                                "コントロール",
                            );
                            let label = if cur_target.name.is_empty() { &fallback_label } else { &cur_target.name };
                            let _ = self.tts_sender.send(TtsCommand::Stop);
                            let _ = self.tts_sender.send(TtsCommand::Speak(label.to_string()));
                            ctx.request_repaint();
                        }

                        if switch_input_hit {
                            self.last_switch_input = Instant::now();
                            play_earcon(Earcon::Success);

                            let cur_target = &self.numbered_targets[self.switch_focus_index % num_targets];
                            let click_x = cur_target.click_x;
                            let click_y = cur_target.click_y;

                            #[cfg(windows)]
                            unsafe {
                                let _ = windows::Win32::UI::WindowsAndMessaging::SetCursorPos(click_x, click_y);
                                std::thread::sleep(Duration::from_millis(25));
                                windows::Win32::UI::Input::KeyboardAndMouse::mouse_event(
                                    windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0,
                                );
                                std::thread::sleep(Duration::from_millis(30));
                                windows::Win32::UI::Input::KeyboardAndMouse::mouse_event(
                                    windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_LEFTUP, 0, 0, 0, 0,
                                );
                            }
                            #[cfg(target_os = "macos")]
                            {
                                let script = format!(
                                    r#"tell application "System Events" to click at {{{}, {}}}"#,
                                    click_x, click_y
                                );
                                let _ = std::process::Command::new("osascript").args(["-e", &script]).status();
                            }
                            #[cfg(all(not(windows), not(target_os = "macos")))]
                            {
                                let _ = std::process::Command::new("xdotool")
                                    .args(["mousemove", &click_x.to_string(), &click_y.to_string(), "click", "1"])
                                    .status();
                            }

                            self.switch_last_tick = Instant::now() + Duration::from_millis(300);
                            self.numbered_targets.clear();
                            ctx.request_repaint();
                        }
                    } else if let Some(last_entry) = self.chat_history.last() {
                        let sug_count = last_entry.quick_suggestions.len();
                        if sug_count > 0 {
                            let auto_tick = self.switch_last_tick.elapsed() >= Duration::from_millis(1500);
                            let step_next = nav_next_hit || auto_tick;
                            let step_prev = nav_prev_hit;

                            if step_next || step_prev {
                                if step_prev {
                                    self.switch_focus_index = if self.switch_focus_index == 0 {
                                        sug_count - 1
                                    } else {
                                        self.switch_focus_index - 1
                                    };
                                } else {
                                    self.switch_focus_index = (self.switch_focus_index + 1) % sug_count;
                                }
                                self.switch_last_tick = Instant::now();
                                if nav_next_hit || nav_prev_hit {
                                    self.last_switch_input = Instant::now();
                                }
                                if let Some(text) = last_entry.quick_suggestions[self.switch_focus_index % sug_count].to_display_string() {
                                    let _ = self.tts_sender.send(TtsCommand::Stop);
                                    let _ = self.tts_sender.send(TtsCommand::Speak(text));
                                }
                                ctx.request_repaint();
                            }

                            if switch_input_hit {
                                self.last_switch_input = Instant::now();
                                play_earcon(Earcon::Success);
                                let idx = self.switch_focus_index % sug_count;
                                if let Some(text) = last_entry.quick_suggestions[idx].to_display_string() {
                                    let _ = self.command_sender.send(AgentCommand::Prompt(text));
                                }
                                self.switch_last_tick = Instant::now();
                            }
                        }
                    }
                }
            }
        }

        // Raccourci clavier 'R' pour basculer le micro (désactivé si l'utilisateur est en train de taper du texte)
        if !self.is_hidden && !is_typing && ctx.input(|i| i.key_pressed(egui::Key::R)) {
            self.toggle_recording(&ctx);
        }

        // Retranscription en temps réel : mise à jour du texte d'écoute
        if self.is_recording {
            let trimmed = self.input_text.trim();
            if !trimmed.is_empty() {
                self.live_transcript = trimmed.to_string();
            }
        }

        let time = ctx.input(|i| i.time);
        ctx.request_repaint_after(Duration::from_millis(16));

        if self.is_hidden {
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(egui::Color32::TRANSPARENT))
                .show(ui, |_ui| {});
            return;
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(egui::Color32::TRANSPARENT).inner_margin(egui::Margin::ZERO))
            .show(ui, |ui| {
                // Fond et cadre stylisé de la boîte de discussion
                ui.painter().rect(
                    response_rect,
                    egui::CornerRadius::same(12),
                    theme.panel_bg,
                    egui::Stroke::new(1.0, theme.border_subtle),
                    egui::StrokeKind::Inside,
                );

                // 1. Barre d'onglets supérieure et boutons système de la fenêtre
                let mut tab_bar_max_x = response_rect.min.x + 210.0;

                let header_rect = egui::Rect::from_min_max(
                    egui::pos2(response_rect.min.x + 10.0, response_rect.min.y + 6.0),
                    egui::pos2(response_rect.max.x - 10.0, response_rect.min.y + 32.0),
                );

                ui.scope_builder(egui::UiBuilder::new().max_rect(header_rect), |ui| {
                    ui.horizontal(|ui| {
                        let tab_assist = ui.selectable_label(self.active_tab == ActiveTab::Assistance, "🤖 Assistance");
                        if tab_assist.clicked() {
                            self.active_tab = ActiveTab::Assistance;
                        }
                        let chat_title = if self.twitch_messages.is_empty() {
                            "💬 Chat twitch".to_string()
                        } else {
                            format!("💬 Chat ({})", self.twitch_messages.len())
                        };
                        let tab_chat = ui.selectable_label(self.active_tab == ActiveTab::Chat, chat_title);
                        if tab_chat.clicked() {
                            self.active_tab = ActiveTab::Chat;
                        }
                        tab_bar_max_x = tab_assist.rect.max.x.max(tab_chat.rect.max.x);

                        // Boutons système en haut à droite : Réduire, Agrandir/Restaurer, Fermer
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;

                            let close_btn = ui.add(
                                egui::Button::new(
                                    egui::RichText::new("×")
                                        .size(13.0)
                                        .color(egui::Color32::from_rgb(235, 235, 245)),
                                )
                                .min_size(egui::vec2(20.0, 18.0))
                                .corner_radius(egui::CornerRadius::same(4)),
                            );
                            if close_btn.on_hover_text("Fermer").clicked() {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }

                            let max_icon = if is_maximized { "⧉" } else { "□" };
                            let max_btn = ui.add(
                                egui::Button::new(
                                    egui::RichText::new(max_icon)
                                        .size(10.5)
                                        .color(egui::Color32::from_rgb(200, 215, 230)),
                                )
                                .min_size(egui::vec2(20.0, 18.0))
                                .corner_radius(egui::CornerRadius::same(4)),
                            );
                            if max_btn.on_hover_text(if is_maximized { "Restaurer" } else { "Agrandir" }).clicked() {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!is_maximized));
                            }

                            let min_btn = ui.add(
                                egui::Button::new(
                                    egui::RichText::new("—")
                                        .size(10.5)
                                        .color(egui::Color32::from_rgb(200, 215, 230)),
                                )
                                .min_size(egui::vec2(20.0, 18.0))
                                .corner_radius(egui::CornerRadius::same(4)),
                            );
                            if min_btn.on_hover_text("Réduire").clicked() {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                            }
                        });
                    });
                });

                let sep_y = response_rect.min.y + 35.0;
                ui.painter().hline(
                    (response_rect.min.x + 8.0)..=(response_rect.max.x - 8.0),
                    sep_y,
                    egui::Stroke::new(1.0, theme.border_subtle),
                );

                // 2. Zone inférieure évaluée en deuxième (contrôles, avatar et saisie) pour précéder les messages au focus Tab
                ui.scope_builder(egui::UiBuilder::new().max_rect(control_rect), |ui| {
                    egui::Frame::new()
                        .fill(theme.control_bg)
                        .stroke(egui::Stroke::new(1.0, theme.border))
                        .corner_radius(egui::CornerRadius::same(12))
                        .inner_margin(egui::Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            let inner_rect = ui.available_rect_before_wrap();
                            let avatar_pos = egui::pos2(inner_rect.max.x - 20.0, inner_rect.center().y);
                            self.draw_avatar(ui, avatar_pos, 14.0, time);

                            let avatar_rect = egui::Rect::from_center_size(avatar_pos, egui::vec2(32.0, 32.0));
                            let avatar_resp = ui.interact(avatar_rect, egui::Id::new("avatar_interact"), egui::Sense::click());
                            if self.status == AgentStatus::Speaking {
                                if avatar_resp.hovered() {
                                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                }
                                if avatar_resp.on_hover_text("Cliquer pour couper la parole (Échap)").clicked() {
                                    self.stop_speaking();
                                }
                            }

                            let content_area = egui::Rect::from_min_max(
                                inner_rect.min,
                                egui::pos2(inner_rect.max.x - 48.0, inner_rect.max.y),
                            );

                            ui.scope_builder(egui::UiBuilder::new().max_rect(content_area), |ui| {
                                ui.vertical(|ui| {
                                    let prov_name = self.llm_config.provider.display_name();
                                    let (locale, _) = get_system_locale_language();
                                    let lang_prefix = locale.split(['-', '_']).next().unwrap_or("fr");
                                    let status_badge = if self.dictation_mode {
                                        format!("{prov_name} · ✍ Mode dictée actif (Échap pour quitter)")
                                    } else if self.switch_access_mode {
                                        if self.switch_step == SwitchAccessStep::Windows {
                                            if !self.switch_windows.is_empty() {
                                                let cur = &self.switch_windows[self.switch_focus_index % self.switch_windows.len()];
                                                let cur_idx = (self.switch_focus_index % self.switch_windows.len()) + 1;
                                                let total_wins = self.switch_windows.len();
                                                localized_speech(
                                                    lang_prefix,
                                                    &format!("🎯 Fenêtres [{cur_idx}/{total_wins}]: {} (Entrée: choisir)", cur.title),
                                                    &format!("🎯 Windows [{cur_idx}/{total_wins}]: {} (Enter: select)", cur.title),
                                                    &format!("🎯 Ventanas [{cur_idx}/{total_wins}]: {} (Entrar: elegir)", cur.title),
                                                    &format!("🎯 Fenster [{cur_idx}/{total_wins}]: {} (Eingabe: wählen)", cur.title),
                                                    &format!("🎯 Finestre [{cur_idx}/{total_wins}]: {} (Invio: seleziona)", cur.title),
                                                    &format!("🎯 Janelas [{cur_idx}/{total_wins}]: {} (Enter: escolher)", cur.title),
                                                    &format!("🎯 Окна [{cur_idx}/{total_wins}]: {} (Enter: выбрать)", cur.title),
                                                    &format!("🎯 窗口 [{cur_idx}/{total_wins}]: {} (Enter: 选择)", cur.title),
                                                    &format!("🎯 ウィンドウ [{cur_idx}/{total_wins}]: {} (Enter: 選択)", cur.title),
                                                )
                                            } else {
                                                localized_speech(
                                                    lang_prefix,
                                                    "🎯 Recherche de fenêtres...",
                                                    "🎯 Searching for windows...",
                                                    "🎯 Buscando ventanas...",
                                                    "🎯 Fenster werden gesucht...",
                                                    "🎯 Ricerca finestre...",
                                                    "🎯 Procurando janelas...",
                                                    "🎯 Поиск окон...",
                                                    "🎯 正在搜索窗口...",
                                                    "🎯 ウィンドウを検索中...",
                                                )
                                            }
                                        } else if self.switch_step == SwitchAccessStep::Zones {
                                            if !self.switch_zones.is_empty() {
                                                let cur = &self.switch_zones[self.switch_focus_index % self.switch_zones.len()];
                                                let cur_idx = (self.switch_focus_index % self.switch_zones.len()) + 1;
                                                let total_zones = self.switch_zones.len();
                                                localized_speech(
                                                    lang_prefix,
                                                    &format!("🎯 [{cur_idx}/{total_zones}] {} (Entrée: ouvrir)", cur.title),
                                                    &format!("🎯 [{cur_idx}/{total_zones}] {} (Enter: open)", cur.title),
                                                    &format!("🎯 [{cur_idx}/{total_zones}] {} (Entrar: abrir)", cur.title),
                                                    &format!("🎯 [{cur_idx}/{total_zones}] {} (Eingabe: öffnen)", cur.title),
                                                    &format!("🎯 [{cur_idx}/{total_zones}] {} (Invio: apri)", cur.title),
                                                    &format!("🎯 [{cur_idx}/{total_zones}] {} (Enter: abrir)", cur.title),
                                                    &format!("🎯 [{cur_idx}/{total_zones}] {} (Enter: открыть)", cur.title),
                                                    &format!("🎯 [{cur_idx}/{total_zones}] {} (Enter: 打开)", cur.title),
                                                    &format!("🎯 [{cur_idx}/{total_zones}] {} (Enter: 開く)", cur.title),
                                                )
                                            } else {
                                                localized_speech(
                                                    lang_prefix,
                                                    "🎯 Analyse des blocs de la fenêtre...",
                                                    "🎯 Analyzing window zones...",
                                                    "🎯 Analizando zonas de la ventana...",
                                                    "🎯 Fensterbereiche werden analysiert...",
                                                    "🎯 Analisi dei blocchi della finestra...",
                                                    "🎯 Analisando blocos da janela...",
                                                    "🎯 Анализ блоков окна...",
                                                    "🎯 正在分析窗口区域...",
                                                    "🎯 ウィンドウブロックを解析中...",
                                                )
                                            }
                                        } else if !self.numbered_targets.is_empty() {
                                            let cur = &self.numbered_targets[self.switch_focus_index % self.numbered_targets.len()];
                                            let label = if cur.name.is_empty() { "Contrôle" } else { cur.name.as_str() };
                                            let cur_idx = (self.switch_focus_index % self.numbered_targets.len()) + 1;
                                            let total_targets = self.numbered_targets.len();
                                            localized_speech(
                                                lang_prefix,
                                                &format!("🎯 Contrôles [{cur_idx}/{total_targets}]: {label} (Entrée: cliquer)"),
                                                &format!("🎯 Controls [{cur_idx}/{total_targets}]: {label} (Enter: click)"),
                                                &format!("🎯 Controles [{cur_idx}/{total_targets}]: {label} (Entrar: clic)"),
                                                &format!("🎯 Steuerelemente [{cur_idx}/{total_targets}]: {label} (Eingabe: Klick)"),
                                                &format!("🎯 Controlli [{cur_idx}/{total_targets}]: {label} (Invio: clic)"),
                                                &format!("🎯 Controles [{cur_idx}/{total_targets}]: {label} (Enter: clicar)"),
                                                &format!("🎯 Элементы [{cur_idx}/{total_targets}]: {label} (Enter: клик)"),
                                                &format!("🎯 控件 [{cur_idx}/{total_targets}]: {label} (Enter: 点击)"),
                                                &format!("🎯 コントロール [{cur_idx}/{total_targets}]: {label} (Enter: クリック)"),
                                            )
                                        } else {
                                            localized_speech(
                                                lang_prefix,
                                                "🎯 Détection des contrôles...",
                                                "🎯 Detecting controls...",
                                                "🎯 Detectando controles...",
                                                "🎯 Steuerelemente werden erkannt...",
                                                "🎯 Rilevamento controlli...",
                                                "🎯 Detectando controles...",
                                                "🎯 Обнаружение элементов...",
                                                "🎯 正在检测控件...",
                                                "🎯 コントロールを検出中...",
                                            )
                                        }
                                    } else {
                                        match self.status {
                                            AgentStatus::Idle => format!("{prov_name} · en veille"),
                                            AgentStatus::Thinking => format!("{prov_name} · réflexion..."),
                                            AgentStatus::Speaking => format!("{prov_name} · en direct"),
                                            AgentStatus::Listening => format!("{prov_name} · écoute active (r)..."),
                                            AgentStatus::EmergencyStopped => "Système verrouillé".to_string(),
                                        }
                                    };

                                    let badge_color = if self.dictation_mode {
                                        egui::Color32::from_rgb(255, 205, 70)
                                    } else if self.switch_access_mode {
                                        egui::Color32::from_rgb(0, 255, 200)
                                    } else if self.status == AgentStatus::Listening {
                                        egui::Color32::from_rgb(255, 80, 80)
                                    } else {
                                        theme.accent
                                    };

                                    ui.horizontal(|ui| {
                                        let badge_resp = ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(&status_badge)
                                                    .size(11.0)
                                                    .color(badge_color),
                                            )
                                            .sense(if self.status == AgentStatus::Speaking { egui::Sense::click() } else { egui::Sense::hover() }),
                                        );
                                        if self.status == AgentStatus::Speaking {
                                            if badge_resp.hovered() {
                                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                            }
                                            if badge_resp.on_hover_text("Cliquer pour couper la parole (Échap)").clicked() {
                                                self.stop_speaking();
                                            }
                                        }
                                        ui.add_space(4.0);
                                        self.draw_mini_equalizer(ui, time);

                                        if self.status == AgentStatus::Speaking {
                                            ui.add_space(6.0);
                                            let stop_speech_btn = ui.add(
                                                egui::Button::new(
                                                    egui::RichText::new("🔇 Couper la parole")
                                                        .size(10.5)
                                                        .color(egui::Color32::from_rgb(255, 140, 140))
                                                        .strong(),
                                                )
                                                .small()
                                                .corner_radius(egui::CornerRadius::same(6)),
                                            );
                                            if stop_speech_btn.on_hover_text("Interrompre la synthèse vocale immédiatement (Échap)").clicked() {
                                                self.stop_speaking();
                                            }
                                        }
                                    });

                                    if self.is_recording && !self.live_transcript.is_empty() {
                                        ui.add_space(2.0);
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(&self.live_transcript)
                                                    .size(11.5)
                                                    .color(egui::Color32::from_rgb(255, 220, 130)),
                                            )
                                            .truncate(),
                                        );
                                    }

                                    ui.add_space(4.0);

                                    // Barre de suggestions d'applications (PATH / Levenshtein)
                                    let mut clicked_app_suggestion: Option<String> = None;
                                    let mut autocomplete_input_to: Option<String> = None;

                                    if !self.input_suggestions.is_empty() {
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 4.0;
                                            ui.label(
                                                egui::RichText::new("💡 Suggestions :")
                                                    .size(10.5)
                                                    .color(theme.accent)
                                                    .strong(),
                                            );
                                            for sug in &self.input_suggestions {
                                                let chip = egui::Button::new(
                                                    egui::RichText::new(format!("🚀 {sug}"))
                                                        .size(10.5)
                                                        .color(egui::Color32::from_rgb(220, 240, 255)),
                                                )
                                                .fill(egui::Color32::from_rgba_unmultiplied(0, 180, 255, 30))
                                                .stroke(egui::Stroke::new(1.0, theme.border_subtle))
                                                .corner_radius(egui::CornerRadius::same(6))
                                                .small();

                                                let resp = ui.add(chip)
                                                    .on_hover_text(format!("Lancer '{}' (Clic gauche)\nTab : compléter dans le champ", sug));
                                                if resp.clicked() {
                                                    clicked_app_suggestion = Some(format!("Ouvrir {sug}"));
                                                }
                                                resp.context_menu(|ui| {
                                                    if ui.button("Compléter dans le champ").clicked() {
                                                        autocomplete_input_to = Some(format!("Ouvrir {sug}"));
                                                        ui.close();
                                                    }
                                                });
                                            }
                                        });
                                        ui.add_space(2.0);
                                    }

                                    // 1ère rangée : Champ de saisie occupant tout l'espace disponible + bouton Envoyer
                                    ui.horizontal(|ui| {
                                        let send_btn_w = 64.0;
                                        let edit_width = (ui.available_width() - send_btn_w - ui.spacing().item_spacing.x).max(60.0);
                                        let input_id = egui::Id::new("prompt_input_text");
                                        let is_input_focused = ctx.memory(|m| m.has_focus(input_id));
                                        let is_modal_open = self.show_api_key_modal;
                                        let is_other_typing = ctx.memory(|m| {
                                            m.has_focus(egui::Id::new("api_key_input_field"))
                                                || m.has_focus(egui::Id::new("model_input_field"))
                                        });

                                        // Autocomplétion de la première suggestion via la touche Tab
                                        if let Some(top_sug) = self.input_suggestions.first() {
                                            if is_input_focused && ctx.input(|i| i.key_pressed(egui::Key::Tab)) {
                                                autocomplete_input_to = Some(format!("Ouvrir {top_sug}"));
                                                ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab));
                                            }
                                        }

                                        let paste_hit = ctx.input(|i| {
                                            i.events.iter().any(|e| matches!(e, egui::Event::Paste(_)))
                                                || ((i.modifiers.ctrl || i.modifiers.command) && i.key_pressed(egui::Key::V))
                                                || (i.modifiers.shift && i.key_pressed(egui::Key::Insert))
                                        });

                                        let mut pending_paste_fallback: Option<String> = None;

                                        // Traitement du collage avant que TextEdit ne consomme la touche Ctrl+V
                                        if paste_hit && !is_modal_open && !is_other_typing {
                                            let clip_opt = get_clipboard_text()
                                                .or_else(get_plain_clipboard_text)
                                                .or_else(|| ctx.input(|i| {
                                                    i.events.iter().find_map(|e| {
                                                        if let egui::Event::Paste(text) = e {
                                                            Some(text.clone())
                                                        } else {
                                                            None
                                                        }
                                                    })
                                                }));

                                            if let Some(clip) = clip_opt {
                                                let clean_clip = clip.replace(['\r', '\n'], " ");
                                                let trimmed_clean = clean_clip.trim().to_string();
                                                if !trimmed_clean.is_empty() {
                                                    self.stop_speaking();
                                                    play_earcon(Earcon::Success);

                                                    if is_input_focused {
                                                        pending_paste_fallback = Some(trimmed_clean.clone());
                                                        ctx.input_mut(|i| {
                                                            i.events.retain(|e| !matches!(e, egui::Event::Paste(_)));
                                                            i.events.push(egui::Event::Paste(trimmed_clean.clone()));
                                                        });
                                                    } else {
                                                        self.input_text = trimmed_clean;
                                                        ctx.memory_mut(|m| m.request_focus(input_id));
                                                    }
                                                    ctx.request_repaint();
                                                }
                                            }

                                            ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::V));
                                            ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::V));
                                        }

                                        let prev_text = self.input_text.clone();
                                        let response = ui.add_sized(
                                            [edit_width, 24.0],
                                            egui::TextEdit::singleline(&mut self.input_text)
                                                .id(input_id)
                                                .hint_text(if self.is_recording {
                                                    "Écoute Whisper active... parlez ou tapez"
                                                } else {
                                                    "Consigne d'exploration... (entrée)"
                                                }),
                                        );

                                        if let Some(pasted) = pending_paste_fallback {
                                            if self.input_text == prev_text {
                                                self.input_text = pasted;
                                                ctx.request_repaint();
                                            }
                                        }

                                        response.context_menu(|ui| {
                                            if ui.button("📋 Coller (Ctrl+V)").clicked() {
                                                if let Some(clip) = get_clipboard_text().or_else(get_plain_clipboard_text) {
                                                    let clean_clip = clip.replace(['\r', '\n'], " ");
                                                    let trimmed_clean = clean_clip.trim().to_string();
                                                    if !trimmed_clean.is_empty() {
                                                        self.input_text = trimmed_clean;
                                                        self.stop_speaking();
                                                        play_earcon(Earcon::Success);
                                                    }
                                                }
                                                ui.memory_mut(|m| m.request_focus(input_id));
                                                ui.close();
                                            }
                                            if ui.button("📄 Copier tout (Ctrl+C)").clicked() {
                                                set_clipboard_text(&self.input_text);
                                                ctx.copy_text(self.input_text.clone());
                                                play_earcon(Earcon::Success);
                                                ui.close();
                                            }
                                            if ui.button("✂ Couper tout (Ctrl+X)").clicked() {
                                                set_clipboard_text(&self.input_text);
                                                ctx.copy_text(self.input_text.clone());
                                                self.input_text.clear();
                                                play_earcon(Earcon::Success);
                                                ui.close();
                                            }
                                            if ui.button("🗑 Effacer").clicked() {
                                                self.input_text.clear();
                                                ui.close();
                                            }
                                        });

                                        let copy_hit = ctx.input(|i| {
                                            i.events.iter().any(|e| matches!(e, egui::Event::Copy))
                                                || ((i.modifiers.ctrl || i.modifiers.command) && i.key_pressed(egui::Key::C))
                                        });
                                        if (response.has_focus() || is_input_focused) && copy_hit && !self.input_text.is_empty() {
                                            set_clipboard_text(&self.input_text);
                                            ctx.copy_text(self.input_text.clone());
                                            play_earcon(Earcon::Success);
                                        }

                                        let cut_hit = ctx.input(|i| {
                                            i.events.iter().any(|e| matches!(e, egui::Event::Cut))
                                                || ((i.modifiers.ctrl || i.modifiers.command) && i.key_pressed(egui::Key::X))
                                        });
                                        if (response.has_focus() || is_input_focused) && cut_hit && !self.input_text.is_empty() {
                                            set_clipboard_text(&self.input_text);
                                            ctx.copy_text(self.input_text.clone());
                                            self.input_text.clear();
                                            play_earcon(Earcon::Success);
                                        }

                                        if response.changed() {
                                            self.stop_speaking();
                                        }

                                        if let Some(completed) = autocomplete_input_to {
                                            self.input_text = completed;
                                            self.last_suggestion_query = self.input_text.trim().to_string();
                                            self.input_suggestions = find_matching_applications(&self.input_text, 5);
                                            ctx.memory_mut(|m| m.request_focus(input_id));
                                            ctx.request_repaint();
                                        }

                                        if let Some(prompt) = clicked_app_suggestion {
                                            self.stop_speaking();
                                            if self.is_recording {
                                                self.stop_recording();
                                            }
                                            self.continuous_mode = true;
                                            self.input_text.clear();
                                            self.input_suggestions.clear();
                                            self.last_suggestion_query.clear();
                                            self.live_transcript.clear();
                                            self.chat_history.push(ChatEntry {
                                                role: ChatRole::User,
                                                text: prompt.clone(),
                                                timestamp: current_time_str(),
                                                quick_suggestions: Vec::new(),
                                                screen_size_kb: None,
                                            });
                                            self.status = AgentStatus::Thinking;
                                            let _ = self.command_sender.send(AgentCommand::Prompt(prompt));
                                        }

                                        let enter_hit = (response.lost_focus() || response.has_focus())
                                            && ctx.input(|i| i.key_pressed(egui::Key::Enter));
                                        let send_btn = ui.add_sized(
                                            [send_btn_w, 24.0],
                                            egui::Button::new(
                                                egui::RichText::new("Envoyer")
                                                    .size(11.5)
                                                    .color(theme.accent_text),
                                            ),
                                        );
                                        if (send_btn.clicked() || enter_hit) && !self.input_text.trim().is_empty() {
                                            self.stop_speaking();
                                            if self.is_recording {
                                                self.stop_recording();
                                            }
                                            self.continuous_mode = true;
                                            self.input_suggestions.clear();
                                            self.last_suggestion_query.clear();
                                            let prompt = std::mem::take(&mut self.input_text).trim().to_string();
                                            self.live_transcript.clear();
                                            self.chat_history.push(ChatEntry {
                                                role: ChatRole::User,
                                                text: prompt.clone(),
                                                timestamp: current_time_str(),
                                                quick_suggestions: Vec::new(),
                                                screen_size_kb: None,
                                            });
                                            if is_cancel_scheduled_command(&prompt) {
                                                let cancelled = cancel_all_scheduled_tasks();
                                                play_earcon(Earcon::Success);
                                                let reply = if cancelled > 0 {
                                                    format!("Toutes les commandes différées ont été annulées ({cancelled} annulée{}).", if cancelled > 1 { "s" } else { "" })
                                                } else {
                                                    "Toutes les commandes différées ont été annulées.".to_string()
                                                };
                                                self.chat_history.push(ChatEntry {
                                                    role: ChatRole::Agent,
                                                    text: reply.clone(),
                                                    timestamp: current_time_str(),
                                                    quick_suggestions: Vec::new(),
                                                    screen_size_kb: None,
                                                });
                                                let _ = self.tts_sender.send(TtsCommand::Speak(reply));
                                                self.status = AgentStatus::Idle;
                                                self.continuous_mode = false;
                                                return;
                                            }
                                            if is_api_key_command(&prompt) {
                                                self.sync_modal_inputs_for_provider(self.llm_config.provider);
                                                self.show_api_key_modal = true;
                                                let reply = "Boîte de configuration de la clé API ouverte.".to_string();
                                                self.chat_history.push(ChatEntry {
                                                    role: ChatRole::Agent,
                                                    text: reply.clone(),
                                                    timestamp: current_time_str(),
                                                    quick_suggestions: Vec::new(),
                                                    screen_size_kb: None,
                                                });
                                                let _ = self.tts_sender.send(TtsCommand::Speak(reply));
                                                self.status = AgentStatus::Idle;
                                                self.continuous_mode = false;
                                            } else if is_hide_command(&prompt) {
                                                self.is_hidden = true;
                                                self.status = AgentStatus::Idle;
                                                let reply = "Overlay masqué. Je reste à l'écoute pour « deepseek ouvre toi ».".to_string();
                                                self.chat_history.push(ChatEntry {
                                                    role: ChatRole::Agent,
                                                    text: reply.clone(),
                                                    timestamp: current_time_str(),
                                                    quick_suggestions: Vec::new(),
                                                    screen_size_kb: None,
                                                });
                                                let _ = self.tts_sender.send(TtsCommand::Speak(reply));
                                            } else {
                                                self.status = AgentStatus::Thinking;
                                                let _ = self.command_sender.send(AgentCommand::Prompt(prompt));
                                            }
                                        }
                                    });

                                    ui.add_space(3.0);

                                    // 2e rangée : Boutons d'action, dictée, profil vocal, clé API et corbeille
                                    ui.horizontal(|ui| {
                                        let mic_text = if self.is_recording {
                                            egui::RichText::new("🔴 Stop (r)")
                                                .color(egui::Color32::from_rgb(255, 90, 90))
                                                .strong()
                                        } else {
                                            egui::RichText::new("🎙 Rec (r)")
                                                .color(egui::Color32::from_rgb(180, 220, 255))
                                        };

                                        let mic_btn = ui.button(mic_text).on_hover_text(
                                            "Activer ou désactiver l'écoute instantanée (raccourci r)",
                                        );

                                        if mic_btn.clicked() {
                                            self.toggle_recording(ui.ctx());
                                        }

                                        let dict_text = if self.dictation_mode {
                                            egui::RichText::new("✍ Dictée ON")
                                                .color(egui::Color32::from_rgb(255, 205, 70))
                                                .strong()
                                        } else {
                                            egui::RichText::new("✍ Dictée")
                                                .color(egui::Color32::from_rgb(180, 210, 235))
                                        };
                                        let dict_btn = ui.button(dict_text).on_hover_text(
                                            "Activer ou désactiver le mode dictée vocale directe (Échap pour quitter)",
                                        );
                                        if dict_btn.clicked() {
                                            if self.dictation_mode {
                                                self.exit_dictation_mode();
                                            } else {
                                                self.dictation_mode = true;
                                                IS_DICTATION_ACTIVE.store(true, Ordering::SeqCst);
                                                restore_external_focus();
                                                if !self.is_recording {
                                                    self.start_recording(ui.ctx(), true);
                                                }
                                            }
                                        }

                                        let switch_text = if self.switch_access_mode {
                                            egui::RichText::new("🎯 Survol ON")
                                                .color(egui::Color32::from_rgb(0, 255, 200))
                                                .strong()
                                        } else {
                                            egui::RichText::new("🎯 Survol")
                                                .color(egui::Color32::from_rgb(180, 210, 235))
                                        };
                                        let switch_btn = ui.button(switch_text).on_hover_text(
                                            "Activer ou désactiver le mode survol automatique + contacteur (Entrée pour valider)",
                                        );
                                        if switch_btn.clicked() {
                                            self.toggle_switch_access_mode();
                                        }

                                        // Sélecteur visuel de profil vocal étendu
                                        let (voice_icon_text, voice_color) = match self.voice_gender {
                                            VoiceProfile::Denise => ("👩 Denise", egui::Color32::from_rgb(255, 170, 220)),
                                            VoiceProfile::Henri => ("👨 Henri", egui::Color32::from_rgb(130, 200, 255)),
                                            VoiceProfile::Hortense => ("🎙️ Hortense", egui::Color32::from_rgb(255, 195, 130)),
                                            VoiceProfile::Paul => ("🎩 Paul", egui::Color32::from_rgb(160, 220, 180)),
                                            VoiceProfile::Sylvie => ("🍁 Sylvie", egui::Color32::from_rgb(255, 150, 150)),
                                            VoiceProfile::Jean => ("⚡ Jean", egui::Color32::from_rgb(205, 170, 255)),
                                        };
                                        let next_gender = self.voice_gender.next();
                                        let voice_btn = ui.button(egui::RichText::new(voice_icon_text).color(voice_color))
                                            .on_hover_text(format!(
                                                "Voix active : {}\nClic gauche : basculer vers {}\nClic droit : choisir parmi toutes les voix",
                                                self.voice_gender.display_name(),
                                                next_gender.display_name()
                                            ));
                                        if voice_btn.clicked() {
                                            self.set_voice_gender(next_gender);
                                        }
                                        voice_btn.context_menu(|ui| {
                                            for &profile in VoiceProfile::ALL {
                                                let is_selected = self.voice_gender == profile;
                                                if ui.selectable_label(is_selected, profile.display_name()).clicked() {
                                                    self.set_voice_gender(profile);
                                                    ui.close();
                                                }
                                            }
                                        });

                                        let key_btn = ui
                                            .button("⚙")
                                            .on_hover_text("Paramètres (Fournisseurs LLM, clé API, Thèmes & Accessibilité)");
                                        if key_btn.clicked() {
                                            self.settings_tab = SettingsTab::LlmProvider;
                                            self.show_api_key_modal = true;
                                            self.sync_modal_inputs_for_provider(self.llm_config.provider);
                                            self.api_key_error = None;
                                            self.stop_speaking();
                                        }

                                        let clear_btn = ui
                                            .button("🗑")
                                            .on_hover_text("Effacer l'historique et la mémoire contextuelle");
                                        if clear_btn.clicked() {
                                            self.command_sender.send(AgentCommand::ClearHistory).ok();
                                            self.chat_history.clear();
                                            self.chat_history.push(ChatEntry {
                                                role: ChatRole::Agent,
                                                text: "Mémoire contextuelle réinitialisée.".to_string(),
                                                timestamp: current_time_str(),
                                                quick_suggestions: Vec::new(),
                                                screen_size_kb: None,
                                            });
                                            self.live_transcript.clear();
                                            let _ = self.tts_sender.send(TtsCommand::Stop);
                                        }
                                    });
                                });
                            });
                        });
                });

                // 3. Boîte de discussion adaptative (évaluée après les contrôles pour la navigation Tab)
                let mut computed_chat_h = current_chat_h;
                let messages_rect = egui::Rect::from_min_max(
                    egui::pos2(response_rect.min.x + 8.0, sep_y + 4.0),
                    egui::pos2(response_rect.max.x - 8.0, response_rect.max.y - 6.0),
                );

                ui.scope_builder(egui::UiBuilder::new().max_rect(messages_rect), |ui| {
                    match self.active_tab {
                        ActiveTab::Assistance => {
                            let max_scroll_h = (messages_rect.height()).max(20.0);
                            let scroll_out = egui::ScrollArea::vertical()
                                .id_salt("assistance_chat_scroll")
                                .stick_to_bottom(true)
                                .max_height(max_scroll_h)
                                .auto_shrink([false, true])
                                .show(ui, |ui| {
                                    let last_user_idx = self.chat_history.iter().rposition(|e| e.role == ChatRole::User);
                                    let last_agent_idx = self.chat_history.iter().rposition(|e| e.role == ChatRole::Agent);
                                    let mut clicked_quick_suggestion: Option<String> = None;
                                    let mut request_stop_speaking = false;

                                    for (idx, entry) in self.chat_history.iter().enumerate() {
                                        let is_highlighted = Some(idx) == last_user_idx || Some(idx) == last_agent_idx;
                                        let msg_id = ui.make_persistent_id(format!("chat_entry_item_{idx}"));
                                        let layer_id = ui.layer_id();
                                        ui.memory_mut(|m| m.interested_in_focus(msg_id, layer_id));
                                        let is_msg_focused = ui.memory(|m| m.has_focus(msg_id));

                                        ui.vertical(|ui| {
                                            ui.horizontal(|ui| {
                                                let (label_name, label_color) = match entry.role {
                                                    ChatRole::User => {
                                                        let name = if let Some(kb) = entry.screen_size_kb {
                                                            format!("🗣 Vous - {:.1} ko", kb)
                                                        } else {
                                                            "🗣 Vous".to_string()
                                                        };
                                                        (name, egui::Color32::from_rgb(255, 215, 120))
                                                    }
                                            ChatRole::Agent => (self.llm_config.provider.display_name().to_string(), theme.accent),
                                                };
                                                ui.label(
                                                    egui::RichText::new(&label_name)
                                                        .size(if is_highlighted { 11.5 } else { 10.0 })
                                                        .color(label_color)
                                                        .strong(),
                                                );
                                                ui.label(
                                                    egui::RichText::new(&entry.timestamp)
                                                        .size(if is_highlighted { 10.0 } else { 9.0 })
                                                        .color(egui::Color32::from_rgba_unmultiplied(170, 185, 205, 140)),
                                                );

                                                if entry.role == ChatRole::Agent && Some(idx) == last_agent_idx && self.status == AgentStatus::Speaking {
                                                    ui.add_space(4.0);
                                                    let stop_btn = ui.add(
                                                        egui::Button::new(
                                                            egui::RichText::new("🔇 Couper")
                                                                .size(9.5)
                                                                .color(egui::Color32::from_rgb(255, 130, 130)),
                                                        )
                                                        .small()
                                                        .corner_radius(egui::CornerRadius::same(6)),
                                                    );
                                                    if stop_btn.on_hover_text("Interrompre la lecture vocale (Échap)").clicked() {
                                                        request_stop_speaking = true;
                                                    }
                                                }
                                            });

                                            let (text_size, line_height, text_color, is_strong) = match entry.role {
                                                ChatRole::User => {
                                                    if is_highlighted {
                                                        (14.5, 20.0, theme.text_user, true)
                                                    } else {
                                                        (12.0, 16.5, egui::Color32::from_rgba_unmultiplied(225, 210, 160, 185), false)
                                                    }
                                                }
                                                ChatRole::Agent => {
                                                    if is_highlighted {
                                                        (15.5, 22.0, egui::Color32::from_rgb(255, 255, 255), true)
                                                    } else {
                                                        (12.5, 17.5, egui::Color32::from_rgba_unmultiplied(200, 210, 225, 180), false)
                                                    }
                                                }
                                            };

                                            let has_interactive_md = (entry.text.contains('[') && entry.text.contains("]("))
                                                || entry.text.contains("[link")
                                                || entry.text.contains("[/link]")
                                                || entry.text.contains("```")
                                                || entry.text.contains("▶ ")
                                                || entry.text.contains("🎬 ")
                                                || entry.text.contains("🎵 ")
                                                || entry.text.contains("📄 ")
                                                || entry.text.contains("🖼")
                                                || (entry.text.contains('|') && entry.text.contains('\n'));

                                            if entry.role == ChatRole::Agent && has_interactive_md {
                                                if let Some(prompt) = floating_window::render_markdown(ui, &entry.text, &format!("chat_entry_{idx}")) {
                                                    clicked_quick_suggestion = Some(prompt);
                                                }
                                            } else {
                                            let mut rich = egui::RichText::new(&entry.text)
                                                .size(text_size)
                                                .line_height(Some(line_height))
                                                .color(text_color);
                                            if is_strong {
                                                rich = rich.strong();
                                            }
                                            let label_resp = ui.add(egui::Label::new(rich).wrap());
                                            let interact_resp = ui.interact(label_resp.rect, msg_id, egui::Sense::click());

                                            if interact_resp.clicked() {
                                                ui.memory_mut(|m| m.request_focus(msg_id));
                                            }
                                            let effective_focused = is_msg_focused || interact_resp.has_focus();

                                            // Copie au clavier Ctrl+C dès que le message a le focus clavier (ou au survol sans focus actif ailleurs)
                                            let copy_msg_hit = ctx.input(|i| {
                                                i.events.iter().any(|e| matches!(e, egui::Event::Copy))
                                                    || ((i.modifiers.ctrl || i.modifiers.command) && i.key_pressed(egui::Key::C))
                                            });
                                            let should_copy = effective_focused
                                                || (interact_resp.hovered() && ui.memory(|m| m.focused().is_none()));

                                            if should_copy && copy_msg_hit {
                                                set_clipboard_text(&entry.text);
                                                ctx.copy_text(entry.text.clone());
                                                play_earcon(Earcon::Success);
                                            }

                                            if effective_focused {
                                                label_resp.scroll_to_me(Some(egui::Align::Center));
                                                ui.painter().rect_stroke(
                                                    label_resp.rect.expand(4.0),
                                                    egui::CornerRadius::same(6),
                                                    egui::Stroke::new(1.8, theme.accent),
                                                    egui::StrokeKind::Outside,
                                                );
                                            }

                                            interact_resp.context_menu(|ui| {
                                                if ui.button("📋 Copier le texte (Ctrl+C)").clicked() {
                                                    set_clipboard_text(&entry.text);
                                                    ctx.copy_text(entry.text.clone());
                                                    play_earcon(Earcon::Success);
                                                    ui.close();
                                                }
                                            });
                                            }

                                            // Boutons-onglets pour les suggestions rapides
                                            if !entry.quick_suggestions.is_empty() {
                                                ui.add_space(4.0);
                                                ui.horizontal_wrapped(|ui| {
                                                    for sug in &entry.quick_suggestions {
                                                        if let Some(clean_label) = sug.to_display_string() {
                                                            let clean_label = clean_label.trim();
                                                            if clean_label.is_empty() {
                                                                continue;
                                                            }
                                                            let btn_text = egui::RichText::new(format!("💡 {clean_label}"))
                                                                .size(11.0)
                                                                .color(theme.accent);

                                                            let btn = egui::Button::new(btn_text)
                                                                .stroke(egui::Stroke::new(1.0, theme.border))
                                                                .corner_radius(egui::CornerRadius::same(10));

                                                            if ui.add(btn).on_hover_text("Cliquer pour choisir cette suggestion").clicked() {
                                                                clicked_quick_suggestion = Some(clean_label.to_string());
                                                            }
                                                        }
                                                    }
                                                });
                                            }
                                        });
                                        ui.add_space(if is_highlighted { 8.0 } else { 5.0 });
                                    }

                                    if let Some(prompt) = clicked_quick_suggestion {
                                        self.stop_speaking();
                                        if self.is_recording {
                                            self.stop_recording();
                                        }
                                        self.status = AgentStatus::Thinking;
                                        self.continuous_mode = true;
                                        self.live_transcript.clear();
                                        self.chat_history.push(ChatEntry {
                                            role: ChatRole::User,
                                            text: prompt.clone(),
                                            timestamp: current_time_str(),
                                            quick_suggestions: Vec::new(),
                                            screen_size_kb: None,
                                        });
                                        let _ = self.command_sender.send(AgentCommand::Prompt(prompt));
                                    }

                                    if request_stop_speaking {
                                        self.stop_speaking();
                                    }
                                });
                            computed_chat_h = (scroll_out.content_size.y + 44.0).clamp(min_chat_h, max_chat_h);
                        }
                        ActiveTab::Chat => {
                            // Barre de recherche et salon actif
                            ui.horizontal(|ui| {
                                if !self.current_twitch_channel.is_empty() {
                                    ui.label(
                                        egui::RichText::new("Salon :")
                                            .size(11.0)
                                            .color(egui::Color32::from_rgb(170, 185, 205)),
                                    );
                                    ui.hyperlink_to(
                                        egui::RichText::new(format!("#{}", self.current_twitch_channel))
                                            .size(11.0)
                                            .color(egui::Color32::from_rgb(0, 195, 255))
                                            .underline(),
                                        format!("https://twitch.tv/{}", self.current_twitch_channel),
                                    );
                                    ui.separator();
                                }

                                let search_edit = ui.add_sized(
                                    [110.0, 18.0],
                                    egui::TextEdit::singleline(&mut self.twitch_search_query)
                                        .hint_text("Chaîne..."),
                                );
                                let enter_pressed = search_edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                                if (ui.small_button("🔍").clicked() || enter_pressed) && !self.twitch_search_query.trim().is_empty() {
                                    let _ = self.command_sender.send(AgentCommand::SearchTwitch(
                                        self.twitch_search_query.trim().to_string(),
                                    ));
                                }
                            });

                            if !self.twitch_search_results.is_empty() {
                                ui.add_space(2.0);
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(
                                        egui::RichText::new("Résultats :")
                                            .size(10.5)
                                            .color(egui::Color32::from_rgb(255, 215, 120)),
                                    );
                                    let mut selected_channel = None;
                                    for ch in &self.twitch_search_results {
                                        if ui.small_button(format!("▶ {}", ch.display_name)).on_hover_text("Rejoindre ce salon irc").clicked() {
                                            selected_channel = Some(ch.login.clone());
                                        }
                                        if ui.small_button("↗").on_hover_text("Ouvrir sur twitch").clicked() {
                                            ui.ctx().open_url(egui::OpenUrl::new_tab(format!("https://twitch.tv/{}", ch.login)));
                                        }
                                    }
                                    if let Some(ch) = selected_channel {
                                        self.current_twitch_channel = ch.clone();
                                        let _ = self.twitch_channel_sender.send(ch);
                                        self.twitch_messages.clear();
                                        self.twitch_search_results.clear();
                                    }
                                });
                            }
                            ui.separator();
                            let max_chat_scroll_h = (messages_rect.height() - 28.0).max(20.0);
                            let scroll_out = egui::ScrollArea::vertical()
                                .id_salt("twitch_chat_scroll")
                                .stick_to_bottom(true)
                                .max_height(max_chat_scroll_h)
                                .auto_shrink([false, true])
                                .show(ui, |ui| {
                                    if self.twitch_messages.is_empty() {
                                        ui.label(
                                            egui::RichText::new("En attente de messages twitch...")
                                                .size(12.0)
                                                .italics()
                                                .color(egui::Color32::from_rgba_unmultiplied(180, 190, 205, 150)),
                                        );
                                    } else {
                                        for (idx, msg) in self.twitch_messages.iter().enumerate() {
                                            let t_id = ui.make_persistent_id(format!("twitch_msg_item_{idx}"));
                                            let layer_id = ui.layer_id();
                                            ui.memory_mut(|m| m.interested_in_focus(t_id, layer_id));
                                            let is_t_focused = ui.memory(|m| m.has_focus(t_id));
                                            let resp = ui.horizontal_wrapped(|ui| {
                                                ui.label(
                                                    egui::RichText::new(&msg.timestamp)
                                                        .size(9.0)
                                                        .color(egui::Color32::from_rgba_unmultiplied(140, 150, 170, 140)),
                                                );
                                                ui.label(
                                                    egui::RichText::new(format!("{}:", msg.author))
                                                        .size(12.0)
                                                        .color(egui::Color32::from_rgb(169, 112, 255))
                                                        .strong(),
                                                );
                                                ui.label(
                                                    egui::RichText::new(&msg.text)
                                                        .size(12.5)
                                                        .color(egui::Color32::WHITE),
                                                );
                                            }).response;
                                            let resp = ui.interact(resp.rect, t_id, egui::Sense::click());

                                            if resp.clicked() {
                                                ui.memory_mut(|m| m.request_focus(t_id));
                                            }
                                            let effective_t_focused = is_t_focused || resp.has_focus();
                                            let copy_t_hit = ctx.input(|i| {
                                                i.events.iter().any(|e| matches!(e, egui::Event::Copy))
                                                    || ((i.modifiers.ctrl || i.modifiers.command) && i.key_pressed(egui::Key::C))
                                            });
                                            if (effective_t_focused || resp.hovered()) && copy_t_hit {
                                                let text = format!("{}: {}", msg.author, msg.text);
                                                set_clipboard_text(&text);
                                                ctx.copy_text(text);
                                                play_earcon(Earcon::Success);
                                            }
                                            if effective_t_focused {
                                                resp.scroll_to_me(Some(egui::Align::Center));
                                                ui.painter().rect_stroke(
                                                    resp.rect.expand(2.0),
                                                    egui::CornerRadius::same(3),
                                                    egui::Stroke::new(1.2, egui::Color32::from_rgb(0, 200, 255)),
                                                    egui::StrokeKind::Outside,
                                                );
                                            }
                                            ui.add_space(3.0);
                                        }
                                    }
                                });
                            computed_chat_h = (scroll_out.content_size.y + 72.0).clamp(min_chat_h, max_chat_h);
                        }
                    }
                });

                // Hystérésis asymétrique pour self.chat_height
                if computed_chat_h > self.chat_height + 4.0 || computed_chat_h < self.chat_height - 8.0 {
                    self.chat_height = computed_chat_h;
                }

                // Zones de déplacement : en-tête supérieur (en réservant l'espace des boutons à droite) et bandeau inférieur
                let window_buttons_width = 84.0;
                let drag_rect_top = egui::Rect::from_min_max(
                    egui::pos2(tab_bar_max_x + 6.0, response_rect.min.y),
                    egui::pos2((win_w - padding - window_buttons_width).max(tab_bar_max_x + 6.0), response_rect.min.y + 30.0),
                );
                let drag_rect_bot = egui::Rect::from_min_max(
                    egui::pos2(control_rect.min.x + 8.0, control_rect.min.y + 4.0),
                    egui::pos2(control_rect.max.x - 48.0, control_rect.min.y + 24.0),
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
                    if let Some(cursor) = get_screen_cursor_pos(&ctx) {
                        let win_p = self.window_pos.unwrap_or_default();
                        self.drag_offset = Some(egui::vec2(cursor.x - win_p.x, cursor.y - win_p.y));
                    }
                }

                if is_dragged && !is_maximized {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                    let offset = self.drag_offset.unwrap_or(egui::vec2(win_w / 2.0, 16.0));

                    let cursor_screen = get_screen_cursor_pos(&ctx);
                    if let Some(cursor) = cursor_screen {
                        let max_x = (work_x + work_w - win_w).max(work_x);
                        let max_y = (taskbar_top - active_win_h).max(work_y);

                        let raw_x = cursor.x - offset.x;
                        let raw_y = cursor.y - offset.y;

                        // Magnétisme à la barre des tâches : si le bas de l'overlay s'en approche à moins de 24 px
                        let target_y = if (raw_y - max_y).abs() < 24.0 || raw_y >= max_y {
                            max_y
                        } else {
                            raw_y.clamp(work_y, max_y)
                        };
                        let target_x = raw_x.clamp(work_x, max_x);

                        let new_pos = egui::pos2(target_x, target_y);
                        self.window_pos = Some(new_pos);
                        self.window_bottom_y = Some(new_pos.y + active_win_h);
                        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(new_pos));
                    }
                }
                if drag_stopped {
                    self.drag_offset = None;
                }

                // Zones de redimensionnement interactif sur l'ensemble des bordures et coins (haut, côtés, bas)
                let border_thickness = 6.0;
                let total_w = win_w;
                let total_h = active_win_h;

                let resize_zones = [
                    // Bord haut et coins hauts
                    (egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(border_thickness, border_thickness)), egui::viewport::ResizeDirection::NorthWest, egui::CursorIcon::ResizeNorthWest),
                    (egui::Rect::from_min_size(egui::pos2(total_w - border_thickness, 0.0), egui::vec2(border_thickness, border_thickness)), egui::viewport::ResizeDirection::NorthEast, egui::CursorIcon::ResizeNorthEast),
                    (egui::Rect::from_min_size(egui::pos2(border_thickness, 0.0), egui::vec2((total_w - 2.0 * border_thickness).max(0.0), border_thickness)), egui::viewport::ResizeDirection::North, egui::CursorIcon::ResizeNorth),

                    // Côtés gauche et droite
                    (egui::Rect::from_min_size(egui::pos2(0.0, border_thickness), egui::vec2(border_thickness, (total_h - 2.0 * border_thickness).max(0.0))), egui::viewport::ResizeDirection::West, egui::CursorIcon::ResizeWest),
                    (egui::Rect::from_min_size(egui::pos2(total_w - border_thickness, border_thickness), egui::vec2(border_thickness, (total_h - 2.0 * border_thickness).max(0.0))), egui::viewport::ResizeDirection::East, egui::CursorIcon::ResizeEast),

                    // Bord bas et coins bas (South, SouthWest, SouthEast)
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

                // Poignées visuelles de redimensionnement discrètes aux coins
                let painter = ui.painter();

                // Tracé des badges de ciblage numéroté (Click by Number)
                if self.show_numbered_targets {
                    for target in &self.numbered_targets {
                        let ppp = ctx.pixels_per_point().max(0.5);
                        let left = target.rect[0] as f32 / ppp;
                        let top = target.rect[1] as f32 / ppp;
                        let badge_rect = egui::Rect::from_min_size(egui::pos2(left, top), egui::vec2(22.0, 18.0));
                        
                        painter.rect_filled(
                            badge_rect,
                            egui::CornerRadius::same(4),
                            egui::Color32::from_rgb(255, 215, 0),
                        );
                        painter.rect_stroke(
                            badge_rect,
                            egui::CornerRadius::same(4),
                            egui::Stroke::new(1.2, egui::Color32::BLACK),
                            egui::StrokeKind::Outside,
                        );
                        painter.text(
                            badge_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            target.index.to_string(),
                            egui::FontId::monospace(11.0),
                            egui::Color32::BLACK,
                        );
                    }
                }


                // Halo visuel pulsé entourant la zone de navigation analysée par le LLM
                if self.switch_access_mode && self.switch_step == SwitchAccessStep::Zones && !self.switch_zones.is_empty() {
                    let cur_zone = &self.switch_zones[self.switch_focus_index % self.switch_zones.len()];
                    let ppp = ctx.pixels_per_point().max(0.5);
                    let z_rect = egui::Rect::from_min_max(
                        egui::pos2(cur_zone.rect[0] as f32 / ppp, cur_zone.rect[1] as f32 / ppp),
                        egui::pos2(cur_zone.rect[2] as f32 / ppp, cur_zone.rect[3] as f32 / ppp),
                    );
                    let pulse = ((time * 4.0).sin().abs() as f32) * 0.3 + 0.7;
                    painter.rect_stroke(
                        z_rect.expand(6.0),
                        egui::CornerRadius::same(8),
                        egui::Stroke::new(2.5, egui::Color32::from_rgba_unmultiplied(0, (220.0 * pulse) as u8, 255, 230)),
                        egui::StrokeKind::Outside,
                    );
                }

                // Indicateur visuel du mode Switch Access
                if self.switch_access_mode {
                    let pulse = ((time * 4.0).sin().abs() as f32) * 0.4 + 0.6;
                    let stroke_color = egui::Color32::from_rgba_unmultiplied(
                        (0.0 * pulse) as u8,
                        (255.0 * pulse) as u8,
                        (200.0 * pulse) as u8,
                        255,
                    );
                    painter.rect_stroke(
                        control_rect.expand(2.0),
                        egui::CornerRadius::same(12),
                        egui::Stroke::new(2.5, stroke_color),
                        egui::StrokeKind::Outside,
                    );
                }
                // Tracé de l'outline rect de la zone cible en mode dictée
                if self.dictation_mode {
                    if let Ok(guard) = LAST_FOCUSED_RECT.lock() {
                        if let Some([left, top, right, bottom]) = *guard {
                            let ppp = ctx.pixels_per_point().max(0.5);
                            let screen_rect = egui::Rect::from_min_max(
                                egui::pos2(left as f32 / ppp, top as f32 / ppp),
                                egui::pos2(right as f32 / ppp, bottom as f32 / ppp),
                            );
                            // Dessin d'un halo semi-transparent et d'une bordure dorée
                            ui.painter().rect_stroke(
                                screen_rect,
                                egui::CornerRadius::same(4),
                                egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 205, 70)),
                                egui::StrokeKind::Outside,
                            );
                        }
                    }
                }
                let grip_nw = egui::Rect::from_min_size(egui::pos2(padding + 2.0, padding + 2.0), egui::vec2(10.0, 10.0));
                painter.line_segment(
                    [egui::pos2(grip_nw.min.x, grip_nw.max.y), egui::pos2(grip_nw.max.x, grip_nw.min.y)],
                    egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(0, 180, 255, 90)),
                );
                painter.line_segment(
                    [egui::pos2(grip_nw.min.x + 3.0, grip_nw.max.y), egui::pos2(grip_nw.max.x, grip_nw.min.y + 3.0)],
                    egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(0, 180, 255, 50)),
                );

                let grip_se = egui::Rect::from_min_size(egui::pos2(total_w - padding - 12.0, total_h - padding - 12.0), egui::vec2(10.0, 10.0));
                painter.line_segment(
                    [egui::pos2(grip_se.min.x, grip_se.max.y), egui::pos2(grip_se.max.x, grip_se.min.y)],
                    egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(0, 180, 255, 90)),
                );
                painter.line_segment(
                    [egui::pos2(grip_se.min.x + 3.0, grip_se.max.y), egui::pos2(grip_se.max.x, grip_se.min.y + 3.0)],
                    egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(0, 180, 255, 50)),
                );

                // Boîte modale de configuration (Fournisseurs LLM & Thèmes d'accessibilité)
                if self.show_api_key_modal {
                    egui::Window::new("settings_modal")
                        .title_bar(false)
                        .collapsible(false)
                        .resizable(false)
                        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                        .frame(
                            egui::Frame::new()
                                .fill(theme.modal_bg)
                                .stroke(egui::Stroke::new(1.2, theme.border))
                                .corner_radius(egui::CornerRadius::same(10))
                                .inner_margin(egui::Margin::symmetric(12, 10)),
                        )
                        .show(&ctx, |ui| {
                            // En-tête avec onglets de configuration
                            ui.horizontal(|ui| {
                                let tab_llm = ui.selectable_label(
                                    self.settings_tab == SettingsTab::LlmProvider,
                                    egui::RichText::new("🔑 Fournisseur LLM")
                                        .size(12.0)
                                        .color(if self.settings_tab == SettingsTab::LlmProvider {
                                            theme.accent
                                        } else {
                                            theme.text_secondary
                                        })
                                        .strong(),
                                );
                                if tab_llm.clicked() {
                                    self.settings_tab = SettingsTab::LlmProvider;
                                }
                                let tab_theme = ui.selectable_label(
                                    self.settings_tab == SettingsTab::Theme,
                                    egui::RichText::new("🎨 Thèmes & Accessibilité")
                                        .size(12.0)
                                        .color(if self.settings_tab == SettingsTab::Theme {
                                            theme.accent
                                        } else {
                                            theme.text_secondary
                                        })
                                        .strong(),
                                );
                                if tab_theme.clicked() {
                                    self.settings_tab = SettingsTab::Theme;
                                }
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.add(
                                        egui::Button::new(
                                            egui::RichText::new("✕")
                                                .size(11.0)
                                                .color(egui::Color32::from_rgb(255, 130, 130)),
                                        )
                                        .min_size(egui::vec2(18.0, 16.0))
                                        .corner_radius(egui::CornerRadius::same(4)),
                                    ).on_hover_text("Fermer").clicked() {
                                        self.show_api_key_modal = false;
                                        self.api_key_error = None;
                                    }
                                });
                            });
                            ui.add_space(4.0);
                            ui.separator();
                            ui.add_space(4.0);

                            match self.settings_tab {
                                SettingsTab::LlmProvider => {
                            // --- Onglet Fournisseur LLM ---
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing = egui::vec2(4.0, 3.0);
                                for &provider in LlmProvider::ALL {
                                    let is_sel = self.selected_provider_modal == provider;
                                    if ui.selectable_label(is_sel, provider.display_name()).clicked() {
                                        self.sync_modal_inputs_for_provider(provider);
                                    }
                                }
                            });

                            ui.add_space(4.0);
                            let env_var_target = self.selected_provider_modal.env_key_name();
                            let env_model_target = self.selected_provider_modal.env_model_name();
                            let default_model_name = self.selected_provider_modal.default_model();

                            // --- Section Clé API ---
                            ui.label(
                                egui::RichText::new(format!("Clé d'API ({env_var_target}) :"))
                                    .size(11.0)
                                    .color(egui::Color32::WHITE)
                                    .strong(),
                            );
                            ui.add_space(2.0);

                            ui.horizontal(|ui| {
                                let key_input_id = egui::Id::new("api_key_input_field");
                                let is_key_focused = ctx.memory(|m| m.has_focus(key_input_id));
                                let paste_key_hit = ctx.input(|i| {
                                    i.events.iter().any(|e| matches!(e, egui::Event::Paste(_)))
                                        || ((i.modifiers.ctrl || i.modifiers.command) && i.key_pressed(egui::Key::V))
                                        || (i.modifiers.shift && i.key_pressed(egui::Key::Insert))
                                });
                                if is_key_focused && paste_key_hit {
                                    if let Some(clip) = get_clipboard_text().or_else(get_plain_clipboard_text) {
                                        let trimmed = clip.trim().to_string();
                                        if !trimmed.is_empty() {
                                            self.api_key_input = trimmed;
                                            play_earcon(Earcon::Success);
                                        }
                                    }
                                    ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::V));
                                    ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::V));
                                }
                                let edit = egui::TextEdit::singleline(&mut self.api_key_input)
                                    .id(key_input_id)
                                    .password(!self.show_api_key_plain)
                                    .hint_text("sk-...")
                                    .desired_width(230.0);
                                let edit_resp = ui.add(edit);

                                let eye_icon = if self.show_api_key_plain { "🔒" } else { "👁" };
                                if ui.button(eye_icon).on_hover_text("Afficher/Masquer la clé").clicked() {
                                    self.show_api_key_plain = !self.show_api_key_plain;
                                }

                                if ui.button("📋").on_hover_text("Coller depuis le presse-papiers").clicked() {
                                    if let Some(clip) = get_clipboard_text() {
                                        self.api_key_input = clip.trim().to_string();
                                    }
                                }

                                edit_resp.context_menu(|ui| {
                                    if ui.button("📋 Coller").clicked() {
                                        if let Some(clip) = get_clipboard_text() {
                                            self.api_key_input = clip.trim().to_string();
                                        }
                                        ui.close();
                                    }
                                    if ui.button("📄 Copier").clicked() {
                                        set_clipboard_text(&self.api_key_input);
                                        ui.close();
                                    }
                                    if ui.button("🗑 Effacer").clicked() {
                                        self.api_key_input.clear();
                                        ui.close();
                                    }
                                });

                                let copy_key_hit = ctx.input(|i| {
                                    i.events.iter().any(|e| matches!(e, egui::Event::Copy))
                                        || ((i.modifiers.ctrl || i.modifiers.command) && i.key_pressed(egui::Key::C))
                                });
                                if edit_resp.has_focus() && copy_key_hit && !self.api_key_input.is_empty() {
                                    set_clipboard_text(&self.api_key_input);
                                    ctx.copy_text(self.api_key_input.clone());
                                    play_earcon(Earcon::Success);
                                }

                            });

                            ui.add_space(5.0);

                            // --- Section Modèle de raisonnement ---
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(format!("Modèle LLM ({env_model_target}) :"))
                                        .size(11.0)
                                        .color(egui::Color32::WHITE)
                                        .strong(),
                                );
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.small_button("↺ Recommandé")
                                        .on_hover_text(format!("Rétablir le modèle par défaut ({default_model_name})"))
                                        .clicked()
                                    {
                                        self.model_input = default_model_name.to_string();
                                    }
                                });
                            });
                            ui.add_space(2.0);

                            ui.horizontal(|ui| {
                                let model_input_id = egui::Id::new("model_input_field");
                                let is_model_focused = ctx.memory(|m| m.has_focus(model_input_id));
                                let paste_model_hit = ctx.input(|i| {
                                    i.events.iter().any(|e| matches!(e, egui::Event::Paste(_)))
                                        || ((i.modifiers.ctrl || i.modifiers.command) && i.key_pressed(egui::Key::V))
                                        || (i.modifiers.shift && i.key_pressed(egui::Key::Insert))
                                });
                                if is_model_focused && paste_model_hit {
                                    if let Some(clip) = get_clipboard_text().or_else(get_plain_clipboard_text) {
                                        let trimmed = clip.trim().to_string();
                                        if !trimmed.is_empty() {
                                            self.model_input = trimmed;
                                            play_earcon(Earcon::Success);
                                        }
                                    }
                                    ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::V));
                                    ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::V));
                                }
                                let model_edit = egui::TextEdit::singleline(&mut self.model_input)
                                    .id(model_input_id)
                                    .hint_text(default_model_name)
                                    .desired_width(240.0);
                                let model_resp = ui.add(model_edit);

                                model_resp.context_menu(|ui| {
                                    if ui.button("📋 Coller").clicked() {
                                        if let Some(clip) = get_clipboard_text() {
                                            self.model_input = clip.trim().to_string();
                                        }
                                        ui.close();
                                    }
                                    if ui.button("📄 Copier").clicked() {
                                        set_clipboard_text(&self.model_input);
                                        ui.close();
                                    }
                                    if ui.button(format!("↺ Rétablir {default_model_name}")).clicked() {
                                        self.model_input = default_model_name.to_string();
                                        ui.close();
                                    }
                                    if ui.button("🗑 Effacer").clicked() {
                                        self.model_input.clear();
                                        ui.close();
                                    }
                                });

                            });

                            if let Some(ref err) = self.api_key_error {
                                ui.add_space(2.0);
                                ui.label(egui::RichText::new(err).color(egui::Color32::from_rgb(255, 110, 110)).size(11.0));
                            }

                            ui.add_space(5.0);
                            ui.horizontal(|ui| {
                                if ui.button("💾 Enregistrer & Appliquer").clicked() {
                                    let key = self.api_key_input.trim();
                                    let model = if self.model_input.trim().is_empty() {
                                        default_model_name
                                    } else {
                                        self.model_input.trim()
                                    };

                                    if key.is_empty() {
                                        self.api_key_error = Some("La clé API ne peut pas être vide.".to_string());
                                    } else if let Err(e) = write_env_var_to_file(".env", env_var_target, key) {
                                        self.api_key_error = Some(format!("Erreur d'écriture clé : {e}"));
                                    } else if let Err(e) = write_env_var_to_file(".env", env_model_target, model) {
                                        self.api_key_error = Some(format!("Erreur d'écriture modèle : {e}"));
                                    } else {
                                        let _ = write_env_var_to_file(".env", "LLM_PROVIDER", self.selected_provider_modal.display_name());
                                        #[cfg(windows)]
                                        restart_application();
                                        self.show_api_key_modal = false;
                                    }
                                }
                                if ui.button("Annuler").clicked() {
                                    self.show_api_key_modal = false;
                                    self.api_key_error = None;
                                }
                            });
                                }
                                SettingsTab::Theme => {
                            // --- Onglet Thèmes & Accessibilité ---
                            ui.label(
                                egui::RichText::new("Profils de contraste et accessibilité visuelle :")
                                    .size(11.5)
                                    .color(theme.text_secondary),
                            );
                            ui.add_space(6.0);

                            let mut current_theme = self.theme;
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Thème :").size(12.0).strong().color(egui::Color32::WHITE));
                                egui::ComboBox::from_id_salt("settings_theme_combobox")
                                    .selected_text(current_theme.display_name())
                                    .width(260.0)
                                    .show_ui(ui, |ui| {
                                        for &t in AppTheme::ALL {
                                            let t_colors = t.colors();
                                            ui.horizontal(|ui| {
                                                if ui.selectable_value(&mut current_theme, t, t.display_name()).clicked() {
                                                    self.set_theme(t);
                                                }
                                                let (rect, _) = ui.allocate_exact_size(egui::vec2(44.0, 12.0), egui::Sense::hover());
                                                let p = ui.painter();
                                                p.rect_filled(egui::Rect::from_min_size(rect.min, egui::vec2(10.0, 12.0)), egui::CornerRadius::same(2), t_colors.control_bg);
                                                p.rect_filled(egui::Rect::from_min_size(egui::pos2(rect.min.x + 11.0, rect.min.y), egui::vec2(10.0, 12.0)), egui::CornerRadius::same(2), t_colors.border);
                                                p.rect_filled(egui::Rect::from_min_size(egui::pos2(rect.min.x + 22.0, rect.min.y), egui::vec2(10.0, 12.0)), egui::CornerRadius::same(2), t_colors.accent);
                                                p.rect_filled(egui::Rect::from_min_size(egui::pos2(rect.min.x + 33.0, rect.min.y), egui::vec2(10.0, 12.0)), egui::CornerRadius::same(2), t_colors.text_user);
                                            });
                                        }
                                    });
                            });

                            if current_theme != self.theme {
                                self.set_theme(current_theme);
                            }

                            ui.add_space(8.0);
                            let cur_colors = self.theme.colors();
                            egui::Frame::new()
                                .fill(cur_colors.control_bg)
                                .stroke(egui::Stroke::new(1.0, cur_colors.border))
                                .corner_radius(egui::CornerRadius::same(8))
                                .inner_margin(egui::Margin::symmetric(10, 8))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(egui::RichText::new(self.theme.display_name()).strong().size(12.0).color(cur_colors.accent));
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            let (rect, _) = ui.allocate_exact_size(egui::vec2(58.0, 14.0), egui::Sense::hover());
                                            let p = ui.painter();
                                            p.rect_filled(egui::Rect::from_min_size(rect.min, egui::vec2(14.0, 14.0)), egui::CornerRadius::same(2), cur_colors.control_bg);
                                            p.rect_filled(egui::Rect::from_min_size(egui::pos2(rect.min.x + 15.0, rect.min.y), egui::vec2(14.0, 14.0)), egui::CornerRadius::same(2), cur_colors.border);
                                            p.rect_filled(egui::Rect::from_min_size(egui::pos2(rect.min.x + 30.0, rect.min.y), egui::vec2(14.0, 14.0)), egui::CornerRadius::same(2), cur_colors.accent);
                                            p.rect_filled(egui::Rect::from_min_size(egui::pos2(rect.min.x + 45.0, rect.min.y), egui::vec2(13.0, 14.0)), egui::CornerRadius::same(2), cur_colors.text_user);
                                        });
                                    });
                                    ui.add_space(3.0);
                                    ui.label(
                                        egui::RichText::new(self.theme.description())
                                            .size(10.5)
                                            .color(cur_colors.text_secondary)
                                            .italics(),
                                    );
                                });

                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                if ui.button("Fermer").clicked() {
                                    self.show_api_key_modal = false;
                                }
                                ui.label(
                                    egui::RichText::new("Astuce vocale : dites « thème suivant » ou « contraste élevé »")
                                        .size(10.0)
                                        .color(theme.text_secondary),
                                );
                            });
                                }
                            }
                        });
                }

                // Affichage dynamique des fenêtres volantes (stats, tables, contenus, formulaires de requête)
                let mut submitted_card_prompts = Vec::new();
                let prev_card_count = self.floating_cards.len();
                self.floating_cards.retain_mut(|card| {
                    if let Some(prompt) = card.show(&ctx) {
                        submitted_card_prompts.push(prompt);
                    }
                    card.open
                });
                if self.floating_cards.len() != prev_card_count {
                    update_open_floating_cards(&self.floating_cards);
                    ctx.request_repaint();
                }
                for prompt in submitted_card_prompts {
                    self.stop_speaking();
                    if self.is_recording {
                        self.stop_recording();
                    }
                    self.status = AgentStatus::Thinking;
                    self.continuous_mode = true;
                    self.live_transcript.clear();
                    self.chat_history.push(ChatEntry {
                        role: ChatRole::User,
                        text: prompt.clone(),
                        timestamp: current_time_str(),
                        quick_suggestions: Vec::new(),
                        screen_size_kb: None,
                    });
                    let _ = self.command_sender.send(AgentCommand::Prompt(prompt));
                }
            });
    }
}

fn resolve_llm_config() -> LlmConfig {
    // Ordre de priorité en cascade
    let providers_cascade = [
        (LlmProvider::DeepSeek, &["DEEPSEEK_API_KEY"][..]),
        (LlmProvider::OpenAI, &["OPENAI_API_KEY"][..]),
        (LlmProvider::Anthropic, &["ANTHROPIC_API_KEY"][..]),
        (LlmProvider::Mistral, &["MISTRAL_API_KEY"][..]),
        (LlmProvider::Glm, &["GLM_API_KEY", "ZHIPU_API_KEY"][..]),
        (LlmProvider::Groq, &["GROQ_API_KEY"][..]),
    ];

    // Fournisseur forcé via LLM_PROVIDER
    let forced_provider = std::env::var("LLM_PROVIDER")
        .ok()
        .or_else(|| read_env_var_from_file(".env", "LLM_PROVIDER"))
        .and_then(|s| LlmProvider::from_name(&s));

    let mut chosen_provider = LlmProvider::DeepSeek;
    let mut found_key = String::new();

    if let Some(forced) = forced_provider {
        for (prov, keys) in &providers_cascade {
            if *prov == forced {
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
                break;
            }
        }
    }

    if found_key.is_empty() {
        for (prov, keys) in &providers_cascade {
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

    let endpoint_override = std::env::var("LLM_BASE_URL")
        .or_else(|_| std::env::var("LLM_ENDPOINT"))
        .ok()
        .or_else(|| read_env_var_from_file(".env", "LLM_BASE_URL"))
        .or_else(|| read_env_var_from_file(".env", "LLM_ENDPOINT"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let model = model_override.unwrap_or_else(|| chosen_provider.default_model().to_string());
    let endpoint = endpoint_override.unwrap_or_else(|| chosen_provider.default_endpoint().to_string());

    LlmConfig {
        provider: chosen_provider,
        api_key: found_key,
        model,
        endpoint,
    }
}

fn resolve_initial_voice() -> VoiceGender {
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

fn parse_agent_response(raw: &str) -> AgentResponsePayload {
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

    // Extraction fiabilisée des blocs Markdown (```json, ``` json, ```)
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

    // 1. Désérialisation directe
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

    // 2. Extraction du bloc JSON entre la première accolade '{' et la dernière '}'
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

            // 3. Repli dynamique : extraction ciblée de la narration, passes et actions via serde_json::Value
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

struct BusyGuard;
impl Drop for BusyGuard {
    fn drop(&mut self) {
        AGENT_BUSY.store(false, Ordering::SeqCst);
    }
}

/// Envoi unifié d'une requête de complétion selon le fournisseur configuré
async fn send_llm_chat_completion(
    client: &reqwest::Client,
    config: &LlmConfig,
    system_prompt: &str,
    messages_history: &[ChatMessage],
    temperature: f32,
    max_tokens: u32,
) -> Result<reqwest::Response, reqwest::Error> {
    if config.provider == LlmProvider::Anthropic {
        let mut anthropic_messages: Vec<ChatMessage> = Vec::new();
        for msg in messages_history {
            if msg.role == "system" {
                continue;
            }
            let role = if msg.role == "assistant" { "assistant" } else { "user" };
            if let Some(last) = anthropic_messages.last_mut() {
                if last.role == role {
                    last.content.push_str("\n\n");
                    last.content.push_str(&msg.content);
                    continue;
                }
            }
            anthropic_messages.push(ChatMessage {
                role: role.to_string(),
                content: msg.content.clone(),
            });
        }
        if anthropic_messages.first().map_or(false, |m| m.role != "user") {
            anthropic_messages.insert(0, ChatMessage {
                role: "user".to_string(),
                content: "Bonjour".to_string(),
            });
        }

        let req_payload = AnthropicMessagesRequest {
            model: config.model.clone(),
            system: if system_prompt.trim().is_empty() { None } else { Some(system_prompt.to_string()) },
            messages: anthropic_messages,
            max_tokens,
            temperature,
        };

        client
            .post(&config.endpoint)
            .header("x-api-key", &config.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&req_payload)
            .send()
            .await
    } else {
        let mut messages = Vec::new();
        if !system_prompt.trim().is_empty() {
            messages.push(ChatMessage {
                role: "system".to_string(),
                content: system_prompt.to_string(),
            });
        }
        messages.extend_from_slice(messages_history);

        let response_format = match config.provider {
            LlmProvider::DeepSeek | LlmProvider::OpenAI | LlmProvider::Mistral | LlmProvider::Groq => {
                Some(ChatResponseFormat {
                    r#type: "json_object".to_string(),
                })
            }
            _ => None,
        };

        let req_payload = DeepSeekChatRequest {
            model: config.model.clone(),
            messages,
            temperature,
            max_tokens,
            top_p: 1.0,
            stream: false,
            response_format,
        };

        client
            .post(&config.endpoint)
            .bearer_auth(&config.api_key)
            .json(&req_payload)
            .send()
            .await
    }
}

async fn parse_llm_response_text(res: reqwest::Response, provider: LlmProvider) -> Result<String, String> {
    if provider == LlmProvider::Anthropic {
        let body = res.json::<AnthropicMessagesResponse>().await
            .map_err(|e| format!("Erreur décodage JSON Anthropic: {e}"))?;
        let content = body.content
            .into_iter()
            .filter(|b| b.block_type == "text")
            .filter_map(|b| b.text)
            .collect::<Vec<_>>()
            .join("\n");
        if content.is_empty() {
            Err("Réponse Anthropic vide".to_string())
        } else {
            Ok(content)
        }
    } else {
        let body = res.json::<DeepSeekChatResponse>().await
            .map_err(|e| format!("Erreur décodage JSON OpenAI/DeepSeek: {e}"))?;
        if let Some(choice) = body.choices.first() {
            Ok(choice.message.content.clone())
        } else {
            Err("Aucun choix retourné dans la réponse".to_string())
        }
    }
}

async fn compact_history_if_needed(
    llm_config: &LlmConfig,
    history: &mut Vec<ChatMessage>,
    client: &reqwest::Client,
    last_call_time: &mut Option<Instant>,
) {
    const COMPACTION_CHAR_THRESHOLD: usize = 6000;
    const COMPACTION_MSG_THRESHOLD: usize = 8;
    const KEEP_RECENT_COUNT: usize = 3;

    let total_chars: usize = history.iter().map(|m| m.content.len()).sum();
    if (history.len() < COMPACTION_MSG_THRESHOLD && total_chars < COMPACTION_CHAR_THRESHOLD)
        || history.len() <= KEEP_RECENT_COUNT
    {
        return;
    }

    let split_idx = history.len().saturating_sub(KEEP_RECENT_COUNT);
    let to_summarize = &history[..split_idx];
    let recent = history[split_idx..].to_vec();

    let mut conversation_text = String::new();
    for msg in to_summarize {
        conversation_text.push_str(&format!("{}: {}\n", msg.role, msg.content));
    }

    let safe_conversation_text = truncate_with_notice(&conversation_text, 8000);

    // Respecter un délai de debounce avant l'appel de condensation
    let min_debounce = Duration::from_millis(3000);
    if let Some(prev) = *last_call_time {
        let elapsed = prev.elapsed();
        if elapsed < min_debounce {
            tokio::time::sleep(min_debounce - elapsed).await;
        }
    }

    let (_, system_lang_name) = get_system_locale_language();
    let sys_prompt = format!("Tu es un synthétiseur de mémoire conversationnelle. Résume fidèlement les points clés, décisions et actions passées en 250 mots maximum dans la langue du système de l'utilisateur ({system_lang_name}) sous forme de points synthétiques.");
    let messages_to_condense = vec![
        ChatMessage {
            role: "user".to_string(),
            content: format!("Voici les échanges passés à condenser :\n\n{}", safe_conversation_text),
        },
    ];

    let response = send_llm_chat_completion(
        client,
        llm_config,
        &sys_prompt,
        &messages_to_condense,
        0.2,
        400,
    ).await;

    *last_call_time = Some(Instant::now());

    if let Ok(res) = response {
        if res.status().is_success() {
            if let Ok(summary_text) = parse_llm_response_text(res, llm_config.provider).await {
                let summary = summary_text.trim();
                if !summary.is_empty() {
                    println!("[Mémoire] Compaction contextuelle réussie ({} messages condensés).", to_summarize.len());
                    let mut new_history = Vec::with_capacity(recent.len() + 2);
                    new_history.push(ChatMessage {
                        role: "user".to_string(),
                        content: format!("[Note contextuelle - résumé des échanges antérieurs] :\n{}", summary),
                    });
                    new_history.push(ChatMessage {
                        role: "assistant".to_string(),
                        content: "Contexte précédent bien assimilé.".to_string(),
                    });
                    new_history.extend(recent);
                    *history = new_history;
                    return;
                }
            }
        }
    }

    // Repli de sécurité en cas d'erreur de condensation
    println!("[Mémoire] Repli : éviction fifo sans résumé.");
    *history = recent;
}

fn build_standard_system_instructions(lang_name: &str) -> String {
    format!(r###"Tu es l'agent d'exploration Libertide multiplateforme (Windows, macOS, Linux).
Réponds STRICTEMENT en JSON sans texte hors JSON. "narration" concise rédigée impérativement dans la langue du système de l'utilisateur : {lang_name} (max 2 phrases).

Règles principales :
1. Filtrage requête : Bruit/inintelligible -> "invalid_request": true. Salutations/chit-chat -> "invalid_request": false, "actions": [], narration courtoise.
2. Budget de raisonnement autonome et passes profondes ("next_passes": 1..15 ou action "set_reasoning_passes") :
    - ÉVALUATION ET PILOTAGE DU BUDGET PAR LE MODÈLE : Tu gères toi-même le nombre de passes d'exécution nécessaires via le champ racine "next_passes" ou l'action "set_reasoning_passes" :
      * Basse importance / Réponse directe / Simple clic / Tâche ponctuelle rapide ("vite", "juste un coup d'œil", information simple) : 1 à 3 passes.
      * Importance normale / Tâche standard multi-étapes (navigation web, formulaire, lecture, interaction applicative standard) : 4 à 7 passes.
      * Haute importance / Recherche approfondie / Mission de contact / Comparatif de produits ou services / Benchmarks / Enquêtes : Tu DOIS impérativement t'allouer un budget de passes profondes ("next_passes": 8 à 12) dès la première passe et ne JAMAIS clore prématurément.
      * Importance critique / Urgence vitale explicite / Grand départ précipité : 13 à 15 passes.
    - MODULATION DYNAMIQUE CONTINUE : À chaque passe, réévalue et ajuste "next_passes" selon l'état d'avancement. Si la tâche exige plus d'étapes d'exploration, augmente le budget via "next_passes" ou "set_reasoning_passes".
 3. PROTOCOLE URGENCE VITALE & SANTÉ :
   - CAS 1 - DEMANDE EXPLICITE D'ALERTE / SECOURS (ex: "alerte Robin je fais un malaise", "appelle le 15", "aide médicale d'urgence") :
     * IMMÉDIATEMENT "next_passes": 15.
     * SANS ATTENDRE : exécute immédiatement l'envoi d'alerte ("actions": [...]).
     * Narration : 1 phrase courte confirmant l'envoi + rappelle secours (15 SAMU, 18 Pompiers, 112).
     * Silence parasite : appelle "mute_audio" (isole bruits tiers, garde voix Libertide audible).
     * PIVOT OBLIGATOIRE : Tant que l'alerte n'est pas envoyée et qu'il reste des passes, INTERDICTION de renvoyer "actions": [].
   - CAS 2 - DIDASCALIES (*gémissements*, *soupir*, *aïe*, [pleurs], etc.), JEU DE RÔLE, CITATION OU SITUATION AMBIGUË SANS DEMANDE FORMELLE :
     * INTERDICTION STRICTE DE PANIQUER OU D'ENVOYER DES MESSAGES À DES TIERS : conserve impérativement "actions": [].
     * Reste calme et attentif. Vérifie la situation dans la narration (ex: "J'ai perçu des signes de malaise (*gémissements*). Avez-vous besoin d'une assistance ou d'alerter un proche ?").
     * Fournis OBLIGATOIREMENT une demande de confirmation dans "quick_suggestions" : ["Oui, alerter mes proches / secours", "Non, tout va bien / fausse alerte"].
   - Départ précipité : autonomie pour sauver fichiers et fermer session / verrouiller écran.
 3bis. PROTOCOLE D'ANALYSE APPROFONDIE STRATÉGIQUE (CONTACTS, PRODUITS ET SERVICES) :
   - Dès que la demande implique de trouver ou contacter une personne, trouver des coordonnées, ou rechercher / comparer des produits et services :
     * GESTION DU BUDGET : Définis immédiatement "next_passes": 8 à 12 (ou appelle l'action "set_reasoning_passes": 10). INTERDICTION de clore la mission en 1 seule passe ou de poser une question passive avec "actions": [].
     * VOLET CONTACTS & COMMUNICATION ("contacte...", "écris à...", "trouve le contact", "appelle...") :
       - Niveau 1 (Local) : Vérifie et sélectionne les messageries ou logiciels mail ouverts (Teams, Outlook, WhatsApp, Discord, Slack) et cherche le contact.
       - Niveau 2 (Web & Annuaires pros) : Si absent en local ou application fermée, ouvre le navigateur ("navigate_to_url") pour chercher sur le web, LinkedIn, annuaires d'entreprises ou pages professionnelles.
       - Niveau 3 (Projection fiche contact) : Dès identification des coordonnées (email, téléphone, poste, profil), projette OBLIGATOIREMENT une fiche volante de synthèse via "show_floating_card" (card_type: "stats", id: "fiche_contact", title: "Fiche contact : <Nom>", data: {{"Nom": "...", "Email": "...", "Téléphone": "...", "Poste / Société": "...", "Source": "..."}}), puis prépare le canal de communication (mailto:, message, appel).
     * VOLET PRODUITS & SERVICES ("compare...", "meilleur...", "prix de...", "trouve une offre/produit") :
       - Niveau 1 (Exploration web) : Ouvre le navigateur ("navigate_to_url") sur les moteurs, comparateurs ou sites marchands de référence.
       - Niveau 2 (Extraction métrique) : Relève systématiquement prix, spécifications, disponibilité et notes/avis.
       - Niveau 3 (Tableau comparatif volant) : Projette OBLIGATOIREMENT un tableau comparatif structuré dans une fenêtre volante via "show_floating_card" (card_type: "table", title: "Comparatif offres", data: {{"headers": ["Modèle / Offre", "Prix", "Caractéristiques", "Avis"], "rows": [["...", "...", "...", "..."]]}}).
     * POLITIQUE DE PIVOT EN CAS D'ÉCHEC (TOUS VOLETS) :
       - L'absence de résultat ou le blocage sur un premier canal (ex: contact absent dans Teams, page web non accessible) NE MET PAS FIN À LA MISSION. C'est un ordre de PIVOT IMMÉDIAT vers le canal suivant (local -> web -> annuaires) sans renvoyer "actions": [].
       - Tant que tous les canaux n'ont pas été explorés et que les résultats ne sont pas projetés dans une fenêtre volante ("show_floating_card"), INTERDICTION de renvoyer "actions": [].
 3ter. PROTOCOLE RÉSUMÉ DE PAGE ET SYNTHÈSE DE CONTENU ("résume cette page", "synthèse", "de quoi ça parle", "fais un résumé") :
   - Dès que l'utilisateur souhaite résumer ou analyser une page web, un document ou un ensemble d'informations à l'écran :
     * Analyse minutieusement le texte, titres et points clés de la page.
     * Projette SYSTÉMATIQUEMENT une fenêtre volante au format Markdown via l'action "show_floating_card" :
       card_type: "markdown", id: "resume_page", title: "Résumé : <Titre de la page/sujet>", data: {{"text": "# SLICE Docus — Synthèse de la chaîne\n\n## Identité\n- **Chaîne** : @SliceDocus — **56,4 k abonnés**, **875 vidéos**.\n\n## Vidéos récentes / populaires\n- ▶ La guerre des satellites qui redessine notre monde (2,7 k vues, il y a 20 h)\n- ▶ Galea: les trésors engloutis de Venise (5 k vues, il y a 3 j)\n- ▶ Bio-invasion | Fourmis de feu (95 k vues, il y a 7 j)\n\n## À retenir\n- Formats longs de 34 à 55 min...\n\n> Cliquez sur une vidéo ci-dessus pour la lancer."}}.
     * LIENS D'ACTION & VIDÉOS CLIQUABLES : Dans le texte Markdown et la narration, insère systématiquement des liens cliquables via la macro `[link="action:consigne_ou_url"]Icône Libellé[/link]` (ou en Markdown `Icône Libellé`). Un clic sur le lien ouvre l'URL ou envoie immédiatement la consigne à l'agent.
     * Dans ta "narration" vocale : sois très concis (1 à 2 phrases) pour confirmer oralement que la synthèse est projetée dans la fenêtre volante.
     * Fournis des suggestions rapides utiles dans "quick_suggestions" (ex: ["Lancer la vidéo 1", "Lancer la vidéo 2", "Fermer le résumé"]).
4. Interface & Ciblage :
   - Hôte navigateur : préfixe "[Navigateur]" (ex: "[Navigateur] Barre d'adresse").
   - Contenu web : libellé direct sans préfixe issu de "[Page web - ...]".
   - ACCEPTATION AUTOMATIQUE DES COOKIES : Dès qu'une bannière de cookies ou consentement RGPD est présent, clique SYSTÉMATIQUEMENT et immédiatement sur le bouton pour tout accepter ("Tout accepter", "Accepter tout", "Autoriser", "J'accepte", "Continuer sans refuser") sans demander de confirmation.
   - BANNIÈRES DE NOTIFICATION WEB, PUSH & NEWSLETTERS ("Activez les notifications", "Ne manquez rien", etc.) : Ce ne sont PAS des notifications système Windows (toasts). Ce sont des popups ou bannières intrusives sur la page web. Tu DOIS SYSTÉMATIQUEMENT les traiter immédiatement pour débloquer la page : clique en priorité sur la croix de fermeture ("Fermer (croix ✕)", "croix", "Fermer", "Plus tard", "Refuser", "Non merci"), ou sur "Activer" si demandé ou si aucune option de fermeture n'est présente. Ne laisse JAMAIS une bannière de notification web bloquer la page sans la valider ou la fermer !
   - LECTURE DES NOTIFICATIONS SYSTÈME (TOASTS) : Dès qu'une notification Windows ou un toast est présent, lis impérativement son titre et son contenu dans ta "narration" pour informer l'utilisateur, mais LAISSE-LA TRANQUILLE (ne tente pas de la fermer, ne clique pas dessus et laisse le système la gérer).
   - ALERTES & MESSAGES D'ERREUR (FERMETURE SYSTÉMATIQUE APRÈS LECTURE) : Dès qu'une boîte de dialogue d'alerte, un popup d'avertissement ou un message d'erreur est à l'écran, lis-le clairement ou résume-le dans ta "narration", puis FERME-LA SYSTÉMATIQUEMENT en cliquant sur son bouton de fermeture/validation ("OK", "Fermer", "Annuler", "Continuer", croix) pour débloquer l'écran et poursuivre ton travail. Ne JAMAIS envoyer "escape".
   - PRIORITÉ ABSOLUE AUX CHANGEMENTS WEB : Si des informations, boutons ou données sont présent sur la page web (signalés sous 'INFORMATIONS WEB MODIFIÉES / NOUVELLES'), ils priment impérativement sur tous les éléments statiques et les autres fenêtres. Base en priorité absolue tes actions ou réponses sur ces changements.
   - OBLIGATION DE CIBLAGE EXPLICITE DU CHAMP : Pour toute saisie, sélection ou clic ("write_text", "replace_field_text", "clear_text", "focus_element", "click_element", "click_button"), tu DOIS IMPÉRATIVEMENT renseigner le champ interactable précis visé ("target", "target_name" ou "button_name") en utilisant sa macro ou son identifiant exact observé à l'écran (ex: input, select, textarea, combobox, bouton, lien, video, audio).
   - LECTEURS VIDÉO & AUDIO : Les balises vidéo et audio sont des éléments interactables (signalés par [@video:...], [@audio:...], ou [video]/[audio]). Pour lancer la lecture, mettre en pause ou interagir avec un média, utilise "click_element" ou "click_button" en ciblant la vidéo, l'audio ou ses contrôles de lecture ("play", "lecture", etc.).
   - SÉCURITÉ DU CIBLAGE : "target": null et les cibles génériques ou fictives sont STRICTEMENT INTERDITES. Le moteur d'exécution valide déterministement chaque élément contre l'arbre réel de l'écran ; toute cible non vérifiable ou absente sera rejetée par sécurité.
   - Raccourcis clavier & touches : utilise OBLIGATOIREMENT "accessibility_shortcut" avec le nom ou la combinaison de touches exacte (ex: "action_center", "open_settings", "ctrl+shift+b", "ctrl+t", "enter", "close_tab", "copy", "undo", "space"). Ne JAMAIS envoyer "run_command" pour simuler des touches ou des raccourcis système !
   - Lecture code/fichier : "read_editor_content" (focalise et extrait Ctrl+A/Ctrl+C).
   - Onglet/page : "accessibility_shortcut": "close_tab" (jamais "close_app" pour un onglet).
   - DISTINCTION ABSOLUE ENTRE "open_search" ET "open_search_browser" (2 COMMANDES DISTINCTES) :
     * "open_search" (ou accessibility_shortcut: "open_search") : Recherche SYSTÈME / OS locale (Win+S). Sert STRICTEMENT ET UNIQUEMENT à rechercher un fichier local, une application installée ou un paramètre Windows sur l'ordinateur. INTERDICTION FORMELLE d'utiliser "open_search" pour chercher sur internet ou sur le web !
     * "open_search_browser" (ou "focus_address_bar" / accessibility_shortcut: "open_search_browser") : Recherche WEB / NAVIGATEUR. Sert SYSTÉMATIQUEMENT pour chercher sur le web, aller sur internet, ou saisir une requête dans l'omnibox/barre d'adresse du navigateur. Si l'utilisateur demande de chercher sur le web, Google, YouTube, la météo, un produit, etc., utilise TOUJOURS "open_search_browser" (ou "focus_address_bar" / "navigate_to_url").
4bis. RÈGLES DE LIENS INTERACTIFS & MACRO [link="cible"]label[/link] (CHAT & FENÊTRES VOLANTES) :
   - RÈGLE D'OR : Tout élément multimédia, ressource, fichier, page web, action ou suggestion mentionné dans "narration" ou dans une fenêtre volante ("show_floating_card") DOIT IMPÉRATIVEMENT être formaté sous forme de lien cliquable, prioritairement avec la macro robuste `[link="cible"]Icône Libellé[/link]` (ou en Markdown `Icône Libellé`).
   - LIENS DANS LES TABLEAUX MARKDOWN : Tu PEUX et DOIS insérer des liens `[link="..."]...[/link]` à l'intérieur des cellules de tableaux Markdown ou de fenêtres volantes (ex: `| Produit | Action | Prix |\n| --- | --- | --- |\n| Drone Pro | [link="action:Commander Drone Pro"]⚡ Commander[/link] | 299€ |`).
   - AVANTAGE MAJEUR DE LA MACRO `[link="..."]...[/link]` : Elle évite tout conflit de parenthèses imbriquées dans les requêtes et les URLs complexes.
   - RÉACTIVITÉ DANS LE CHAT LIBERTIDE : Un clic sur un lien dans le chat ou dans une carte volante insère et envoie automatiquement la consigne dans le chat et déclenche l'exécution par l'agent. La synthèse vocale (TTS) prononce uniquement le libellé sans lire la cible technique ni les balises.
   - TYPOLOGIE DES MACROS OBLIGATOIRES :
     * Vidéos (YouTube, streaming) : `[link="action:Lancer la vidéo : Titre"]▶ Titre[/link]` ou `[link="https://..."]▶ Titre[/link]` (INTERDICTION d'énumérer des vidéos sans lien).
     * Audio & Musique (Spotify, radios, podcasts) : `[link="action:Lancer l'audio : Titre"]🎵 Titre[/link]`.
     * Images & Visuels : `[link="action:Ouvrir l'image : chemin_ou_titre"]🖼️ Titre[/link]`.
     * Documents & PDF (tableurs, rapports, logs) : `[link="action:Ouvrir le fichier : nom.pdf"]📄 nom.pdf[/link]`.
     * Liens Web & Articles : `[link="https://..."]🌐 Nom du site[/link]`.
     * Actions & Clics système : `[link="action:Valider le panier"]⚡ Valider le panier[/link]`.
     * Suggestions de choix : `[link="action:Consigne suggérée"]💡 Option recommandée[/link]`.
5. Audio, Volume & Paramètres système (RÈGLE ABSOLUE ANTI-TERMINAL) :
   - INTERDICTION STRICTE d'utiliser "run_command" avec "explorer.exe", "start ms-settings:sound", "cmd" ou toute commande console pour le son ou les réglages. L'invite de commande ne doit JAMAIS s'ouvrir pour gérer le son !
   - Actions dédiées pour l'audio et les paramètres :
     * Réglage natif du volume : action "set_volume" ("up"|"down"|"mute"|"unmute"|"min") ou "mute_audio".
     * Panneau des réglages audio et choix de sortie (casque/haut-parleur) : action "accessibility_shortcut": "action_center" (ou "sound_settings", simule Win+A sans aucune invite de commande) ou "open_settings" (Win+I).
     * Raccourcis multimédia universels : "accessibility_shortcut": "volume_up" | "volume_down" | "mute_volume".
     * Lecteurs vidéo & musique (YouTube, Deezer, Spotify) : utilise directement "click_button" avec le libellé observé (ex: "Désactiver le son (m)", "Pause (k)") ou "accessibility_shortcut": "m" / "k" / "space". Ne quitte jamais la page pour régler un média web !
   - "focus_address_bar" pour cibler la barre d'adresse du navigateur.
6. Navigation & Web :
   - "navigate_to_url" ouvre par défaut un nouvel onglet ("target": "new_tab").
   - Achat : panier -> valider panier -> AVANT paiement stopper avec "actions": [] et demander confirmation.
   - "scroll" ("down"|"up"|"bottom"|"top", amount: 1..2) pour explorer.
   - RETOUR EN ARRIÈRE & UNDO NAVIGATION : "navigate_back" (ou "accessibility_shortcut": "browser_back", Alt+Gauche) pour revenir immédiatement à la page ou aux résultats précédents si une navigation s'est trompée de page, mène à une impasse ou à un tag erroné. "navigate_forward" pour avancer.
7. Autres actions :
   - Dictée : "activate_dictation", "deactivate_dictation", "toggle_dictation", "dictate".
   - Démarrage : "enable_autostart", "disable_autostart", "check_autostart", "set_autostart".
   - Sélection : "select_text_between" (from, to).
   - Immersion : "activate_immersion" (Win+Ctrl+D), "deactivate_immersion".
   - Temporisation & Différé : "schedule_prompt" (delay_seconds: 1..3600, prompt: "...") ou "schedule_prompts" (tasks: [{{"delay_seconds": 10, "prompt": "..."}}]) pour différer des consignes futures (téléchargement en cours, rappel, temporisation). "cancel_scheduled_tasks" pour annuler immédiatement toutes les commandes différées / timers en attente.
   - Fichiers & Documents : "open_file" (path: "chemin/fichier.ext") pour ouvrir directement toute image, vidéo, PDF, tableur ou document dans son application système par défaut (selon extension / MIME type).
   - Fenêtres volantes d'analyse & formulaires de requête interactifs : "show_floating_card" (id: "...", title: "...", card_type: "form"|"table"|"stats"|"markdown", data: {{...}}) pour projeter une fenêtre volante avec un formulaire interactif de filtres/requête (inputs, jauges/sliders numériques, selectbox, radio, checkbox pour recherche personnalisée de vols, hôtels, shopping), un tableau comparatif, des données chiffrées ou une fiche technique. Pour "markdown", insère des liens cliquables `[link="action:..."]⚡ Libellé[/link]` pour permettre à l'utilisateur de piloter les choix ou actions directement depuis la fenêtre volante.
   - Fermeture carte : "close_floating_card" (id: "..."). Formate tous les choix, suggestions et références sous forme de liens cliquables cliquables au clic dans le chat.
 8. Fin de passes : Dès que "actions" est vide (tâche finie ou choix requis), fournis OBLIGATOIREMENT 2 à 4 suggestions concrètes dans "quick_suggestions".

Format JSON obligatoire :
{{
  "narration": "Explication vocale concise ou vide.",
  "next_passes": 5,
  "actions": [
    {{"action": "open_app", "name": "nom_executable"}},
    {{"action": "open_search"}},
    {{"action": "open_search_browser"}},
    {{"action": "navigate_back"}},
    {{"action": "navigate_forward"}},
    {{"action": "undo"}},
    {{"action": "redo"}},
    {{"action": "close_app", "name": "nom_ou_titre"}},
    {{"action": "click_element", "window": null, "target_name": "nom"}},
    {{"action": "click_button", "window": null, "button_name": "nom"}},
    {{"action": "focus_element", "window": null, "target_name": "nom"}},
    {{"action": "scroll", "direction": "down"|"up"|"bottom"|"top", "amount": 1, "window": null}},
    {{"action": "focus_window", "title": null, "pid": null}},
    {{"action": "focus_address_bar", "window": null}},
    {{"action": "read_editor_content", "window": null, "target": null}},
    {{"action": "clear_text", "window": null}},
    {{"action": "write_text", "text": "...", "target": null, "press_enter": false}},
    {{"action": "dictate", "text": "...", "window": null, "target": null, "press_enter": false}},
    {{"action": "activate_dictation", "window": null, "target": null}},
    {{"action": "deactivate_dictation"}},
    {{"action": "toggle_dictation"}},
    {{"action": "select_text_between", "from": "...", "to": "...", "window": null, "target": null}},
    {{"action": "replace_field_text", "text": "...", "target": null}},
    {{"action": "run_command", "command": "..."}},
    {{"action": "kill_process", "pid": null, "name": null}},
    {{"action": "navigate_to_url", "url": "...", "target": "new_tab"|"current_tab"|"new_window"}},
    {{"action": "accessibility_shortcut", "shortcut": "browser_back"|"browser_forward"|"open_search"|"open_search_browser"|"action_center"|"open_settings"|"sound_settings"|"volume_up"|"volume_down"|"mute_volume"|"enter"|"escape"|"space"|"close_tab"|"refresh_page"|"ctrl+shift+b"|"<combinaison_touches>"}},
    {{"action": "deactivate_immersion"}},
    {{"action": "set_reasoning_passes", "passes": 5, "reason": null}},
    {{"action": "schedule_prompt", "delay_seconds": 10, "prompt": "..."}},
    {{"action": "schedule_prompts", "tasks": [{{"delay_seconds": 10, "prompt": "..."}}]}},
    {{"action": "cancel_scheduled_tasks"}},
    {{"action": "mute_audio"}},
    {{"action": "set_volume", "command": "mute"|"unmute"|"down"|"up"|"min"}},
    {{"action": "open_file", "path": "chemin/vers/image_ou_doc.png"}},
    {{"action": "show_floating_card", "id": "stats_1", "title": "Tableau comparatif", "card_type": "markdown", "data": {{"text": "| Modèle | Lien / Action | Prix |\n| --- | --- | --- |\n| Produit A | [link=\"action:Commander Produit A\"]⚡ Commander[/link] | 49€ |\n| Produit B | [link=\"action:Commander Produit B\"]⚡ Commander[/link] | 59€ |"}}}},
    {{"action": "show_floating_card", "id": "vol_search", "title": "Filtres Billet d'Avion", "card_type": "form", "data": {{"submit_label": "Rechercher les vols", "prompt_prefix": "Recherche de vol avec les critères suivants :", "fields": [{{"id": "depart", "label": "Ville de départ", "type": "text", "value": "Paris"}}, {{"id": "destination", "label": "Destination", "type": "text", "value": "Tokyo"}}, {{"id": "prix_max", "label": "Budget maximum", "type": "number_slider", "min": 200, "max": 3000, "step": 50, "value": 900, "unit": "€"}}, {{"id": "classe", "label": "Classe", "type": "select", "options": ["Économique", "Premium", "Affaires"], "selected": "Économique"}}, {{"id": "direct", "label": "Vols directs seulement", "type": "checkbox", "checked": true}}, {{"id": "bagage", "label": "Bagages", "type": "radio", "options": ["Cabine seul", "1 bagage en soute", "2 bagages en soute"], "selected": "1 bagage en soute"}}]}}}},
    {{"action": "show_floating_card", "id": "resume_page", "title": "Résumé de la page", "card_type": "markdown", "data": {{"text": "# SLICE Docus — Synthèse\n## Vidéos populaires\n- [link=\"action:Lancer la vidéo : La guerre des satellites\"]▶ La guerre des satellites (2,7 k vues)[/link]\n- [link=\"action:Lancer la vidéo : Bio-invasion\"]▶ Bio-invasion (95 k vues)[/link]\n\n> Cliquez sur une vidéo pour la lancer."}}}},
     {{"action": "close_floating_card", "id": "stats_1"}}
  ],
  "quick_suggestions": ["Option 1", "Option 2"]
}}"###)
}

const DICTATION_SYSTEM_INSTRUCTIONS: &str = r#"Tu es l'assistant de dictée vocale et de saisie textuelle pour Windows.
Tu ne traites AUCUNE tâche d'exploration, navigation web, clic d'interface ou manipulation de fenêtres.
Tu analyses STRICTEMENT la voix pour retranscrire du texte, déplacer le curseur ou stopper la dictée.
Réponds STRICTEMENT en JSON sans aucun texte autour.

Règles de décision :
1. Arrêt de la dictée ("deactivate_dictation") :
   - Si l'utilisateur demande d'arrêter, quitter, couper ou stopper la dictée (ex: "arrête la dictée", "stop dictée", "quitter la dictée", "pause dictée", "c'est fini", "termine la dictée") :
     N'écris JAMAIS cette consigne dans le document ! Émets l'action "deactivate_dictation" avec "narration": "Dictée désactivée.".
2. Contrôle et déplacement du curseur ("cursor_move") :
   - Si l'utilisateur donne un ordre de mise en page ou de déplacement de curseur ("à la ligne", "nouveau paragraphe", "curseur à gauche", "efface le mot", "début de ligne"), utilise l'action "cursor_move" avec le libellé de commande adéquat. Maintiens "narration": "".
3. Retranscription de texte dicté ("dictate") :
   - Pour tout autre propos dicté : retranscris fidèlement le texte parlé avec ponctuation naturelle et soignée (points, virgules, majuscules).
   - Utilise l'action "dictate" avec "target": null, "window": null, "press_enter": false (ou true si "entrée" explicite).
   - Maintiens STRICTEMENT "narration": "" (silence complet).
4. Silence / Bruit :
   - Si inintelligible ou bruit de micro : "invalid_request": true, "narration": "", "actions": [].

Format JSON obligatoire :
{
  "narration": "",
  "actions": [
    {"action": "dictate", "text": "texte à insérer", "target": null, "window": null, "press_enter": false}
  ],
  "quick_suggestions": []
}"#;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PromptTrigger {
    User,
    Surveillance,
    Immersion { screen_changed: bool },
}

async fn call_llm_prompt(
    llm_config: LlmConfig,
    user_instruction: String,
    full_prompt: String,
    history: Arc<tokio::sync::Mutex<Vec<ChatMessage>>>,
    event_tx: StdSender<AgentEvent>,
    last_call_time: Arc<tokio::sync::Mutex<Option<Instant>>>,
    trigger: PromptTrigger,
    request_id: u64,
) {
    if is_request_cancelled(request_id) {
        return;
    }

    // Si la consigne est vide ou composée uniquement de bruits/symboles parasites, ne pas solliciter le LLM
    if user_instruction.trim().is_empty() || !user_instruction.chars().any(|c| c.is_alphanumeric()) {
        println!("[LLM] Instruction vide ou bruit parasite ignoré, aucune requête LLM envoyée.");
        let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
        return;
    }

    AGENT_BUSY.store(true, Ordering::SeqCst);
    let _busy_guard = BusyGuard;
    let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Thinking));

    if llm_config.api_key.is_empty() {
        let _ = event_tx.send(AgentEvent::ReplaceNarration {
            text: format!("Veuillez renseigner {} dans le fichier .env.", llm_config.provider.env_key_name()),
            quick_suggestions: Vec::new(),
        });
        let _ = event_tx.send(AgentEvent::RequestApiKey);
        let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
        return;
    }

    let client = reqwest::Client::new();
    let (_, system_lang) = get_system_locale_language();
    let is_dictation = IS_DICTATION_ACTIVE.load(Ordering::SeqCst);
    let system_instructions = if is_dictation {
        DICTATION_SYSTEM_INSTRUCTIONS.to_string()
    } else {
        build_standard_system_instructions(&system_lang)
    };

    let safe_user_instruction = truncate_with_notice(&user_instruction, 1000);
    let safe_full_prompt = truncate_with_notice(&full_prompt, 32000);
    {
        let mut hist_guard = history.lock().await;
        hist_guard.push(ChatMessage {
            role: "user".to_string(),
            content: safe_full_prompt,
        });
    }

    let mut max_agent_passes = TARGET_REASONING_PASSES.load(Ordering::SeqCst).clamp(1, 15);
    let mut has_spoken = false;
    let mut pass = 1;

    while pass <= max_agent_passes {
        if is_request_cancelled(request_id) {
            return;
        }
        let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Thinking));

        // Compaction progressive de la mémoire si l'historique dépasse le seuil
        {
            let mut hist_guard = history.lock().await;
            let mut time_guard = last_call_time.lock().await;
            compact_history_if_needed(&llm_config, &mut hist_guard, &client, &mut time_guard).await;
        }

        if is_request_cancelled(request_id) {
            return;
        }

        // Debounce : pause minimale entre deux requêtes API
        let min_debounce = if is_dictation {
            Duration::from_millis(400)
        } else if pass == 1 {
            Duration::from_millis(3000)
        } else {
            Duration::from_millis(2000)
        };
        let prev_call = { *last_call_time.lock().await };
        if let Some(prev) = prev_call {
            let elapsed = prev.elapsed();
            if elapsed < min_debounce {
                tokio::time::sleep(min_debounce - elapsed).await;
            }
        }

        if is_request_cancelled(request_id) {
            return;
        }

        let hist_snapshot = history.lock().await.clone();

        let (temperature, max_tokens) = if is_dictation {
            (0.3, 300)
        } else {
            (0.7, 1800)
        };

        let mut attempts = 0;
        const MAX_RETRIES: usize = 3;
        const DEFAULT_RETRY_DELAY: Duration = Duration::from_millis(4000);
        let mut success_payload: Option<AgentResponsePayload> = None;

        loop {
            if is_request_cancelled(request_id) {
                return;
            }
            attempts += 1;

            let response = send_llm_chat_completion(
                &client,
                &llm_config,
                &system_instructions,
                &hist_snapshot,
                temperature,
                max_tokens,
            ).await;

            match response {
                Ok(res) if res.status().is_success() => {
                    {
                        let mut time_guard = last_call_time.lock().await;
                        *time_guard = Some(Instant::now());
                    }
                    if let Ok(raw_content) = parse_llm_response_text(res, llm_config.provider).await {
                        println!("\n=================== [Réponse {} brute (passe {}/{})] ===================", llm_config.provider.display_name(), pass, max_agent_passes);
                        println!("{}", raw_content.trim());
                        println!("============================================================");

                        history.lock().await.push(ChatMessage {
                            role: "assistant".to_string(),
                            content: raw_content.clone(),
                        });

                        let payload = parse_agent_response(&raw_content);
                        success_payload = Some(payload);
                        break;
                    }
                    history.lock().await.pop();
                    if !is_request_cancelled(request_id) {
                        let _ = event_tx.send(AgentEvent::ReplaceNarration {
                            text: "Format de réponse inattendu.".into(),
                            quick_suggestions: Vec::new(),
                        });
                    }
                    break;
                }
                Ok(res) if res.status() == reqwest::StatusCode::TOO_MANY_REQUESTS && attempts <= MAX_RETRIES => {
                    let wait_secs = res.headers()
                        .get("retry-after")
                        .and_then(|h| h.to_str().ok())
                        .and_then(|s| s.parse::<u64>().ok())
                        .unwrap_or(4);

                    let safe_wait_secs = (wait_secs + 1).max(4);

                    // Si le délai dépasse 12 secondes, il s'agit d'une saturation de jetons : ne pas bloquer l'agent
                    if wait_secs > 12 {
                        let err_body = res.text().await.unwrap_or_default();
                        println!("[LLM 429] Détails renvoyés par l'API : {}", err_body);
                        history.lock().await.pop();
                        let mins = (wait_secs + 59) / 60;
                        if !is_request_cancelled(request_id) {
                            let _ = event_tx.send(AgentEvent::ReplaceNarration {
                                text: format!("Plafond instantané DeepSeek atteint (pause requise par l'API : {mins} min). Historique réinitialisé."),
                                quick_suggestions: Vec::new(),
                            });
                        }
                        history.lock().await.clear();
                        TARGET_REASONING_PASSES.store(5, Ordering::SeqCst);
                        break;
                    }

                    let _ = event_tx.send(AgentEvent::SilentNarration(format!(
                        "Limite de requêtes atteinte. Pause de {safe_wait_secs} s avant réessai..."
                    )));
                    tokio::time::sleep(Duration::from_secs(safe_wait_secs)).await;
                    {
                        let mut time_guard = last_call_time.lock().await;
                        *time_guard = Some(Instant::now());
                    }
                    continue;
                }
                Ok(res) => {
                    {
                        let mut time_guard = last_call_time.lock().await;
                        *time_guard = Some(Instant::now());
                    }
                    history.lock().await.pop();
                    let status = res.status();
                    if !is_request_cancelled(request_id) {
                        let _ = event_tx.send(AgentEvent::ReplaceNarration {
                            text: format!("Erreur API {} : {status}", llm_config.provider.display_name()),
                            quick_suggestions: Vec::new(),
                        });
                    }
                    break;
                }
                Err(_err) if attempts <= MAX_RETRIES => {
                    let _ = event_tx.send(AgentEvent::SilentNarration(
                        "Connexion interrompue, nouvelle tentative dans 3 secondes...".into(),
                    ));
                    tokio::time::sleep(DEFAULT_RETRY_DELAY).await;
                    {
                        let mut time_guard = last_call_time.lock().await;
                        *time_guard = Some(Instant::now());
                    }
                    continue;
                }
                Err(err) => {
                    {
                        let mut time_guard = last_call_time.lock().await;
                        *time_guard = Some(Instant::now());
                    }
                    history.lock().await.pop();
                    if !is_request_cancelled(request_id) {
                        let _ = event_tx.send(AgentEvent::ReplaceNarration {
                            text: format!("Erreur réseau : {err}"),
                            quick_suggestions: Vec::new(),
                        });
                    }
                    break;
                }
            }
        }

        let Some(mut payload) = success_payload else {
            break;
        };

        if is_request_cancelled(request_id) {
            return;
        }

        // Prise en compte immédiate du champ racine next_passes
        if let Some(np) = payload.next_passes {
            let clamped = np.clamp(1, 15);
            max_agent_passes = clamped;
            TARGET_REASONING_PASSES.store(clamped, Ordering::SeqCst);
            println!("[Agent] Quota de raisonnement mis à jour via next_passes : {max_agent_passes} passe(s) max");
        }

        if payload.invalid_request
            || payload.narration.trim().eq_ignore_ascii_case("invalid_request")
            || (payload.narration.trim().is_empty() && payload.actions.is_empty())
        {
            println!("[Agent] Requête incomplète, bruit parasite ou incomprise (narration vide sans action) : mode silencieux activé (aucun affichage, TTS ni requête LLM supplémentaire).");
            history.lock().await.pop();
            let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
            let _ = event_tx.send(AgentEvent::RequestIgnored);
            return;
        }

        // 2. Dispatch immédiat dès réception du message pour les fenêtres volantes
        let shown_card_ids: Vec<String> = payload.actions.iter().filter_map(|act| match act {
            AgentAction::ShowFloatingCard { id: Some(cid), .. } => Some(cid.clone()),
            _ => None,
        }).collect();

        for act in &payload.actions {
            match act {
                AgentAction::ShowFloatingCard { id, title, card_type, data } => {
                    let card = FloatingCard::from_action_data(id.clone(), title.clone(), card_type, data);
                    let _ = event_tx.send(AgentEvent::ShowFloatingCard(card));
                }
                AgentAction::CloseFloatingCard { id } => {
                    if shown_card_ids.contains(id) {
                        println!("[Agent] Ignoré close_floating_card(\"{id}\") car la carte a été mise à jour dans la même passe.");
                    } else {
                        let _ = event_tx.send(AgentEvent::CloseFloatingCard(id.clone()));
                    }
                }
                _ => {}
            }
        }

        // En mode immersion, si l'écran n'a pas bougé et qu'aucune action ni suggestion rapide n'est formulée, préserver le silence
        if let PromptTrigger::Immersion { screen_changed } = trigger {
            let no_actions_or_suggestions = payload.actions.is_empty() && payload.quick_suggestions.is_empty();
            if (!screen_changed && no_actions_or_suggestions) || (payload.narration.trim().is_empty() && no_actions_or_suggestions) {
                println!("[Immersion] Écran inchangé ou silence demandé sans action/suggestion : préservation du silence.");
                let mut hist_guard = history.lock().await;
                hist_guard.pop();
                hist_guard.pop();
                let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                return;
            }
        }

        if trigger == PromptTrigger::Surveillance
            && payload.actions.is_empty() && payload.quick_suggestions.is_empty() && payload.narration.trim().is_empty()
        {
            println!("[Surveillance] L'IA a analysé l'écran : aucune action nécessaire.");
            let mut hist_guard = history.lock().await;
            hist_guard.pop();
            hist_guard.pop();
            let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
            return;
        }

        println!("[Agent] Narration (passe {}) : \"{}\"", pass, payload.narration);
        println!("[Agent] {} action(s) planifiée(s) :", payload.actions.len());
        for (i, act) in payload.actions.iter().enumerate() {
            println!("  [{}] {:?}", i + 1, act);
        }

        if payload.actions.is_empty() {
            println!("[Agent] Tâche accomplie : aucune action supplémentaire. Fin de la séquence après {} passe(s).", pass);
            if payload.quick_suggestions.is_empty() {
                let screen = {
                    LAST_SCREEN_SUMMARY.lock().ok().and_then(|s| s.clone()).unwrap_or_else(|| {
                        #[cfg(windows)]
                        { summarize_screen_state(None, true) }
                        #[cfg(not(windows))]
                        { summarize_screen_state(None, false) }
                    })
                };
                payload.quick_suggestions = generate_fallback_suggestions_from_screen(&screen);
            }
            if !payload.narration.trim().is_empty() || !payload.quick_suggestions.is_empty() {
                if !sanitize_for_tts(&payload.narration).trim().is_empty() {
                    has_spoken = true;
                }
                let _ = event_tx.send(AgentEvent::ReplaceNarration {
                    text: payload.narration.clone(),
                    quick_suggestions: payload.quick_suggestions.clone(),
                });
            }
            break;
        }

        if is_request_cancelled(request_id) {
            return;
        }

        if !payload.narration.trim().is_empty() || !payload.quick_suggestions.is_empty() {
            if !sanitize_for_tts(&payload.narration).trim().is_empty() {
                has_spoken = true;
            }
            let _ = event_tx.send(AgentEvent::ReplaceNarration {
                text: payload.narration.clone(),
                quick_suggestions: payload.quick_suggestions.clone(),
            });
        }

        let actions_to_run = payload.actions;

        let has_web_action = actions_to_run.iter().any(|a| {
            matches!(
                a,
                AgentAction::OpenBrowser { .. }
                    | AgentAction::NavigateToUrl { .. }
                    | AgentAction::ClickElement { .. }
                    | AgentAction::ClickButton { .. }
                    | AgentAction::Scroll { .. }
                    | AgentAction::SummarizeScreen { .. }
                    | AgentAction::FocusWindow { .. }
            )
        });

        let (report, screen_after) = tokio::task::spawn_blocking(move || {
            if is_request_cancelled(request_id) {
                return ("Actions interrompues.".to_string(), String::new());
            }
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let rep = execute_system_actions(&actions_to_run, request_id);
                if is_request_cancelled(request_id) {
                    return (rep, String::new());
                }
                if is_dictation {
                    return (rep, String::new());
                }
                let wait_ms = if has_web_action { 2200 } else { 1200 };
                std::thread::sleep(Duration::from_millis(wait_ms));
                if is_request_cancelled(request_id) {
                    return (rep, String::new());
                }
                #[cfg(windows)]
                let sc = summarize_screen_state(None, true);
                #[cfg(not(windows))]
                let sc = summarize_screen_state(None, false);
                (rep, sc)
            }));
            match outcome {
                Ok(res) => res,
                Err(err) => {
                    eprintln!("[Résilience] Panique interceptée lors de l'exécution des actions : {:?}", err);
                    ("Une action système a rencontré une erreur non critique et a été interrompue en toute sécurité.".to_string(), String::new())
                }
            }
        }).await.unwrap_or_else(|_| ("Erreur d'exécution du worker.".to_string(), String::new()));

        if is_request_cancelled(request_id) {
            return;
        }

        // Stratégie de Rollback / Undo automatique si l'action a échoué pendant une passe d'exploration
        let action_failed = report.contains("introuvable")
            || report.contains("non réactive")
            || report.contains("Ciblage rejeté");
        if action_failed && pass > 1 {
            println!("[Résilience Agent] Échec d'action détecté dans la passe {pass}. Déclenchement de l'undo automatique.");
            let _ = undo_last_system_action();
        }

        // Re-synchronisation du quota de passes si l'action SetReasoningPasses a été invoquée
        let synced_passes = TARGET_REASONING_PASSES.load(Ordering::SeqCst).clamp(1, 15);
        if synced_passes != max_agent_passes {
            max_agent_passes = synced_passes;
            println!("[Agent] Quota de raisonnement resynchronisé avec TARGET_REASONING_PASSES : {max_agent_passes} passe(s) max");
        }

        if is_dictation {
            println!("[Dictée] Action exécutée instantanément, fin de séquence.");
            break;
        }

        if !screen_after.trim().is_empty() {
            if let Ok(mut lock) = LAST_SCREEN_SUMMARY.lock() {
                *lock = Some(screen_after.clone());
            }
        }

        if pass < max_agent_passes {
            let compact_report = truncate_with_notice(&report, 10000);
            let mut step_feedback = format!("[Retour d'exécution étape {pass}] :\n{compact_report}");
            if !screen_after.trim().is_empty() {
                let screen_after_clean = if screen_after.contains("<html") || screen_after.contains("<table") || screen_after.contains("<body") {
                    html_util::sanitize_html_to_markdown(&screen_after)
                } else {
                    screen_after.clone()
                };
                let max_screen = if screen_after_clean.contains("[Page web") || screen_after_clean.contains("[Navigateur]") {
                    15360
                } else {
                    12000
                };
                let safe_screen = truncate_with_notice(screen_after_clean.trim(), max_screen);
                step_feedback.push_str(&format!("\n\n[État de l'écran suite aux actions] :\n{safe_screen}"));
            }

            // Injection de l'état des fenêtres volantes ouvertes dans les étapes multi-passes
            if let Ok(guard) = OPEN_FLOATING_CARDS.lock() {
                if !guard.is_empty() {
                    step_feedback.push_str("\n\n[Fenêtres volantes ouvertes à l'écran] :\n");
                    for c in guard.iter() {
                        step_feedback.push_str(&format!("- ID: \"{}\" | Titre: \"{}\" | Type: \"{}\"\n", c.id, c.title, c.card_type));
                        if !c.content_preview.is_empty() {
                            let preview = truncate_with_notice(&c.content_preview, 1500);
                            step_feedback.push_str(&format!("  [Contenu actuel déjà affiché à conserver/fusionner] :\n\"\"\"\n{}\n\"\"\"\n", preview.trim()));
                        }
                    }
                    step_feedback.push_str("RÈGLE DE FUSION IMPÉRATIVE : Conserve et fusionne les éléments déjà présents ci-dessus avec les nouvelles informations dans \"show_floating_card\". Ne ferme JAMAIS la carte (\"close_floating_card\") !");
                }
            }

            step_feedback.push_str(&format!(
                "\n\n[Consigne étape] : Poursuis : \"{}\".\n\
                - RÈGLE D'OR : Réponds STRICTEMENT et UNIQUEMENT avec un objet JSON valide (contenant \"narration\", \"next_passes\", \"actions\", \"quick_suggestions\"). Aucun texte libre ou markdown en dehors du JSON.\n\
                - Audio, son & paramètres OS -> INTERDICTION de faire 'run_command' (cmd/explorer). Utilise 'set_volume', 'mute_audio' ou 'accessibility_shortcut' ('action_center' Win+A, 'open_settings' Win+I, 'm', 'space').\n\
                - Bannière cookies -> clique automatiquement 'Tout accepter' / 'Accepter'.\n\
                - Bannière de notification web / push ('Activez les notifications franceinfo...', etc.) -> traite-la immédiatement en cliquant sur la croix 'Fermer (croix ✕)' / 'Fermer' / 'Refuser' / 'Plus tard' ou 'Activer' pour libérer la vue.\n\
                - Pour lancer ou contrôler une vidéo ou un audio -> clique directement sur l'élément vidéo/audio ou son bouton de lecture ('play' / 'lecture').\n\
                - Notification système (toast) -> lis son contenu dans ta narration et laisse-la tranquille (ne la ferme pas).\n\
                - Alerte / modale d'erreur -> lis le message dans ta narration, puis clique impérativement sur 'OK' / 'Fermer' pour la refermer.\n\
                - Fenêtre volante Markdown -> insère des liens cliquables [link=\"action:...\"]Texte[/link] pour permettre à l'utilisateur de déclencher des suites ou choix en un clic.\n\
                - Liens interactifs -> Formate SYSTÉMATIQUEMENT vidéos, audios, images, PDF/fichiers et actions en liens cliquables via la macro [link=\"action:...\"]Icône Libellé[/link] ou [link=\"https://...\"]🌐 Web[/link] (y compris dans les cellules de tableaux Markdown). Le clic envoie directement la commande dans le chat de Libertide.\n\
                - Fenêtre volante (\"show_floating_card\") -> CONSERVE et FUSIONNE impérativement les données précédentes avec les nouvelles (ne supprime pas les éléments déjà trouvés lors des passes précédentes). INTERDICTION d'émettre \"close_floating_card\" pour une carte que tu viens d'ouvrir ou mettre à jour.\n\
                 - Mission approfondie (contact / produit / service) -> définis 'next_passes' (8 à 12), explore méthodiquement (local -> web -> annuaires) et projette les résultats dans 'show_floating_card'. Ne renvoie pas 'actions': [] tant que les résultats ne sont pas trouvés.\n\
                - Échec/blocage -> pivote immédiatement vers une alternative (app, web, autre fenêtre). Ne renvoie JAMAIS 'actions': [] si l'objectif n'est pas achevé.\n\
                - Tâche finie ou choix requis -> 'actions': [] + 2 à 4 'quick_suggestions'.",
                safe_user_instruction
            ));

            history.lock().await.push(ChatMessage {
                role: "user".to_string(),
                content: step_feedback,
            });
            // Marquer la fin de l'exécution pour que le debounce de la passe suivante s'applique bien
            {
                let mut time_guard = last_call_time.lock().await;
                *time_guard = Some(Instant::now());
            }
            pass += 1;
        } else {
            println!("[Agent] Nombre maximal de passes ({max_agent_passes}) atteint.");
            let screen_for_sug = if !screen_after.trim().is_empty() {
                screen_after.clone()
            } else {
                LAST_SCREEN_SUMMARY.lock().ok().and_then(|s| s.clone()).unwrap_or_default()
            };
            let auto_suggestions = generate_fallback_suggestions_from_screen(&screen_for_sug);
            has_spoken = true;
            let _ = event_tx.send(AgentEvent::ReplaceNarration {
                text: "Actions terminées. Voici les suites possibles identifiées à l'écran :".to_string(),
                quick_suggestions: auto_suggestions,
            });
            break;
        }
    }

    // Libération explicite du verrou d'activité avant d'émettre l'état Idle
    drop(_busy_guard);

    if !is_request_cancelled(request_id) && !has_spoken {
        let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
    }
}

async fn analyze_navigation_zones_with_llm(
    client: &reqwest::Client,
    config: &LlmConfig,
    window_title: &str,
    targets: &[NumberedElementTarget],
) -> Vec<NavigationZone> {
    if targets.is_empty() {
        return Vec::new();
    }

    let mut prompt_content = format!("Fenêtre analysée : \"{}\"\nContrôles interactifs détectés :\n", window_title);
    for t in targets.iter().take(40) {
        prompt_content.push_str(&format!("- ID #{}: \"{}\" [x:{}, y:{}, w:{}, h:{}]\n", t.index, t.name, t.rect[0], t.rect[1], t.rect[2] - t.rect[0], t.rect[3] - t.rect[1]));
    }

    let sys_prompt = r#"Tu es un expert d'ergonomie et d'accessibilité numérique pour moteur Switch Access / Contacteur.
Reçois la liste des éléments interactifs d'une fenêtre d'ordinateur.
Découpe et regroupe ces éléments en 2 à 4 blocs d'interaction logiques évidents (ex: "En-tête et commandes de fenêtre", "Barre de navigation et onglets", "Formulaire et recherche", "Contenu principal", "Pied de page et actions").
RÈGLE ESSENTIELLE : Attribue à chaque zone un "title" court, explicite et naturel qui nomme la fonction de navigation (ex: "Navigation et onglets", "Recherche", "Contenu principal", "Boutons d'action").
INTERDICTION FORMELLE de nommer les blocs "Bloc 1", "Bloc 2", "Zone 1", "Bloc 1/4" ou similaire : donne toujours le nom réel de la section ou de la navigation.
Réponds STRICTEMENT en JSON :
{
  "zones": [
    {
      "title": "Nom explicite du bloc de navigation",
      "description": "Courte description pour synthèse vocale",
      "target_indices": [1, 2, 3]
    }
  ]
}"#;

    let messages = vec![ChatMessage {
        role: "user".to_string(),
        content: prompt_content,
    }];

    let response = send_llm_chat_completion(client, config, sys_prompt, &messages, 0.2, 700).await;

    if let Ok(res) = response {
        if res.status().is_success() {
            if let Ok(raw) = parse_llm_response_text(res, config.provider).await {
                let clean = raw.trim();
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
                let json_candidate = if let (Some(s), Some(e)) = (unquoted.find('{'), unquoted.rfind('}')) {
                    if s <= e { &unquoted[s..=e] } else { unquoted }
                } else {
                    unquoted
                };
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_candidate) {
                    if let Some(zones_arr) = val.get("zones").and_then(|z| z.as_array()) {
                        let mut result_zones = Vec::new();
                        for z in zones_arr {
                            let title = z.get("title").and_then(|v| v.as_str()).unwrap_or("Zone").trim().to_string();
                            let desc = z.get("description").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                            let mut zone_targets = Vec::new();
                            if let Some(indices) = z.get("target_indices").and_then(|v| v.as_array()) {
                                for idx_val in indices {
                                    if let Some(idx) = idx_val.as_u64() {
                                        if let Some(t) = targets.iter().find(|t| t.index == idx as usize) {
                                            zone_targets.push(t.clone());
                                        }
                                    }
                                }
                            }
                            if !zone_targets.is_empty() {
                                let min_x = zone_targets.iter().map(|t| t.rect[0]).min().unwrap_or(0);
                                let min_y = zone_targets.iter().map(|t| t.rect[1]).min().unwrap_or(0);
                                let max_x = zone_targets.iter().map(|t| t.rect[2]).max().unwrap_or(0);
                                let max_y = zone_targets.iter().map(|t| t.rect[3]).max().unwrap_or(0);
                                result_zones.push(NavigationZone {
                                    title,
                                    description: desc,
                                    click_x: min_x + (max_x - min_x) / 2,
                                    click_y: min_y + (max_y - min_y) / 2,
                                    rect: [min_x, min_y, max_x, max_y],
                                    targets: zone_targets,
                                });
                            }
                        }
                        if !result_zones.is_empty() {
                            return result_zones;
                        }
                    }
                }
            }
        }
    }

    // Repli géométrique robuste si pas de réseau ou absence de clé LLM
    let mut top_targets = Vec::new();
    let mut main_targets = Vec::new();
    for t in targets {
        if t.rect[1] < 55 {
            top_targets.push(t.clone());
        } else {
            main_targets.push(t.clone());
        }
    }

    let mut fallback_zones = Vec::new();
    if !top_targets.is_empty() {
        let min_x = top_targets.iter().map(|t| t.rect[0]).min().unwrap_or(0);
        let min_y = top_targets.iter().map(|t| t.rect[1]).min().unwrap_or(0);
        let max_x = top_targets.iter().map(|t| t.rect[2]).max().unwrap_or(0);
        let max_y = top_targets.iter().map(|t| t.rect[3]).max().unwrap_or(0);
        fallback_zones.push(NavigationZone {
            title: "Barre supérieure et commandes".to_string(),
            description: "Boutons de fenêtre et en-tête".to_string(),
            click_x: min_x + (max_x - min_x) / 2,
            click_y: min_y + (max_y - min_y) / 2,
            rect: [min_x, min_y, max_x, max_y],
            targets: top_targets,
        });
    }
    if !main_targets.is_empty() {
        let min_x = main_targets.iter().map(|t| t.rect[0]).min().unwrap_or(0);
        let min_y = main_targets.iter().map(|t| t.rect[1]).min().unwrap_or(0);
        let max_x = main_targets.iter().map(|t| t.rect[2]).max().unwrap_or(0);
        let max_y = main_targets.iter().map(|t| t.rect[3]).max().unwrap_or(0);
        fallback_zones.push(NavigationZone {
            title: "Corps et zone principale".to_string(),
            description: "Contenu interactif et champs".to_string(),
            click_x: min_x + (max_x - min_x) / 2,
            click_y: min_y + (max_y - min_y) / 2,
            rect: [min_x, min_y, max_x, max_y],
            targets: main_targets,
        });
    }
    fallback_zones
}

fn main() -> eframe::Result<()> {
    // Si l'application est lancée au démarrage de Windows (CWD = System32),
    // repositionner le répertoire courant sur le dossier du projet ou de l'exécutable.
    if let Ok(cwd) = std::env::current_dir() {
        let cwd_str = cwd.to_string_lossy().to_lowercase();
        if cwd_str.contains("system32") || cwd_str.contains("syswow64") {
            if let Ok(exe) = std::env::current_exe() {
                if let Some(parent) = exe.parent() {
                    let project_dir = parent.parent().and_then(|p| p.parent()).filter(|p| p.join("Cargo.toml").exists());
                    let target_dir = project_dir.unwrap_or(parent);
                    let _ = std::env::set_current_dir(target_dir);
                }
            }
        }
    }

    let (event_tx, event_rx) = channel::<AgentEvent>();
    let (cmd_tx, mut cmd_rx) = unbounded_channel::<AgentCommand>();
    let initial_voice = resolve_initial_voice();
    let tts_tx = spawn_tts_worker(event_tx.clone(), initial_voice);
    let (twitch_ch_tx, twitch_ch_rx) = channel::<String>();
    spawn_twitch_worker(event_tx.clone(), twitch_ch_rx);
    let llm_config = resolve_llm_config();
    let audio_tx = spawn_audio_worker(event_tx.clone(), llm_config.api_key.clone());

    let initial_twitch_channel = std::env::var("TWITCH_CHANNEL").unwrap_or_default();

    // Horloge d'analyse périodique d'écran toutes les 60 secondes
    let periodic_cmd_tx = cmd_tx.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(60));
            let _ = periodic_cmd_tx.send(AgentCommand::PeriodicScreenCheck);
        }
    });

    // Runtime Tokio en arrière-plan pour requêter le fournisseur LLM
    let bg_llm_config = llm_config.clone();
    let bg_tts_tx = tts_tx.clone();
    let scheduler_cmd_tx = cmd_tx.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("Échec d'initialisation du runtime Tokio");
        automation::init_scheduler(scheduler_cmd_tx, rt.handle().clone());
        rt.block_on(async move {
            let history = Arc::new(tokio::sync::Mutex::new(Vec::<ChatMessage>::new()));
            let last_call_time = Arc::new(tokio::sync::Mutex::new(None));
            let mut current_task: Option<tokio::task::JoinHandle<()>> = None;

            while let Some(cmd) = cmd_rx.recv().await {
                match cmd {
                    AgentCommand::AnalyzeNavigationZones { window_title, targets } => {
                        let client = reqwest::Client::new();
                        let current_llm_config = bg_llm_config.clone();
                        let event_tx = event_tx.clone();
                        tokio::spawn(async move {
                            let zones = analyze_navigation_zones_with_llm(&client, &current_llm_config, &window_title, &targets).await;
                            let _ = event_tx.send(AgentEvent::NavigationZonesReady {
                                window_title,
                                zones,
                            });
                        });
                    }
                    AgentCommand::Prompt(prompt) => {
                        if prompt.trim().is_empty() || !prompt.chars().any(|c| c.is_alphanumeric()) {
                            println!("[Agent] Commande vide ignorée (bruit parasite), aucune requête LLM.");
                            let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                            continue;
                        }
                        let request_id = CURRENT_REQUEST_ID.fetch_add(1, Ordering::SeqCst) + 1;
                        if let Some(handle) = current_task.take() {
                            handle.abort();
                        }
                        let _ = bg_tts_tx.send(TtsCommand::Stop);
                        IS_EMERGENCY_STOPPED.store(false, Ordering::SeqCst);
                        let initial_passes = 6;
                        TARGET_REASONING_PASSES.store(initial_passes, Ordering::SeqCst);

                        let history = history.clone();
                        let last_call_time = last_call_time.clone();
                        let event_tx = event_tx.clone();
                        let current_llm_config = bg_llm_config.clone();

                        current_task = Some(tokio::spawn(async move {
                            if is_request_cancelled(request_id) {
                                return;
                            }
                            if let Some(cli_feedback) = try_execute_direct_cli(&prompt) {
                                if is_request_cancelled(request_id) { return; }
                                let _ = event_tx.send(AgentEvent::ScreenPayloadSize(0.0));
                                let _ = event_tx.send(AgentEvent::ReplaceNarration {
                                    text: cli_feedback.clone(),
                                    quick_suggestions: Vec::new(),
                                });
                                let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                            } else if let Some(text_to_write) = parse_write_command(&prompt) {
                                if is_request_cancelled(request_id) { return; }
                                let _ = event_tx.send(AgentEvent::ScreenPayloadSize(0.0));
                                let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Thinking));
                                let narration = tokio::task::spawn_blocking(move || {
                                    write_to_browser_or_txt(&text_to_write)
                                }).await.unwrap_or_else(|_| "Erreur lors de l'écriture.".to_string());
                                if is_request_cancelled(request_id) { return; }
                                let _ = event_tx.send(AgentEvent::ReplaceNarration {
                                    text: narration,
                                    quick_suggestions: Vec::new(),
                                });
                                let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                            } else if IS_DICTATION_ACTIVE.load(Ordering::SeqCst) {
                                if is_request_cancelled(request_id) { return; }
                                let _ = event_tx.send(AgentEvent::ScreenPayloadSize(0.0));
                                let final_prompt = format!(
                                    "[MODE DICTÉE ACTIF] : L'utilisateur est en mode dictée vocale. Analyse sa voix : s'il s'agit d'une consigne pour arrêter/quitter la dictée, appelle 'deactivate_dictation'. S'il s'agit de texte à insérer, retranscris-le avec ponctuation soignée via 'dictate' (target: null) et garde narration: \"\".\n\n[Demande utilisateur]\n{prompt}"
                                );
                                call_llm_prompt(
                                    current_llm_config,
                                    prompt.clone(),
                                    final_prompt,
                                    history,
                                    event_tx,
                                    last_call_time,
                                    PromptTrigger::User,
                                    request_id,
                                ).await;
                            } else {
                                let (field_content, screen_state): (Option<String>, String) = tokio::task::spawn_blocking(|| {
                                    #[cfg(windows)]
                                    {
                                        (get_active_field_content(), summarize_screen_state(None, true))
                                    }
                                    #[cfg(not(windows))]
                                    {
                                        (get_active_field_content(), summarize_screen_state(None, false))
                                    }
                                }).await.unwrap_or((None, String::new()));

                                if is_request_cancelled(request_id) { return; }
                                let mut prompt_sections = Vec::new();

                                if !screen_state.trim().is_empty() {
                                    if let Ok(mut lock) = LAST_SCREEN_SUMMARY.lock() {
                                        *lock = Some(screen_state.clone());
                                    }
                                    let screen_state = if screen_state.contains("<html") || screen_state.contains("<table") || screen_state.contains("<body") {
                                        html_util::sanitize_html_to_markdown(&screen_state)
                                    } else {
                                        screen_state
                                    };
                                    let max_screen = if screen_state.contains("[Page web") || screen_state.contains("[Navigateur]") {
                                        15360
                                    } else {
                                        12000
                                    };
                                    let safe_screen = truncate_with_notice(screen_state.trim(), max_screen);
                                    let kb = safe_screen.len() as f32 / 1024.0;
                                    let _ = event_tx.send(AgentEvent::ScreenPayloadSize(kb));
                                    prompt_sections.push(format!("[État actuel de l'écran]\n{safe_screen}"));
                                } else {
                                    let _ = event_tx.send(AgentEvent::ScreenPayloadSize(0.0));
                                }

                                if let Some(content) = field_content {
                                    if !content.trim().is_empty() {
                                        let safe_content = truncate_with_notice(content.trim(), 1200);
                                        prompt_sections.push(format!("[Contenu actuel du champ de saisie]\n\"\"\"\n{safe_content}\n\"\"\""));
                                    }
                                }

                            // 1. Injection automatique de l'état des fenêtres volantes ouvertes au LLM
                            if let Ok(guard) = OPEN_FLOATING_CARDS.lock() {
                                if !guard.is_empty() {
                                    let mut card_section = String::from("[Fenêtres volantes actuellement ouvertes à l'écran] :\n");
                                    for c in guard.iter() {
                                        card_section.push_str(&format!("- ID: \"{}\" | Titre: \"{}\" | Type: \"{}\"\n", c.id, c.title, c.card_type));
                                        if !c.content_preview.is_empty() {
                                            let preview = truncate_with_notice(&c.content_preview, 1500);
                                            card_section.push_str(&format!("  [Contenu actuel déjà affiché à conserver/fusionner] :\n\"\"\"\n{}\n\"\"\"\n", preview.trim()));
                                        }
                                    }
                                    card_section.push_str("Instruction : Ces fenêtres volantes sont déjà affichées à l'écran. Si ta réponse apporte de nouvelles données, une synthèse ou une mise à jour, réémets impérativement \"show_floating_card\" avec le même ID en FUSIONNANT les données existantes. Ne ferme JAMAIS la carte si tu la mets à jour.");
                                    prompt_sections.push(card_section);
                                }
                            }

                                prompt_sections.push(format!("[Demande utilisateur]\n{prompt}"));
                                let final_prompt = prompt_sections.join("\n\n");

                                call_llm_prompt(
                                    current_llm_config,
                                    prompt.clone(),
                                    final_prompt,
                                    history,
                                    event_tx,
                                    last_call_time,
                                    PromptTrigger::User,
                                    request_id,
                                ).await;
                            }
                        }));
                    }
                    AgentCommand::ClearHistory => {
                        let _ = CURRENT_REQUEST_ID.fetch_add(1, Ordering::SeqCst);
                        if let Some(handle) = current_task.take() {
                            handle.abort();
                        }
                        let _ = bg_tts_tx.send(TtsCommand::Stop);
                        cancel_all_scheduled_tasks();
                        IS_EMERGENCY_STOPPED.store(false, Ordering::SeqCst);
                        IS_DICTATION_ACTIVE.store(false, Ordering::SeqCst);
                        TARGET_REASONING_PASSES.store(5, Ordering::SeqCst);
                        if let Ok(mut hist) = IMMERSION_SCREEN_HISTORY.lock() {
                            hist.clear();
                        }
                        history.lock().await.clear();
                        if let Ok(mut lock) = LAST_SCREEN_SUMMARY.lock() {
                            *lock = None;
                        }
                    }
                    AgentCommand::SearchTwitch(query) => {
                        let event_tx = event_tx.clone();
                        tokio::spawn(async move {
                            let results = search_twitch_channels(&query).await;
                            let _ = event_tx.send(AgentEvent::TwitchSearchResults(results));
                        });
                    }
                    AgentCommand::PeriodicScreenCheck => {
                        if IS_EMERGENCY_STOPPED.load(Ordering::SeqCst) {
                            continue;
                        }
                        if AGENT_BUSY.load(Ordering::SeqCst)
                            || current_task.as_ref().map_or(false, |h| !h.is_finished())
                        {
                            println!("[Surveillance] Agent occupé, analyse d'écran différée.");
                            continue;
                        }

                        let request_id = CURRENT_REQUEST_ID.fetch_add(1, Ordering::SeqCst) + 1;
                        let history = history.clone();
                        let last_call_time = last_call_time.clone();
                        let event_tx = event_tx.clone();
                        let current_llm_config = bg_llm_config.clone();

                        current_task = Some(tokio::spawn(async move {
                        #[cfg(windows)]
                        {
                            if is_request_cancelled(request_id) { return; }
                            let current_summary = tokio::task::spawn_blocking(|| {
                                summarize_screen_state(None, false)
                            }).await.unwrap_or_default();

                            if is_request_cancelled(request_id) { return; }

                            let is_immersion = IS_IMMERSION_ACTIVE.load(Ordering::SeqCst);
                            if is_immersion {
                                let (screen_diff, screen_changed) = {
                                    let mut hist = IMMERSION_SCREEN_HISTORY.lock().unwrap_or_else(|e| e.into_inner());
                                    let last_summary = hist.last().cloned();
                                    hist.push(current_summary.clone());
                                    if hist.len() > 6 {
                                        hist.remove(0);
                                    }
                                    if let Some(prev) = last_summary {
                                        let diff = compute_screen_diff(&prev, &current_summary);
                                        let changed = !diff.trim().is_empty();
                                        (diff, changed)
                                    } else {
                                        (String::new(), true)
                                    }
                                };

                                if !screen_changed {
                                    println!("[Immersion] Aucun changement à l'écran, silence préservé.");
                                    return;
                                }

                                let immersion_prompt = if screen_diff.is_empty() {
                                    format!(
                                        "[Suivi du mode immersion - état initial]\n\n\
                                        [État d'écran actuel] :\n{}\n\n\
                                        Instruction : Session d'immersion active. Consigne de silence absolu : réponds STRICTEMENT avec \"narration\": \"\", \"actions\": [], \"quick_suggestions\": [].",
                                        truncate_with_notice(&current_summary, 1500)
                                    )
                                } else {
                                    format!(
                                        "[Suivi du mode immersion - différentiel des changements]\n\n\
                                        {}\n\n\
                                        Instruction impérative : Analyse UNIQUEMENT et STRICTEMENT les lignes de changement ci-dessus (+ et -). \
                                        Ne commente absolument rien de ce qui est fixe ou inchangé. \
                                        Consigne de silence absolu : si ces changements font partie du travail normal de l'utilisateur, réponds STRICTEMENT avec \"narration\": \"\", \"actions\": [], \"quick_suggestions\": []. \
                                        Ne commente que les éléments modifiés si une intervention ou suggestion est réellement utile.",
                                        truncate_with_notice(&screen_diff, 1800)
                                    )
                                };

                                println!("[Immersion] Changement détecté (diff: {} octets), transmission du diff au LLM...", screen_diff.len());
                                call_llm_prompt(current_llm_config, "Suivi immersion".to_string(), immersion_prompt, history, event_tx, last_call_time, PromptTrigger::Immersion { screen_changed: true }, request_id).await;
                                return;
                            }

                            let prev_summary = {
                                let mut lock = LAST_SCREEN_SUMMARY.lock().unwrap_or_else(|e| e.into_inner());
                                let prev = lock.clone();
                                *lock = Some(current_summary.clone());
                                prev
                            };

                            if let Some(prev) = prev_summary {
                                if prev.trim() == current_summary.trim() {
                                    println!("[Surveillance] Aucun changement à l'écran.");
                                    return;
                                }

                                println!("[Surveillance] Changement détecté, transmission du différentiel au LLM...");
                                let diff_prompt = format!(
                                    "[Surveillance périodique automatique de l'écran]\n\n\
                                    [État d'écran précédent] :\n{}\n\n\
                                    [État d'écran actuel] :\n{}\n\n\
                                    Instruction : Analyse les différences entre ces deux états d'écran. \
                                    Détermine si une intervention ou une suite d'actions est nécessaire ou utile pour assister l'utilisateur.\n\
                                    - Si aucune intervention n'est requise : réponds STRICTEMENT avec \"actions\": [] et \"narration\": \"\" pour préserver le silence.\n\
                                    - Si une intervention est pertinente : renseigne \"narration\" pour expliquer ce que tu constates et fournis la suite d'actions à exécuter dans \"actions\".",
                                    truncate_with_notice(&prev, 1500),
                                    truncate_with_notice(&current_summary, 1500)
                                );

                                call_llm_prompt(current_llm_config, "Surveillance écran".to_string(), diff_prompt, history, event_tx, last_call_time, PromptTrigger::Surveillance, request_id).await;
                            } else {
                                println!("[Surveillance] Premier instantané d'écran enregistré.");
                            }
                        }
                        }));
                    }
                    AgentCommand::UndoLastAction => {
                        let _ = CURRENT_REQUEST_ID.fetch_add(1, Ordering::SeqCst);
                        if let Some(handle) = current_task.take() {
                            handle.abort();
                        }
                        let _ = bg_tts_tx.send(TtsCommand::Stop);
                        let feedback = if let Some(fb) = undo_last_system_action() {
                            fb
                        } else {
                            #[cfg(windows)]
                            send_hotkey(&[windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0x11)], windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(0x5A));
                            #[cfg(not(windows))]
                            let _ = std::process::Command::new("xdotool").args(["key", "ctrl+z"]).status();
                            "Dernière action annulée.".to_string()
                        };
                        let _ = event_tx.send(AgentEvent::ReplaceNarration {
                            text: feedback.clone(),
                            quick_suggestions: Vec::new(),
                        });
                        let _ = bg_tts_tx.send(TtsCommand::Speak(feedback));
                        let _ = event_tx.send(AgentEvent::StatusChanged(AgentStatus::Idle));
                    }
                }
            }
        });
    });

    // Fenêtre transparente compacte, ferrée en bas à droite
    let mut viewport = egui::ViewportBuilder::default()
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top()
        .with_resizable(true)
        .with_min_inner_size([380.0, 158.0])
        .with_inner_size([560.0, 178.0]);

    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../public/favicon-256x256.png")) {
        viewport = viewport.with_icon(Arc::new(icon));
    }

    let native_options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "Libertide overlay",
        native_options,
        Box::new(move |cc| {
            Ok(Box::new(OverlayApp::new(
                cc,
                event_rx,
                cmd_tx,
                tts_tx,
                audio_tx,
                twitch_ch_tx,
                initial_twitch_channel,
                initial_voice,
            )))
        }),
    )
}