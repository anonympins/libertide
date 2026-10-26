# Changelog - Libertide

All notable changes to this project will be documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---
## [0.6.3] - 2026-11-02

### 🌐 Full Multilingual Localization & Internationalization (`main.rs`, `locales/`)
- **Native `rust_i18n` Engine Integration**:
  - Embedded compile-time internationalization engine (`i18n!("locales", fallback = "en")`) supporting key-based translations (`t!(...)`).
  - Comprehensive UI text externalization across tabs, action buttons, tooltips, dialogs, status badges, contextual menus, and floating card narrator actions.
  - **System & Automation Commands Internationalization (`commands.*`, `roles.*`)**:
    - Externalized all element role descriptions (*button, link, input field, textarea, tab, text reader, checkbox, dropdown, slider, media*).
    - Localized spoken target narration, badge feedback, and search failure alerts.
    - Fully localized quick action suggestions (*"Disable dictation"*, *"Clear text"*, *"Close badges"*, *"Scroll down"*).
    - Localized execution feedbacks for undo actions, context memory clearing, scheduled task cancellations, and screen analysis prompts.
    - Replaced hardcoded multilingual helper routines (`localized_speech`) with direct compiled `t!(...)` calls.
    - **Theme Profiles Internationalization (`themes.*`)**: Fully translated theme display names and accessibility descriptions across the settings combo-box and live preview cards for all 9 supported languages.
  - Dynamic system language detection (`get_system_locale_language`) with precedence resolution: `.env` / `APP_LANG` variable ➔ Win32 `GetUserDefaultLocaleName` ➔ POSIX `LC_ALL` / `LC_MESSAGES` / `LANG`.
  - Real-time voice-driven language switching: speak commands like *"passer en anglais"*, *"switch to english"*, *"passer en français"*, *"auf deutsch"*, *"en español"*, *"in italiano"*, *"em português"*, *"по-русски"*, *"日本語"*, or *"中文"*.

### ⚙️ Dedicated Language Settings Tab (`main.rs`, `theme.rs`)
- **Interactive Language Configuration Modal (`SettingsTab::Language`)**:
  - Added a dedicated **Language** tab in the main settings modal alongside *LLM Provider* and *Theme*.
  - Visual language selector supporting 10 options:
    - `auto`: System Default (Auto-detect)
    - `fr`: 🇫🇷 Français
    - `en`: 🇬🇧 English
    - `de`: 🇩🇪 Deutsch
    - `es`: 🇪🇸 Español
    - `it`: 🇮🇹 Italiano
    - `pt`: 🇵🇹 Português
    - `ru`: 🇷🇺 Русский
    - `ja`: 🇯🇵 日本語
    - `zh`: 🇨🇳 中文
  - Accessibility-compliant: compatible with direct mouse clicks and dwell-pointer triggers (`apply_dwell_to_response`).
  - Instant locale re-indexing and persistence into `.env` (`APP_LANG=<locale>`).

### 🔔 Locales & Acoustic Feedback / Earcons (`audio.rs`, `main.rs`)
- **Refined Auditory Cues (Earcons)**:
  - Added and refined acoustic notifications for language changes, microphone state transitions, and setting commits.
  - Non-visual auditory feedback confirming language adjustments in real-time without visual focus interruption.

### 📖 Documentation & Localization Guides (`README.md`)
- **Updated README & Accessibility Guides**:
  - Documented multilingual voice controls and automatic locale detection.
  - Updated setup steps and feature descriptions to reflect cross-language voice accessibility and WCAG AAA compliance.

---
## [0.6.2] - 2026-10-31

