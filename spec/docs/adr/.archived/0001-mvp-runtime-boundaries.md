# ADR 0001: MVP runtime boundaries

- Status: Accepted
- Date: 2026-08-26

## Decision

SAAA MVP 0 uses these runtime boundaries:

- Desktop shell: Tauri 2 with React and TypeScript.
- Persistence: SAAA-owned SQLite through `rusqlite` with the bundled SQLite library.
- Model provider: OpenAI-compatible Chat Completions over HTTPS, or HTTP only for loopback local endpoints.
- Codex: a Tauri-managed Codex app-server child process. The native executable installed transitively with the exact `@openai/codex-sdk` version is staged into the desktop bundle; `SAAA_CODEX_PATH`, the development dependency path, and a globally installed `codex` are fallbacks.
- Microphone: browser `getUserMedia` and Web Audio capture in the Tauri webview.
- STT: local `whisper.cpp` CLI selected with `SAAA_WHISPER_PATH`; the model is an explicit local filesystem setting and is never downloaded implicitly.
- TTS: the local operating-system speech process (`say`, `espeak-ng`, or Windows SpeechSynthesizer).

## Safety and lifecycle

Codex threads always use a read-only sandbox, approval policy `never`, disabled web search, and no configured write-capable MCP servers. SAAA stores only the Codex thread id and bounded redacted activity. Child processes are interrupted and terminated on cancel or command completion.

Voice capture never leaves the webview-to-Tauri IPC boundary. STT accepts PCM samples and executes a local process. Cloud fallback is not implemented. Recording, transcription, generation, Codex, and TTS each have an explicit stop path.

## Packaging

The package includes the target platform's pinned Codex executable but can start without Codex authentication, whisper, or a model file. Missing optional runtimes degrade only their corresponding feature and are reported with recovery instructions. The application has no Hono, RAG, PostgreSQL, pgvector, or embedding runtime dependency.

SQLite uses `rusqlite` with bundled SQLite. SAAA creates a consistent user-requested backup and automatically backs up an older database before migration. The database file, its backups, and redacted diagnostics are owned by SAAA's OS application-data directory; user-selected whisper models and workspaces remain outside that ownership boundary.

## UI inventory

The MVP reuses only layout and interaction concepts from the referenced UI: sidebar navigation, conversation list, route banner, message surface, composer, settings section menu, cards, status badges, and sticky save controls. No Hono client or RAG data contract is imported.
