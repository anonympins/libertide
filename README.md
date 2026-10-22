
# <img src="public/favicon-256x256.png" width="64" alt="Libertide Overlay Interface" valign="middle"> Libertide
> **Empowering everyone to use their computer with complete freedom and autonomy.**  
> Libertide is a universal assistive overlay (voice, gaze, and switch access) designed to overcome motor, visual, or cognitive challenges without requiring expensive specialized hardware.

[![Version](https://img.shields.io/github/v/release/anonympins/libertide?label=Version)](https://github.com/anonympins/libertide/releases)
[![License](https://img.shields.io/github/license/anonympins/libertide?label=License)](https://github.com/anonympins/libertide/blob/main/LICENSE)
[![WCAG Accessibility](https://img.shields.io/badge/Accessibility-WCAG%20AAA-brightgreen)](#-visual-comfort--high-visibility-wcag-aaa)
[![Platforms](https://img.shields.io/badge/Platforms-Windows%20%7C%20macOS%20%7C%20Linux-blue)](#-getting-started-in-3-steps)

Windows : [![https://github.com/anonympins/fingerprint/actions/workflows/rust.yml](https://img.shields.io/github/actions/workflow/status/anonympins/libertide/rust.yml)](https://github.com/anonympins/libertide/actions/workflows/rust.yml) / UNIX : [![https://github.com/anonympins/libertide/actions/workflows/rust-unix.yml](https://img.shields.io/github/actions/workflow/status/anonympins/libertide/rust-unix.yml)](https://github.com/anonympins/libertide/actions/workflows/rust-unix.yml) / macOS : [![https://github.com/anonympins/libertide/actions/workflows/rust-macos.yml](https://img.shields.io/github/actions/workflow/status/anonympins/libertide/rust-macos.yml)](https://github.com/anonympins/libertide/actions/workflows/rust-macos.yml)

<p align="center">
  <img src="public/screenshot-1.png" alt="Libertide transparent interface" width="360">
  <img src="public/screenshot-2.png" alt="Visual guidance and assisted hover" width="360">
</p>

---

## 💡 Why Libertide? (The Autonomy Challenge)

Modern operating systems and websites require **surgical precision**: fast double-clicks, targeting tiny close buttons, dismissing invasive cookie banners, and holding complex key combinations.

For millions of people, these ordinary interactions become an exhausting hurdle, causing pain or digital exclusion:

- **Tremors and motor impairments** (*Parkinson's disease, essential tremor, ataxia*): clicking small targets triggers accidental clicks or frustrating misses.
- **Severely limited mobility or paralysis** (*Tetraplegia, ALS / Lou Gehrig's disease, muscular dystrophies*): standard mouse or keyboard usage is difficult or impossible.
- **Chronic pain and temporary injuries** (*RSI, acute tendonitis, carpal tunnel syndrome, cast on a limb*): every mouse movement exacerbates discomfort.
- **Low vision and eye fatigue** (*AMD, low vision, light sensitivity*): difficulty locating the cursor or reading low-contrast gray text.
- **Cognitive overload and attention disorders**: difficulty navigating pages cluttered with ads and intricate dropdown menus.

> **The Libertide promise:** You no longer need to adapt to the computer. The computer adapts to your abilities, your rhythm, and your movements.

---

## 🛡️ The Solution: Compassionate Technology

Libertide runs as a **transparent, unobtrusive assistive overlay** displayed above your existing desktop apps (Word, web browsers, emails, file explorers). It combines several core innovations:

### 1. 🎯 Stabilized Hover Mode (Anti-Tremor)
No need to aim with precision or press mouse buttons forcefully:
- **Smart stabilization**: simply move the cursor near the desired zone. As soon as the pointer pauses briefly (200 ms), Libertide identifies the button or text and reads it aloud clearly.
- **One-click universal read-out**: a single tap on `Enter` or a click anywhere inside a paragraph reads the entire block aloud.
- **High-visibility framing**: active windows and hovered targets are highlighted with contrasting borders (orange and cyan) for instant recognition.

### 2. 🔘 Single-Switch Navigation (Switch Access)
For users who can only produce a single physical movement (muscle twitch, sip-and-puff, foot switch, eye blink, or single key press):
- **Simplified cascade navigation**: Choose Window ➔ Choose Region ➔ Trigger Action on Element.
- **Hardware agnostic**: works seamlessly with the `Space` bar, dedicated USB/Bluetooth switches, or numeric keypads.

### 3. 🔢 Click by Number
- Simply say *"Numbers"* or *"Badges"*: large golden numbers appear on all interactive links and buttons on screen.
- Speak the target number (*"1"*, *"5"*, *"twelve"*): Libertide clicks it instantly on your behalf.

### 4. 🎙️ Continuous Dictation & Natural Voice Commands
- **Local continuous dictation**: transcribe letters or messages without touching the keyboard using high-fidelity local speech-to-text (Whisper), keeping your privacy fully intact.
- **Natural language understanding**: talk naturally like you would to an assistant (*"Summarize this page"*, *"Open my emails"*, *"Remind me to take my medicine in 20 minutes"*).

### 5. 🛡️ Removal of Web Obstacles & Complete Safety
- **Farewell to intrusive overlays**: Libertide automatically dismisses cookie prompts and newsletter modals blocking your screen.
- **Ultra-secure transactions**: the assistant can prepare shopping carts or search for train tickets, but **strictly halts before any payment step** to require your explicit manual confirmation.

### 6. 🎨 Visual Comfort & High Visibility (WCAG AAA)
The interface adapts dynamically to your eyesight:
- **Continuous voice zoom**: say *"Zoom in"* or *"Enlarge"* to scale the entire interface up to 250%.
- **Integrated accessibility themes**:
  - *High-Visibility Yellow & Black* (optimized for low vision and AMD).
  - *Soft Amber* (maximum blue-light reduction to soothe eye fatigue and headaches).
  - *Color-blindness Friendly* (cobalt blue and gold high contrast).
  - *Mint Emerald & Amethyst* for calming, restful reading.
- **Audio cues (Earcons)**: gentle sound cues confirm application states without requiring you to look at the screen.

---

## 📖 Everyday Use Cases (Real-World Examples)

### ✉️ 1. Drafting an email or document without strain
1. Open your mail client or word processor.
2. Say *"Dictation mode"* (or click `✍ Dictation`).
3. Dictate your thoughts naturally:
   - *"Hello Doctor comma new line"*
   - *"I confirm our appointment next Tuesday period"*
4. Say *"Stop dictation"* or press `Escape`.

### 🌍 2. Browsing the web without struggling against the screen
- **Read without eye strain**: *"Summarize this article"*  
  ➔ Libertide inspects the web page and displays a clear summary card in large, readable typography.
- **Booking trips or transport**: *"Find train tickets to Lyon for Saturday morning"*  
  ➔ A simplified booking card appears with large, accessible sliders to pick schedules peacefully.
- **Explore events and activities**: *"What festivals are happening near me this summer?"*

### 🖱️ 3. Clicking without shaking
- **With gaze or mouse**: turn on `🎯 Hover Mode`. Point at the target area: Libertide reads out the button (e.g., *"Close window"*, *"Print"*) and waits for your confirmation, ensuring tremors never trigger misclicks.
- **With voice**: say *"Badges"*, then *"Click 4"*.

### 🪟 4. Organizing your workspace
- *"Put the browser on the left and my notes on the right"*  
  ➔ Windows snap edge-to-edge automatically.
- *"Full screen"* / *"Close this tab"*.
- *"Focus mode"* ➔ Creates a clean, distraction-free virtual desktop so you can focus on a single task.

### ⏱️ 5. Reminders and daily assistance
- *"Remind me to take my medication in 30 minutes"*
- *"Mute sound"* / *"Unmute sound"*
- *"Zoom in"* / *"Normal zoom"*

---

## ⌨️ Essential Shortcuts

| Shortcut / Key | Action |
| :--- | :--- |
| **`R` key** | Starts or stops instant speech dictation. |
| **`Spacebar`** | Activates the currently highlighted element in **Switch Access** mode. |
| **`Enter` key** | Confirms a command or triggers a click on the selected element. |
| **`Tab` / Arrows / Numpad `4`-`6`** | Moves selection between regions or items. |
| **`Escape` key** | **Safety & Emergency stop**: aborts any action immediately, silences text-to-speech, or navigates back one level. |
| **`🎯 Hover` button** | Toggles stabilized anti-tremor hover mode on or off. |

---

## 🚀 Getting Started in 3 Steps

### Step 1: Download Libertide
Download the binary built for your OS from the **Releases** page:
- **Windows** (`.exe`): Windows 10 and 11.
- **macOS**: Apple Silicon (M1/M2/M3/M4) or Intel.
- **Linux**: Modern distributions (Ubuntu, Debian, Fedora, Arch...).

### Step 2: Plug in your existing devices
Libertide works out of the box with the peripherals you already own:
- Built-in microphone or a standard headset.
- Standard speakers or headphones.
- Mouse, trackball, trackpad, or external adaptive switch.

### Step 3: Configure your AI provider
To process natural language commands, Libertide connects to your preferred AI provider (DeepSeek, Mistral AI, OpenAI, Claude, Groq...):
1. Launch the application.
2. Click the **🔑 API Key** icon in the top bar (or simply say *"API Key"*).
3. Paste your key: Libertide is ready to assist you right away!

*(During your very first voice recognition session, the lightweight Whisper model will download automatically in the background).*

---

## 🤝 For Caregivers, Families, and Occupational Therapists

If you are assisting someone with diminishing autonomy, Libertide provides vital design guarantees:

- **Zero vendor lock-in**: users keep their favorite software, web browsers, and routines. Libertide doesn't replace their environment; it makes it accessible.
- **Adapts over time**: start with simple stabilized hover, introduce voice dictation, and transition to single-switch access whenever condition changes dictate.
- **Zero proprietary subscriptions required**: Libertide is completely free and open-source.
- **Safety first**: pressing `Escape` instantly returns full manual control to the user or caregiver.

---

## 🗣️ Core Voice Commands Guide

Libertide understands English, French, and most modern languages:

| Category | What you can say out loud |
| :--- | :--- |
| **Interface Control** | *"Hide"* • *"Show up"* • *"Emergency stop"* • *"Stop"* |
| **Pointing & Clicking Help** | *"Hover mode"* • *"Numbers"* / *"Badges"* • *"Click 3"* • *"Switch mode"* |
| **Display & Sizing** | *"Zoom in"* • *"Zoom out"* • *"Reset zoom"* • *"High contrast"* |
| **Dictation & Writing** | *"Dictation mode"* • *"New line"* • *"Delete word"* • *"Stop dictation"* |
| **Reading & Summaries** | *"Summarize this page"* • *"What is this video about?"* • *"Read this text"* |
| **Workspace & Comfort** | *"Tile windows side by side"* • *"Mute sound"* • *"Close tab"* |
| **Reminders & Timers** | *"Remind me to take my medicine in 20 minutes"* |

---

## ❓ Frequently Asked Questions (FAQ)

**Does Libertide work without expensive assistive hardware?**  
Yes, absolutely. That is the core mission: a standard webcam, any basic microphone, and a regular mouse or keyboard spacebar are all you need.

**Do my voice and private data remain private?**  
Speech recognition for continuous dictation runs directly on your local machine via Whisper. Your speech is not sent to external servers for continuous transcription.

**Can I use Libertide at school or at work?**  
Yes. Libertide acts as a non-intrusive transparent desktop overlay without modifying core operating system files or requiring elevated administrative rights for daily tasks.

---

<p align="center">
  <b>Libertide — Crafted with passion for autonomy and digital inclusion for all.</b><br>
  Distributed under a free, open-source license.
</p>