### 🔍 Dynamic Adaptive Zoom & Multi-Viewport Synchronization (`main.rs`, `floating_window.rs`, `overlay.rs`)
- **Global Scale Propagation (`zoom_factor`)**:
  - Instant scale factor synchronization between the primary container and all secondary floating OS viewports (`vp_ctx.set_zoom_factor(ctx.zoom_factor())`).
  - Real-time scale integration in native GDI accessibility badge rendering (`draw_screen_numbered_badges`): proportional sizing of badges, fonts (`Segoe UI`), border strokes, and relative offsets.
  - Interactive font size controls in the *Themes & Accessibility* settings tab (`A -`, `A +`, `100 %`) fully compatible with physical click and dwell pointer, persisted into `.env` (`FONT_SIZE` / `ZOOM_FACTOR`).
  - Maintained voice-driven adaptive zoom commands (*"zoom in"*, *"zoom out"*, *"larger text"*, *"reset zoom"*).

### 🔢 Numbered Target Navigation & AABB Anti-Overlap Engine (`automation.rs`, `main.rs`)
- **Spatial Deduplication & Element Importance Scoring (`score_element_importance`)**:
  - Introduced Axis-Aligned Bounding Box (AABB `38 × 30 px`) collision detection to eliminate overlapping badges in dense UI layouts.
  - Weighted importance ranking: text input fields and textareas take precedence over buttons, hyperlinks, tabs, and static labels.
- **Direct Numpad & Digit Queue Ingestion (`NUMBER_KEY_QUEUE`)**:
  - Low-level system-wide hook interception of numpad digits (`0x60..=0x69`) and top row numeric keys (`0x30..=0x39`) via `switch_keyboard_hook_proc`.
  - Numeric composition buffer (`numbered_input_buffer`) allowing single or multi-digit target selection, confirmed via Enter/Space or auto-committed after a 900 ms inactivity delay.
  - Backspace support (`Backspace`) to erase the last entered digit without canceling or resetting the current switch navigation step.
  - Conservative target reconciliation (`reconcile_numbered_targets`) to preserve stable badge indices across background scans, scrolls, and UI updates.

### 🪟 Window Selection Stability & Navigation Fixes (`main.rs`, `automation.rs`, `window_util.rs`)
- **First Window Selection Fix (*Go to first window fix*)**:
  - Resolved index offsetting when entering switch access mode or stepping backward to window selection (`SwitchAccessStep::Windows`).
  - The first candidate window is now reliably selected, brought into the foreground (`force_foreground_window`, `restore_if_iconic`), and outlined with the high-visibility orange frame without skipping.
  - Stabilized Escape key and single-switch long-press traversal when stepping upward through the hierarchy (Element ➔ Zone ➔ Window) while maintaining target focus.

### 🌐 Cross-Platform Parity & Robustness (`automation.rs`, `commands.rs`)
- Aligned numbered element target structures and collision avoidance mechanisms across Windows (UI Automation), macOS (JXA/Accessibility), and Linux (X11 / Wayland / AT-SPI2).
- Streamlined background target scanner pipeline (`spawn_targets_worker`) to eliminate blank frames and visual flicker during window transitions.

---
## [0.6.1] - 2026-10-29

### 🚀 First-Launch Onboarding & Hover Navigation Detection (`main.rs`)
- **First-Run Interactive Setup (`HOVER_NAV_ASKED`)**:
  - Detects the first launch of the application and persists `HOVER_NAV_ASKED=true` in `.env`.
  - Welcomes the user with a prompt translated into their system language (*"Voulez-vous activer la navigation au survol ?"*, *"Do you want to enable hover navigation?"*, etc.) spoken aloud via TTS and displayed in chat.
  - Includes quick-action suggestion chips (*"Mode survol"* / *"Non merci"*) responsive to both click/dwell and localized voice commands (*"non merci"*, *"pas maintenant"*, etc.).

### ⏱️ Dwell Timer Disabling at 0 ms (`main.rs`)
- **Complete Dwell Deactivation (`DwellState::is_enabled`)**:
  - Added `is_enabled()` to `DwellState` to verify non-zero dwell durations.
  - When `dwell_duration == 0`, `update` and `get_progress` return immediately without triggering actions or drawing progress bars.
  - `apply_dwell_to_response` falls back directly to standard physical click interactions (`response.clicked()`).
  - The settings slider now ranges down to 0 ms with explicit formatting (*"Désactivé (0 ms)"*).

