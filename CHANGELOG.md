# Changelog - Libertide

All notable changes to this project will be documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.5.2] - 2026-10-19

### 🎨 Accessibility Themes & Visual Ergonomics
- **Comprehensive Theme Engine (`AppTheme`)**:
  - Added high-contrast, eye-strain reducing, and colorblind-safe themes:
    - **Cyber Cyan**: Modern neon dark theme (default).
    - **Émeraude Menthe**: Calming forest and mint green palette.
    - **Améthyste Néon**: Synthwave purple and magenta.
    - **High Contrast Yellow (WCAG AAA)**: Pure black and high-visibility gold for low vision.
    - **High Contrast White**: Crisp monochrome styling with pure white text and borders.
    - **Soft Amber**: Warm blue-light reduced palette for night-time comfort.
    - **Colorblind Friendly**: Cobalt blue and high-visibility yellow optimized for deuteranopia and protanopia.
  - **Voice & Modal Control**:
    - Spoken triggers to switch themes on the fly (*"Thème suivant"*, *"Contraste élevé"*, *"Thème sombre"*).
    - Dedicated "Thèmes & Accessibilité" tab in the settings dialog with live preview palette swatches.
    - Theme preference persistence in `.env` (`THEME`).

### 📊 Responsive Floating Cards & Viewport Synchronization
- **Adaptive Table Layouts**:
  - Dynamic proportional column width estimation (`calculate_proportional_col_widths`) based on cell contents and available viewport width.
  - Context menus and shortcuts for copying whole tables, individual rows, or single cells.
- **Floating Window State & Update Pipeline**:
  - Real-time global card registry (`OPEN_FLOATING_CARDS`) keeping LLM reasoning passes synchronized with active floating HUDs.
  - Immediate viewport repaint triggering across card creation and updates.
  - Context preservation preventing cards from accidentally closing when updated within multi-step reasoning chains.

### 🔑 LLM Settings & Window Management
- **Credential Modal Enhancements**:
  - Interactive key inspection with visibility toggle (`show_api_key_plain`), clipboard paste, and custom model reset buttons.
  - Smoothed handling of empty narration responses and silent observation states.
  - Native clipboard support for copying, pasting and cutting (`Ctrl+V`, `Ctrl+C`, `Ctrl+X`) across the prompt input field and credentials dialog, with accurate selection replacement.
- **System Settings & Window Focus**:
  - Direct native shortcuts to open system settings and action center without spawning shell instances.
  - Fixed Omnibox vs Windows Search disambiguation: talking about *"omnibox"* or *"barre d'adresse"* now strictly targets the browser address bar (`focus_address_bar`) with instant zero-latency shortcuts, preventing Windows Search (`Win+S`) from opening.
  - Refined external focus retention and terminal buffer extraction.

---

## [0.5.1] - 2026-10-18

### 🍏 macOS Port & Unix Compatibility
- **Native macOS Support (Apple Silicon M1/M2/M3 & Intel)**:
  - Speech synthesis via the system `say` command with French voice selection (`Amelie`, `Thomas`).
  - Clipboard management on macOS via `pbcopy` and `pbpaste` with heuristic file and image detection.
  - Native document and URL opening using the `open` command.
  - Autostart support managed through `LaunchAgent` (`com.libertide.overlay.plist`).
- **Unix / Linux Stability**:
  - Fixed Unix unit tests and consolidated `xclip` backend for plain text, URI lists, and HTML.
  - Upgraded cryptographic dependency `ring` to version 0.17.13.

### 🤖 Multi-Provider LLM & Reasoning Capacity
- **Expanded API Provider Support**:
  - Unified integration and configuration modal dialog for **OpenAI**, **Anthropic** (Claude), **Mistral AI**, **Zhipu GLM**, **Groq**, and **DeepSeek**.
  - Dynamic persistence of API keys and associated models in the `.env` file.
- **Extended Token Budget**:
  - Increased output generation quota up to 1800 tokens (`max_tokens`) for richer summaries and responses.

### 📊 Interactive Floating Windows & Markdown Rendering
- **Filter and Query Forms (`card_type: "form"`)**:
  - Dynamic generation of interactive forms featuring text inputs, numeric sliders/gauges (`number_slider`), dropdowns (`select`), checkboxes (`checkbox`), and radio buttons (`radio`).
  - Validation and automatic transmission of submitted form criteria to the conversational agent.
- **Rich Markdown Rendering & Responsive Tables**:
  - Support for syntax-highlighted code blocks, blockquotes, horizontal rules, and ordered/unordered lists.
  - Markdown tables with striped rows, horizontal scrolling, and one-click full-table copy.
  - Robust link macro `[link="target"]Label[/link]` and clickable markdown links to trigger actions, file launches, or media playback instantly.

### 🌐 Web Automation & Multimedia Controls
- **Media Detection & Playback Control**:
  - Automatic recognition of `<video>` and `<audio>` tags and player controls via UIA/HTML macros (`[@video]`, `[@audio]`) to start or pause playback.
