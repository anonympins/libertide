# Libertide

[![https://github.com/anonympins/libertide/releases/tag/libertide-0.5.0](https://img.shields.io/github/v/release/anonympins/libertide)](https://github.com/anonympins/libertide/releases)
[![https://raw.githubusercontent.com/anonympins/libertide/refs/heads/main/LICENSE](https://img.shields.io/github/license/anonympins/libertide)](https://github.com/anonympins/libertide/blob/main/LICENSE)
[![https://github.com/anonympins/libertide](https://img.shields.io/github/repo-size/anonympins/libertide)](https://github.com/anonympins/libertide)

Libertide is a voice and visual assistant for autonomous exploration on Windows and Unix / Linux (X11). It combines a discreet transparent overlay interface (developed with [egui](https://github.com/emilk/egui)), continuous local voice recognition with **Whisper**, reasoning from the **DeepSeek-Chat** model, as well as graphical interface automation (Windows UI Automation & X11 EWMH tools, window tiling, web navigation, and input).


---

## 📋 Prerequisites

1. **Operating System**:
   - **Windows**: Windows 10 / 11 (64-bit).
   - **Unix / Linux**: Any modern distribution running an X11 / Xorg desktop session (Ubuntu, Debian, Fedora, Arch Linux, etc.).
2. **Rust & Cargo**: Recent stable version (1.75+ recommended).  
   Install it via [rustup.rs](https://rustup.rs/).
3. **C/C++ Build Tools & Native Dependencies**:
   - **Windows**: Visual Studio C++ Build Tools (required for `whisper.cpp`).
     Install automatically in an administrator PowerShell terminal:
   ```powershell
   # Via winget:
   winget install --id Microsoft.VisualStudio.2022.BuildTools -e --silent --override "--wait --quiet --norestart --nocache --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"

   # Or via direct bootstrapper download:
   Invoke-WebRequest -Uri "https://aka.ms/vs/17/release/vs_buildtools.exe" -OutFile "$env:TEMP\vs_buildtools.exe"; Start-Process -FilePath "$env:TEMP\vs_buildtools.exe" -ArgumentList "--quiet", "--wait", "--norestart", "--nocache", "--add", "Microsoft.VisualStudio.Workload.VCTools", "--includeRecommended" -Wait
   ```
   - **Unix / Linux**: C/C++ toolchain, CMake, ALSA, X11 libraries, and automation utilities (`xdotool`, `wmctrl`, `xclip`, `speech-dispatcher` or `espeak-ng`):
     ```bash
     # Debian / Ubuntu / Linux Mint:
     sudo apt update && sudo apt install -y build-essential cmake pkg-config \
         libasound2-dev libx11-dev xdotool wmctrl xclip speech-dispatcher espeak-ng

     # Fedora / RHEL:
     sudo dnf install -y gcc gcc-c++ cmake alsa-lib-devel libX11-devel xdotool wmctrl xclip speech-dispatcher espeak-ng

     # Arch Linux:
     sudo pacman -S --needed base-devel cmake alsa-lib libx11 xdotool wmctrl xclip speech-dispatcher espeak-ng
     ```
4. **Microphone and speakers**: For Whisper dictation and voice feedback (via Windows SAPI or Unix `spd-say` / `espeak-ng`).
5. **DeepSeek API Key**: Obtain it from the DeepSeek Platform.

---

## ⚙️ Configuration (`.env`)

Create a `.env` file at the project root (`.env`).

### Example `.env` configuration

```ini
# DeepSeek API Key (required for agent reasoning)
DEEPSEEK_API_KEY=sk-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx

# Twitch channel to automatically join in read-only mode (optional)
TWITCH_CHANNEL=

# Custom Whisper model location (optional, see Whisper section)
# WHISPER_MODEL_PATH=models/ggml-base.bin
```

> **Note**: You can also set `DEEPSEEK_API_KEY` as a global environment variable in your Windows terminal (`set DEEPSEEK_API_KEY=...` or `$env:DEEPSEEK_API_KEY="..."`).

---

## 🎙️ Whisper Voice Model

On the first launch of voice listening:
- Libertide checks for the presence of the `models/ggml-base.bin` or `models/ggml-tiny.bin` model.
- If absent, the application automatically downloads it from Hugging Face (approximately 148 MB) into the `models/` subfolder.

---

## 🚀 Building and Running

### 1. Direct development command

To compile and immediately launch the project in debug mode:

```bash
cargo run
```

### 2. Optimized build (Release)

For better graphical performance and reduced inference latency:

```bash
cargo build --release
```

The compiled executable will be available at:
```text
target/release/libertide.exe
```

### 3. Ready-to-use command (PowerShell)

To quickly create the `.env` file if it doesn't already exist, compile, and launch the application in a single line:

```powershell
if (-not (Test-Path .env)) { Set-Content .env "DEEPSEEK_API_KEY=your_key_here`nTWITCH_CHANNEL=" }; cargo run --release
```

---

## ⌨️ Usage and Shortcuts

| Shortcut / Action | Description |
| :--- | :--- |
| **`R` key** | Starts or stops instant Whisper voice capture. |
| **`Escape` key** | **Emergency stop**: exits dictation mode or halts any ongoing action, mutes TTS, and immediately gives control back to the user. |
| **Enter** | Sends the instruction entered in the text field. |
| **Dragging** | Click and drag the top banner or the bottom bar to anchor the overlay wherever you want. |
| **Borders & Corners** | Resize the overlay by stretching the edges and corners. |
| **Assistance tab** | Tracks the dialogue with DeepSeek, automated actions, and clickable quick suggestions. |
| **Chat tab** | Connection and monitoring of a Twitch channel with an integrated search bar. |

---

## 🗣️ Commands Understood by the AI (Human-Friendly Guide)

Libertide understands natural language instructions spoken aloud (via Whisper) or typed into the prompt bar. Below are the primary categories of commands you can give the assistant:

### 1. Overlay Control & Emergency Stop
- **Hide overlay**: *"Hide yourself"* / *"Cache-toi"*, *"Close overlay"* / *"Ferme l'overlay"*, *"Disappear"*
- **Restore overlay**: *"DeepSeek open up"* / *"DeepSeek ouvre-toi"*, *"Show yourself"* / *"Montre-toi"*
- **Immediate emergency stop**: Press **`Escape`** or say *"Stop everything"*, *"Stop"*

### 2. ✍️ Direct Voice Dictation Mode
Switches directly to text input inside whatever window currently has focus (text editor, IDE, form) without triggering exploratory web or system actions:
- **Enable**: *"Dictation mode"* / *"Mode dictée"*, *"Take notes on what I say"* (or click the `✍ Dictée` button).
- **Stop**: Press **`Escape`**, or say *"Stop dictation"* / *"Arrête la dictée"*, *"Exit dictation"*, *"I'm done"*.
- **Formatting & Caret Navigation while dictating**:
  - *"New line"* / *"À la ligne"*, *"Next line"*, *"New paragraph"*
  - *"Cursor left"* / *"Curseur à gauche"*, *"Cursor right"*, *"Up"*, *"Down"*
  - *"Start of line"* / *"Début de ligne"*, *"End of line"*, *"Start of document"*, *"End of document"*
  - *"Delete word"* / *"Efface le mot"*, *"Delete line"* / *"Efface la ligne"*, *"Select all"* / *"Tout sélectionner"*

### 3. 🌐 Web Browsing, Search & Shopping
- **Search / Navigation**:
  - *"Open Google and search for the best noise cancelling headphones"*
  - *"Go to wikipedia.org"*
  - *"Open youtube.com in a new tab"*
- **Interactions & Obstacles**:
  - *"Click 'Accept all'"* (the agent detects and clicks cookie consent dialogs and GDPR banners automatically)
  - *"Click the first result"*
  - *"Scroll down"* / *"Fais défiler vers le bas"*, *"Scroll to the top"*
- **Assisted Shopping**:
  - *"Add the item to the cart and proceed to checkout"* (Libertide will automatically stop before payment and ask for spoken confirmation).

### 4. 🪟 Window Management & Layouts
- **Snap & Tiling Layouts**:
  - *"Put Chrome on the left and VS Code on the right"*
  - *"Tile my windows"* / *"Arrange in a 2x2 grid"*
  - *"Fullscreen"* / *"Maximize this window"*, *"Minimize window"*
  - *"Close the tab"* (closes only the current active tab without terminating the whole application)

### 5. 💻 Code, File & Terminal Inspection
- **Inspect & Summarize code or documents**:
  - *"Read the file opened in VS Code and explain what the main function does"*
  - *"Analyze what is inside my terminal"*
  - *"Select between 'Hello' and 'Best regards'"* (bounded text selection)

### 6. 🧘 Immersion Mode (Virtual Desktop)
Creates a clean, dedicated Windows virtual desktop configured for focused deep work:
- **Start**: *"Activate immersion mode to code"* (spawns a new virtual desktop, opens required applications such as VS Code, a terminal, and docs tiled together)
- **Exit**: *"Exit immersion mode"* / *"Close the virtual desktop"*

### 7. ⚙️ Windows Startup (Autostart)
- **Enable auto-launch**: *"Start when Windows boots"*, *"Enable autostart"*
- **Disable auto-launch**: *"Do not start on boot"*, *"Disable autostart"*
- **Check status**: *"Are you configured to start on boot?"*

### 8. ⚡ Direct CLI Execution
Prefix your prompt with `>` or `$` to instantly run a system command without invoking the LLM:
```bash
> code .
> calc.exe
$ git status
```

---

## 🛠️ Troubleshooting

- **Error `DEEPSEEK_API_KEY in the .env file`**: Verify that the `.env` file is correctly placed at the project root without a hidden `.txt` extension.
- **C++ / Whisper compilation error**: Make sure you have launched Cargo from the *x64 Native Tools Command Prompt for VS* terminal or that the MSVC compiler is accessible in your `PATH`.
- **Microphone inactive**: Verify that microphone access is authorized for desktop applications in Windows privacy settings.