### 👁️ Fixed-Eye ("Sticky Gaze") Protection & TTS Lockout (`main.rs`, `floating_window.rs`)
- **Anti-Sticky Gaze Engine (`DwellState`)**:
  - Memorizes `last_triggered_id` to prevent looping triggers while the user's eye or cursor remains stationary on an element.
  - Requires pointer departure for at least 600 ms and a 1,500 ms minimum refractory interval before re-arming the same control.
- **Extended Lockout for Text Reading (`lock_id`)**:
  - Added `lock_id(&mut self, id, duration)` to `DwellState`.
  - Applies an immediate 6-second lock when reading chat entries, Twitch messages, or floating card *"🔊 Lire"* narrator buttons, eliminating speech repetition caused by ocular jitter or prolonged gaze.

### ✍️ Fixes for Direct Input & Dictation Mode (`commands.rs`, `automation.rs`, `main.rs`)
- **Resolved Truncation of Initial Characters in Dictation Mode (`Dictate`)**:
- **Stabilized Focus and Message Queues**: Increased the delay to 120 ms when restoring external focus (`restore_external_focus_target`), ensuring that target windows (Win32, Chromium, Electron, Java Swing, Notepad) have fully processed `WM_ACTIVATE` and `WM_SETFOCUS` before the first keystroke is sent.
- **Adjusted Character Injection Cadence**: Set the keystroke interval to 10 ms per Unicode character (`send_char_unicode`) and 25 ms for line breaks, preventing dropped keystrokes during high CPU load or when message queues are slow.
- **Automatic Deactivation of Windows Menu Mode (`VK_MENU`)**:
- Removed redundant Alt key trigger when the target window is already in the foreground (`GetForegroundWindow() == hwnd`).
- Immediately sends a `VK_ESCAPE` after bringing the window to the foreground to prevent the Windows menu bar from being inadvertently activated by the Alt key.
- **Comprehensive, Bidirectional Dictation Mode Announcements**:
- Added `enter_dictation_mode`: notifications are now displayed in the chat and spoken aloud via the narrator (TTS). Includes context-aware quick suggestions (e.g., *"Disable dictation"*, *"Clear text"*), restores focus to the host application, and resumes continuous listening once the announcement concludes. - Instantaneous deterministic voice-based detection via `is_start_dictation_command` (e.g., *"active la dictée"*, *"lance la dictée"*, *"mode dictée"*, *"start dictation"*, etc.).
- Synchronized triggering via the interface button ("click & dwell"), the trackball's auxiliary button (`XButton1`), and hardware switches (`SwitchKeyEvent::ToggleDictation`).

### ⌨️ Keyboard Interoperability & Remapping Tools (`automation.rs`)
- **Handling of Injected Events (PowerToys Keyboard Manager, AutoHotkey)**:
- Added filtering for the low-level flag `LLKHF_INJECTED` (`0x00000010`) within the global keyboard hook `switch_keyboard_hook_proc`.
- Keys and shortcuts remapped by third-party software are no longer intercepted or consumed by Libertide, allowing them to be passed directly to the active application and input fields.

### 🎨 Thematic Harmonization of Floating Windows (`floating_window.rs`, `main.rs`)
- **Dynamic Adoption of the Application's Active Theme**:
- `FloatingCard::show` and `render_markdown` now dynamically inherit the application's `ThemeColors` instead of relying on a fixed, dark neon palette.
- Comprehensive harmonization of modal backgrounds, borders, Markdown headers (H1, H2, H3), blockquotes, code blocks, list bullets, statistics badges, table grids, and form controls across all 7 accessibility themes (*Cyber ​​Cyan, Emerald, Amethyst, High-Contrast Yellow, High-Contrast White, Soft Amber, Colorblind-Friendly*). - Touch/mouse scrolling (`handle_drag_to_scroll`) and copy buttons are preserved, featuring the accent and contrast color schemes of the selected profile.

