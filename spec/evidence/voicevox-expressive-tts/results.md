# VOICEVOX expressive TTS results

## 実行済み

`cargo test --manifest-path src-tauri/Cargo.toml --lib ve_ -- --test-threads=1`

対象は設定境界、speech request field、catalog 正規化、予約語 projection、expression 計算。

## 未実行

計画 Section 8 の bun / clippy / 全体 Gate、および Section 9 の live 受入。LARM catalog の実 response は未確認。

## 既存失敗として分けたもの

`legacy_only_settings_reopen_without_selecting_a_cloud_fallback` は `Unsupported settings document` で失敗する。失敗メッセージは今回追加した韻律 validation ではなく、settings document の namespace 拒否である。