- **Web Notification Banner Handling**:
  - Automated dismissal or acceptance of intrusive push notification popups and web banners.
- **Microphone Affinity & Lightweight TTS**:
  - Optimized CPAL microphone capture and trimmed direct TTS dependencies in favor of OS-native engines.
  - Reliable executable path resolution during relative launches or executions from `System32`.

## [0.5.0] - 2026-10-17

### ♿ Accessibility & Inclusive Ergonomics
- **Click by Number (`NumberedElementTarget`)**:
  - Dynamic inspection of interactive UI elements (buttons, links, form inputs, tabs) across the active window.
  - High-contrast numbered golden badges overlaid directly over targets.
  - Spoken trigger ("*Numéros*", "*Badges*", "*Click by number*") followed by saying the number to click with zero pointer effort.
- **Single-Switch Access Mode (`switch_access_mode`)**:
  - Automated sequential scanning highlighting active UI actions and quick suggestions.
  - Validation using `Space` (or external single-switch device) to trigger options or toggle the microphone.
- **Adaptive Voice Zoom**:
  - Dynamic rendering scale adjustment via voice (*"Zoom in"*, *"Zoom out"*, *"Grossis"*, *"Taille normale"*) from 70% to 250%.
- **Auditory Feedback (*Earcons*)**:
  - Dedicated acoustic cues (rising/falling pitch tones) for mic state transitions, action confirmation, and emergency stops.

### 📊 Floating Analytical HUDs & File Launching
- **Independent Multi-Viewport Floating Windows (`FloatingCard`)**:
  - Analytical cards rendered in standalone, borderless OS viewports outside the main overlay container.
  - High-contrast *dark-glass* theme with draggable header bars and close buttons.
  - Support for structured data formats:
    - **Tables**: Multi-column comparison grids with striped formatting.
    - **Stats**: Key-value metric dashboards.
    - **Markdown**: Formatted documentation and notes.
- **Native File Launching (`open_file`)**:
  - Universal opening of images, PDFs, spreadsheets, and media documents directly into the OS default application (`cmd /C start` on Windows, `xdg-open` on Unix/Linux).

### 🎙️ Speech Synthesis & Voice Profiles
- **Neural Kokoro-82M TTS Integration**:
  - Integrated `any_tts` with local model caching (`voices/`) and fast fallback to Windows SAPI or Unix speech dispatchers.
  - Dynamic pre-synthesis and sentence pipelining for low-latency playback.
- **Expanded Voice Profiles**:
  - Added and categorized profiles: Denise (natural female), Henri (natural male), Hortense (studio female), Paul (poised male), Sylvie (Canadian French), Jean (dynamic male), and Kokoro (neural).
  - Persistent voice selection stored in `.env` (`VOICE_PROFILE`).
- **Audio Isolation (`mute_audio`)**:
  - Core Audio API mute for third-party media players (Spotify, YouTube, browser streams) while keeping Libertide's TTS voice channel active and audible.

### 🌐 Web & UI Automation Strategies
- **Automated Cookie Consent Handling**:
  - Systematic detection and acceptance of GDPR and cookie banners without interrupting the user.
- **System Toasts & Notification Policy**:
  - Reads incoming toast notifications aloud without forcibly dismissing them, leaving OS window management intact.
- **HTML-to-Markdown & Payload Management**:
  - Refined HTML sanitizer with payload budget bounds (15 KB cap) to optimize token efficiency and LLM reasoning speed.
- **Browser & Console Enhancements**:
  - Dedicated hotkeys for omnibox selection, browser bookmarks toggle (`Ctrl+Shift+B`), tab management, and interactive terminal commands.

### ⏱️ Deferred Execution & Task Scheduling
- **Natural Language Task Scheduling**:
  - Support for `schedule_prompt` and `schedule_prompts` ("*in 10 seconds*", "*dans 5 minutes*").
  - Immediate local cancellation via voice (*"Annule les commandes différées"* / *"Cancel timers"*).

### 🌍 Localization & System Detection
- **Automatic System Locale Detection**:
  - Inspection of OS user default locale (`GetUserDefaultLocaleName` / `LC_ALL`) to adapt system prompts and spoken responses in French, English, Spanish, German, Italian, Portuguese, Russian, Chinese, or Japanese.

---

## [0.4.0] - 2026-09-28

### Added
- Local speech recognition with Whisper (`ggml-base.bin`).
- Window tiling and snapping layouts (2x2 grid, master stack, dual view).
- DeepSeek reasoning pipeline with asynchronous action executions.
- Virtual desktop immersion mode for coding and deep focus.

---

## [0.3.0] - 2026-08-15

### Added
- Twitch IRC chat integration and channel search.
- Direct CLI execution mode prefix (`>` or `$`).
- Transparent overlay built on egui.

---

## [0.2.0] - 2026-07-01

### Added
- Windows UI Automation element discovery and click dispatching.
- Basic voice dictation mode with cursor movement controls.

---

## [0.1.0] - 2026-05-12

### Added
- Initial prototype of Libertide voice agent.