### 🔌 Hardware Event Queue & Switch Controls (`main.rs`)
- **Processing of Asynchronous Keyboard & Switch Events**:
- Immediate handling of signals from the `SWITCH_KEY_QUEUE` within the main loop:
- `ToggleDictation`: Toggles dictation on/off, with voice feedback.
- `ToggleSwitch`: Toggles hover mode and hierarchical switch control on/off.
- `ToggleVoice`: Cycles through available text-to-speech profiles.
- `Clear`: Performs a full reset of the chat history, clears the contextual analysis cache, and interrupts speech output with an audible notification.Leai
---
## [0.6.0] - 2026-10-28

### 🕹️ Hardware Ergonomics & Trackball Navigation (`main.rs`, `floating_window.rs`, `ui.rs`)
- **Native Support for Trackballs & Ergonomic Mice**:
- Direct interception and dedicated assignment of Windows/OS auxiliary buttons (`VK_XBUTTON1`, `VK_XBUTTON2`, `VK_MBUTTON`).
- **Scroll Wheel Button / Middle Click (`VK_MBUTTON`)**: Instantly interrupts ongoing speech synthesis without disrupting the focus of the host application.
- **Side Button 4 (`VK_XBUTTON1`)**: Toggles Dictation mode instantly, with automatic restoration of the previous external focus.
- **Side Button 5 (`VK_XBUTTON2`)**: Immediately enables/disables Hover mode and switch access (`switch_access_mode`).
- **Drag-to-Scroll (Touch/Gesture-based Scrolling)**:
- Fluid navigation via click-and-drag (`handle_drag_to_scroll`) integrated into all floating windows, data tables, code blocks, and Markdown views—eliminating the need to precisely target scroll bars.
- **Window Cycling Stability (`cycle_user_window`)**:
- Implements a fixed geometric sorting order for windows, preventing erratic jumping caused by Z-order reordering in Windows.

### 🔌 Plug-and-Play Switches & Switch Access (`automation.rs`, `main.rs`)
- **Dedicated Low-Level Keyboard Hook (`WH_KEYBOARD_LL`)**:
- Secure interception and consumption of switch-input keys, ensuring no events "leak" through to background applications. - Ready-to-use universal mapping:
- **Validation / Selection**: `Space`, `Enter`, Numpad `1` / `5`.
- **Next / Previous Navigation**: `Tab` / `Shift+Tab`, Arrow keys, Numpad `2` / `3` / `4` / `6`.
- **Back / Escape**: `Esc`, `Backspace`, Numpad `0`.
- **Dedicated hardware shortcuts**: `F7` (Hover toggle), `F8` / `Pause` (Dictation toggle), `F9` (Voice change), `F4` (Reset).
- **Single-Switch Hold Algorithm**:
- Automatic detection of short presses (selection/click) versus long presses (> 1,100 ms for backspace/exiting a level), enabling full control via a single physical switch.

### ⏱️ Advanced Dwell Pointer & Analyze/Send Button (`main.rs`, `floating_window.rs`)
- **Responsive Dwell Engine with progressive visual feedback (`DwellState`)**:
- Dwell time configurable via a slider in the settings, persisted in `.env` (`DWELL_TIME_MS`, ranging from 600 ms to 2,500 ms).
- Integrated visual progress indicator displayed beneath each button and tab (high-visibility, WCAG-compliant neon green).
- 500 ms anti-bounce cooldown period to prevent unintentional double-triggers.
- **Dual-Function Analyze/Send Button**:
- If the text field contains input: sends the command directly to the agent, then resets the interface and clears suggestions. - If the field is empty: automatically triggers a full semantic analysis of the active window.

