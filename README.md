# bla

A local-first, system-wide voice dictation app for macOS. Hold a hotkey, speak naturally, and cleaned, polished text appears wherever your cursor is — or gets appended to a Markdown file — all speech-to-text and text cleanup runs on-device, with nothing ever sent to the cloud.

## Privacy

bla is built around a strict on-device guarantee:

- **Audio, transcripts, and any derived text never leave your machine.** Speech-to-text and cleanup both run locally.
- **No telemetry, no analytics, no crash reporting** to any external service.
- The **only** network activity the app ever performs is:
  1. A **one-time download** of a Whisper speech-to-text model from Hugging Face, triggered by you on first run.
  2. Optional calls to a **local Ollama instance** (`localhost:11434`) for higher-quality AI text cleanup, if you have Ollama installed and running. If Ollama isn't reachable, bla falls back to a rule-based cleanup pass — no error, no interruption.

History, personal dictionary, and settings are stored locally on your machine and are never uploaded anywhere.

## Requirements

- **macOS** (primary supported platform; Windows build is compile-verified but not yet a supported target)
- **Rust** (stable toolchain, via `rustup`)
- **Node 20+** and **pnpm**
- Xcode Command Line Tools (for native builds on macOS)
- Optional: [Ollama](https://ollama.com) running locally for the AI cleanup pass — bla works fine without it, using rule-based cleanup instead

## Install / build from source

```bash
# Install dependencies
pnpm install

# Run in development mode
pnpm tauri dev

# Build a packaged app
pnpm tauri build
```

On first run, macOS will prompt you to grant:

- **Microphone** access — required to capture your speech.
- **Accessibility** access — required so bla can paste cleaned text into the focused app via synthetic keystrokes.

You'll also be prompted to download a Whisper model on first launch (one-time, from Hugging Face).

## Usage

**Push-to-talk:**

1. Hold the configured hotkey.
2. Speak naturally.
3. Release the hotkey.

Your speech is transcribed on-device, cleaned up (filler words removed, punctuation and formatting applied), and delivered to its destination in under a couple of seconds for a typical utterance.

**Output modes:**

- **Cursor-paste mode** (default): the cleaned text is pasted directly at your cursor position in whatever app is currently focused — notes, email, chat, browser forms, anything.
- **File-output mode**: instead of pasting, the cleaned text is appended to a Markdown file, regardless of which app has focus. This is designed for a frictionless "dictate straight into today's note" workflow — e.g. an Obsidian daily note. The output path supports templating like `{{date:YYYY-MM-DD}}`, so each day's dictation lands in the right daily-note file automatically, with optional timestamps per entry, creating the file if it doesn't already exist.

## Status

bla is under active development. The current milestone (**M1 — MVP**) covers the core dictation pipeline: push-to-talk capture, on-device transcription, pluggable cleanup, and the two output modes described above. UI polish (recording pill, full settings window), history/dictionary, command mode, and packaged releases are tracked in later milestones.

See [ROADMAP.md](./ROADMAP.md) for the full milestone breakdown and current status.

<!--
Screenshot / GIF of the recording pill and settings window: coming in a later milestone (M2/M5),
once the UI shell exists. Not included yet so as not to show a mockup that doesn't reflect the real app.
-->

*Screenshots and a demo GIF are coming once the UI shell (recording pill, settings window) lands.*

## Contributing

Contributions are welcome. See [CONTRIBUTING.md](./CONTRIBUTING.md) for the workflow, testing requirements, and review process.

## License

MIT — see [LICENSE](./LICENSE).
