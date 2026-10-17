# Changelog - Libertide

All notable changes to this project will be documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

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