### 🎨 Interface Redesign to WCAG 2.2 AAA Standards (`main.rs`, `theme.rs`)
- **WCAG 2.2 AAA-compliant touch targets (50 × 50 px)**:
- Standardized bottom action bar (`ACCESSIBILITY_BTN_SIZE = 50x50`): *Analyze*, *Microphone*, *Dictation*, *Hover*, *Voice*, *Settings*, and *Clear* buttons.
- High-contrast typography (22 pt icons, 9 pt sub-labels) featuring bold outlines and accent halos.
- **Enhanced Visual Accessibility Themes**:
- Interactive preview and seamless voice-controlled switching (e.g., *"Next theme"*, *"High contrast"*).
- High-visibility accent borders across the screen: a 5 px gold frame for the active window, a 3 px cyan halo for specific zones, and a 3 px orange border for the currently focused control.

### 📖 Enhanced Voice Narration for Floating Windows & Captures (`floating_window.rs`, `main.rs`)
- **Semantic speech generator for floating windows (`build_narrator_speech`)**:
- Comprehensive, structured oral transcription for all types of floating windows (Markdown, comparison tables, data sheets, forms).
- Automatic inventory of available interactions and actions, presented verbally.
- Instant voice-triggered commands (e.g., *"Read window"*, *"Read card"*, *"Read summary"*) with systematic quick-action suggestions (e.g., `🔊 Read: <Title>`). - **Synchronization and Clickable Multimedia Links**:
- Rendering of and direct clicking on `[link="action:..."]` macros embedded within text and table cells.
- Universal keyboard shortcuts (`Ctrl+A` / `Ctrl+C`) and context menus for copying cells, entire rows, or complete tables with a single click.

### 🧹 Architecture Standardization (`src/`)
- Standardization of automation and text-input interfaces across Windows, macOS (`AXUIElement`), and Linux (`AT-SPI2`).
- Robust action history management (`ActionHistoryManager`) with bidirectional Undo and Redo functionality.

---

## [0.5.6] - 2026-10-26

### 👁️ Multi-Platform Vision & OCR Engine (`ocr.rs`)
- **High-Performance Visual Capture & Recognition**:
  - **Windows**: Ultra-fast (< 3 ms) in-memory 32-bit RGBA frame capture via GDI (`BitBlt`, `GetDIBits`), paired with a cellular luminance gradient transition analysis algorithm (`detect_visual_boxes_from_pixels`) and semantic Win32 / UIA resolution (`resolve_visual_zone_content`).
  - **macOS**: Native **Apple Vision** integration (`VNRecognizeTextRequest`) paired with CoreGraphics capture (`CGWindowListCreateImage`), hardware-accelerated by the Apple Neural Engine (ANE) on Apple Silicon M1 to M4 chips (< 5 ms).
  - **Linux**: Pure-Rust lightweight OCR engine powered by `ocrs` and `rten` (`text-detection` and `text-recognition`), direct X11 capture (`XGetImage`), with seamless background downloading and local model caching in `~/.cache/libertide/models`.
  - **Orphan Text & Button Detection (`enrich_targets_with_ocr`)**:
    - Connected-component bounding box analysis with horizontal bridging for adjacent words on the same line.
    - Strict spatial deduplication: visual OCR targets never overlap controls already backed by a valid native accessibility node.

### 🎯 Hover Mode & Accessibility Ergonomics Integration (`automation.rs`, `ui.rs`)
- **Universal Interaction on Non-Accessible Interfaces**:
  - Automatic discovery and hover vocalization for controls in applications lacking native accessibility trees (HTML5/WebGL Canvas, Java Swing apps, custom ImGui/Qt interfaces, PDF viewers, or remote desktop sessions).
  - Automatic numbered badge assignment and activation shortcuts (`NumberedElementTarget`) for every visually recognized text element (*"visuel (OCR)"*, *"texte (Apple Vision)"*, *"texte (ocrs)"*).
- **Macro-Zone Text Reading Enrichment (`extract_text_from_zone`)**:
  - Integration of OCR-detected text blocks into the zone-reading command, ensuring full speech synthesis even when native UIA, AXUIElement, or AT-SPI2 text nodes are missing.
