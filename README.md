# Libertide

Libertide is a voice and visual assistant for autonomous exploration on Windows. It combines a discreet transparent overlay interface (developed with [egui](https://github.com/emilk/egui)), continuous local voice recognition with **Whisper**, reasoning from the **DeepSeek-Chat** model, as well as advanced graphical interface automation (Windows UI Automation, windowing, web navigation, and input).

---

## 📋 Prerequisites

1. **Operating System**: Windows 10 / 11 (64-bit).
2. **Rust & Cargo**: Recent stable version (1.75+ recommended).  
   Install it via [rustup.rs](https://rustup.rs/).
3. **Visual Studio C++ Build Tools**: Required to compile native C/C++ dependencies (notably `whisper-rs` / `whisper.cpp`).  
   Install automatically in an administrator PowerShell terminal:
   ```powershell
   # Via winget:
   winget install --id Microsoft.VisualStudio.2022.BuildTools -e --silent --override "--wait --quiet --norestart --nocache --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"

   # Or via direct bootstrapper download:
   Invoke-WebRequest -Uri "https://aka.ms/vs/17/release/vs_buildtools.exe" -OutFile "$env:TEMP\vs_buildtools.exe"; Start-Process -FilePath "$env:TEMP\vs_buildtools.exe" -ArgumentList "--quiet", "--wait", "--norestart", "--nocache", "--add", "Microsoft.VisualStudio.Workload.VCTools", "--includeRecommended" -Wait
   ```
4. **Microphone and speakers**: For Whisper dictation and voice feedback via Windows speech synthesis (SAPI).
5. **DeepSeek API Key**: Obtain it from the DeepSeek Platform.

---

## ⚙️ Configuration (`.env`)

Create a `.env` file at the project root (`C:/Dev/libertide-new/.env`).

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
| **`Escape` key** | **Emergency stop**: halts any ongoing action, cuts the voice, and returns control to the user. |
| **Enter** | Sends the instruction entered in the text field. |
| **Dragging** | Click and drag the top banner or the bottom bar to anchor the overlay wherever you want. |
| **Borders & Corners** | Resize the overlay by stretching the edges and corners. |
| **Assistance tab** | Tracks the dialogue with DeepSeek, automated actions, and clickable quick suggestions. |
| **Chat tab** | Connection and monitoring of a Twitch channel with an integrated search bar. |

### Useful voice commands
- **Hide the overlay**: *"hide yourself"*, *"close the overlay"*
- **Show the overlay again**: *"show yourself"*, *"deepseek open up"*
- **Direct CLI commands**: Prefix with `>` or `$` to immediately execute a system command (e.g., `> code .`).

---

## 🛠️ Troubleshooting

- **Error `DEEPSEEK_API_KEY in the .env file`**: Verify that the `.env` file is correctly placed at the project root without a hidden `.txt` extension.
- **C++ / Whisper compilation error**: Make sure you have launched Cargo from the *x64 Native Tools Command Prompt for VS* terminal or that the MSVC compiler is accessible in your `PATH`.
- **Microphone inactive**: Verify that microphone access is authorized for desktop applications in Windows privacy settings.