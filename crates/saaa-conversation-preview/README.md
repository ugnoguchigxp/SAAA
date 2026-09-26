# Conversation preview (I0/I1)

This is an isolated Tauri entry point for the response runtime. It reads the
saved model and TTS selection from a SQLite Online Backup of the current SAAA
database. It does not start the legacy SAAA background workers.

On macOS, make a private directory and run:

```sh
preview_dir=$(mktemp -d /tmp/saaa-conversation-preview.XXXXXX)
chmod 700 "$preview_dir"
SAAA_MVP2X_APP_DATA_DIR="$preview_dir" \
SAAA_PREVIEW_SOURCE_DB="$HOME/Library/Application Support/com.saaa.desktop/saaa.sqlite3" \
cargo run --manifest-path crates/saaa-conversation-preview/Cargo.toml
```

The source DB is opened read-only. The preview writes only inside the new
directory and does not reuse a copied LARM lease. It uses the saved LARM
credential and selected TTS provider. The `Open` button starts the single
preview session; `Close` stops it and releases its LARM connection. Reopening
the app with the same directory retains the isolated evidence and marks any
unfinished prior response as interrupted.

The fixture and isolation tests run without LARM:

```sh
cargo test --manifest-path crates/saaa-conversation-core/Cargo.toml
cargo test --manifest-path crates/saaa-conversation-preview/Cargo.toml --lib
```

`live_saved_database_copy_and_receipt` checks the preview migration and input
receipt against an Online Backup of the saved database, without LARM. Run it
with the two environment variables above and `-- --ignored`.

The ignored live test requires a reachable saved LARM endpoint and working
speaker. It creates one connection, requests one Qwen answer, synthesizes with
the saved TTS choice, plays the result, and releases the connection:

```sh
SAAA_MVP2X_APP_DATA_DIR="$preview_dir" \
SAAA_PREVIEW_SOURCE_DB="$HOME/Library/Application Support/com.saaa.desktop/saaa.sqlite3" \
cargo test --manifest-path crates/saaa-conversation-preview/Cargo.toml --lib \
  live_one_qwen_and_selected_tts -- --ignored --nocapture
```

When the saved voice is System TTS, the speaker path can be checked separately
while LARM is unavailable:

```sh
SAAA_MVP2X_APP_DATA_DIR="$preview_dir" \
SAAA_PREVIEW_SOURCE_DB="$HOME/Library/Application Support/com.saaa.desktop/saaa.sqlite3" \
cargo test --manifest-path crates/saaa-conversation-preview/Cargo.toml --lib \
  live_system_tts_player_only -- --ignored --nocapture
```