- **Switch Navigation & Rendering Robustness**:
  - Seamless integration into the switch/hover hierarchy (Window ➔ Zone ➔ Control/Visual Text).
  - Immediate incorporation of enriched visual targets into hover bounding box calculations and simulated click coordinates.

---

## [0.5.5] - 2026-10-24

### 🎯 Hover Mode & Accessibility Ergonomics
- **Physical Pointer Detection & Anti-Jitter Debounce (200 ms)**:
  - Continuous tracking of absolute cursor coordinates (`get_global_cursor_pos`) across Windows (Win32), macOS (CoreGraphics / `CGEventCreate`), and Linux (X11 / Wayland via `xdotool` / `ydotool`).
  - Anti-jitter algorithm: speech narration waits for 200 ms of verified cursor stillness (`hover_cursor_still_since`) after physical movement before speaking, eliminating auditory clutter during rapid mouse travel.
  - 180 ms speech preemption deadline (`hover_speech_deadline`) enabling instantaneous cancellation and replacement of speech when rapidly traversing across zones or controls.
  - **Adaptive Speech Hierarchy (`build_targeted_hover_speech`)**:
    - Detailed contextual voice announcements: enriched element label, semantic role (*button, link, input field, tab, media player, checkbox*), bound keyboard shortcuts, and relative index within parent macro-zone (*e.g., "1 of 5"*).
    - Automatic context deduplication: suppresses redundant parent macro-zone announcements when hovering consecutive controls within the same zone (`is_same_zone_as_last`).
    - Immediate vocalization of interactive states (*checked/unchecked, expanded/collapsed, typed values, slider levels*).
  - **Semantic Window Partitioning & Macro-Zones (`NavigationZone`)**:
    - Automatic window decomposition into 2 to 4 logical semantic blocks (*Top bar, Content body, Side panel*) analyzed by LLM (`analyze_navigation_zones_with_llm`) with instant geometric fallback (`build_fallback_navigation_zones`).
    - Contextual rewriting of cryptic or symbolic glyphs (*"✕"* ➔ *"Close window"*, *"⚙"* ➔ *"Sound settings"*, *"..."* ➔ *"More options"*).
    - Universal text extraction target (`inject_text_reader_target`) prepended to every zone: single click or Enter press copies the block's text to clipboard and speaks it aloud without manual precision pointing.
  - **High-Visibility Visual Feedback (WCAG AAA)**:
    - Direct on-screen drawing of high-contrast borders: vivid 5px orange boundary around the target window (`draw_screen_orange_border`), 3px cyan halo around active macro-zones, and 3px crisp highlight around the control under cursor (`draw_screen_control_border`).
    - Synchronized physical mouse cursor repositioning (`set_global_cursor_pos`) during switch/keyboard assisted navigation.
  - **Multimodal & Single-Switch Interaction**:
    - Natural multilingual voice activation/deactivation (*"mode survol"*, *"hover mode"*, *"switch mode"*, *"mode contacteur"*) alongside interactive HUD toggle button `🎯 Survol`.
    - Full hardware switch or keyboard navigation (`Space`, `Enter`, `Tab`, directional arrows, numpad `4`/`6`).
    - Contextual step-by-step upward traversal with `Escape` (Control ➔ Zone ➔ Window) without terminating the mode.
    - Automatic detection and target updates for popped menus and dropdown lists after click actions (`new_popped_targets`).

### 🚀 Cross-Platform Robustness & System Fixes
- **Cross-Platform Accessibility Parity (macOS / Linux / Windows)**:
  - Fixed `E0382` compiler errors (*use of moved value: `e.value`*) on macOS and Linux when extracting target `name` and `value` fields in `automation.rs`.
  - Standardized accessibility trees across Windows UI Automation, macOS `AXUIElement` (via JXA), and Linux AT-SPI2 (via Python introspector).

---

## [0.5.4] - 2026-10-22

### 🎯 Hover Mode & Hierarchical Switch Access
- **Pointer Detection & Anti-Jitter Debounce (200 ms)**:
  - Automatic physical pointer stillness detection to trigger vocal narration without auditory overload during rapid movements.
  - 180 ms announcement queue window with seamless preemption (`hover_speech_deadline`).
  - **Adaptive Speech Hierarchy (`build_targeted_hover_speech`)**:
    - Detailed control announcement: accessible label, semantic role, associated keyboard shortcut, and relative position (*e.g., "1 of 4"*).
    - Context deduplication: suppresses redundant parent macro-zone names when hovering within the same interaction block.
  - **AI-Assisted Semantic Zoning (`analyze_navigation_zones_with_llm`)**:
    - Automatic decomposition of complex windows into 2 to 4 logical macro-zones for streamlined navigation.
    - Contextual rewrites of cryptic, truncated, or symbolic labels (*"✕"* ➔ *"Close window"*, *"⚙"* ➔ *"Settings"*) tailored for screen readers and low-vision users.
  - **3-Step Hierarchical Navigation (`SwitchAccessStep`)**:
    - Cascading navigation flow: Window selection ➔ Zone/block selection ➔ Individual control interaction.
    - Dynamic discovery of popped menus, context menus, and dropdown controls post-interaction (`new_popped_targets`).
  - **High-Visibility Visual Feedback**:
    - Bold high-visibility orange borders around target windows and active controls (`draw_screen_orange_border`).
    - Pulsing cyan halo outlining active macro-zones.
  - **Multimodal Control & Universal Shortcuts**:
    - Voice control activation and deactivation (*"hover mode"*, *"switch access"*, *"switch mode"*, *"mode survol"*).
    - Full hardware switch and keyboard support (`Space`, `Enter`, `Tab`, arrow keys, numpad `4`/`6`).
    - Contextual `Escape` key for smooth step-by-step upward traversal through the navigation hierarchy.
    - Interactive `🎯 Survol` HUD toggle button with real-time target status badge.

---

## [0.5.3] - 2026-10-21

### 🚀 Cross-Platform Automation Parity
- **Linux Accessibility (AT-SPI2)**: Implemented full support for UI element discovery and interaction on Linux desktops (X11/Wayland) via the AT-SPI2 accessibility bridge. This enables `click_element`, `list_interactive_elements`, and semantic element finding (`find_best_button`, etc.) on Linux.
- **macOS Automation Enhancements**: Added native `osascript` macros for simulated clicks, keyboard input, and system commands, significantly improving automation reliability on macOS.
- **Unified Command Execution**: The `execute_system_actions` function in `commands.rs` is now fully implemented for non-Windows platforms, translating abstract actions like `ClickButton` into native `xdotool` or `osascript` calls.

### 🧭 Navigation & Command Refinements
- **Disambiguated Search**: The agent now correctly distinguishes between a system-wide search (`open_search` for apps/files) and a web search (`open_search_browser` for the browser's omnibox).
- **Browser Navigation**: Added dedicated `navigate_back` and `navigate_forward` actions for seamless history navigation in web browsers.
- **Direct CLI Execution**: The direct command execution feature (prefix `>`) is now enabled on all platforms (Windows, macOS, Linux).

### 🛠️ Build & CI/CD
- **Comprehensive Pull Request Validation**: GitHub Actions workflows are now configured to run tests and builds on pull requests targeting *any* branch, ensuring code quality before merges.
- **Release Safety**: The release creation step in the CI pipeline is now protected to only run on tag pushes, preventing accidental releases from pull requests.

### 🐛 Bug Fixes & Stability
- **Test Suite Stabilization**: Addressed and fixed multiple unit test failures across Windows, Linux, and macOS environments, ensuring a more stable and reliable build process.
- **Duplicate Import Fix**: Resolved `E0252` compilation errors caused by duplicate `use` statements in `src/ui.rs`, cleaning up the module's namespace.

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