# 実装と検証の記録

実施日: 2026年10月1日。対象の回帰は合格、実モデル受入と全体gateは未合格。0件・ignore・途中終了を合格へ加算しない。

## 実行した確認

| 確認 | 結果 |
| --- | --- |
| 新規Context比較＋現行queue E2E＋Tool後Provider失敗 | 5件合格、実モデル2件は通常実行ではignore。live Contextは別途実行して不合格 |
| Memory ONの新規比較 | 1件合格（legacy／stable各20ターン＋異常系） |
| runtime conversation_check unit | 9件合格。Compiler3、stream抽出2、既存queue／予算／replay4 |
| transport chunks | 3件合格。usage-onlyとcontent併記、モデルalias、切断等 |
| memory::context_window | 20件合格。現在入力・過去system除外・出典・projection回帰 |
| task_queue／queue_progress／tts_dictionary／tts_recovery／tts_streaming | 8＋7＋12＋1＋19件合格 |
| 新規offline report | 2件合格。missingと明示0、fallback別試行、未知schema拒否、JSON TTFTなし |
| ASR連続受付／ASR IPC／report frontend | 20件合格 |
| 会話queue UI（単独実行） | 1件合格 |
| 辞書UI（単独実行） | 10件合格 |
| IPCとASR bindings | 7＋1件合格 |
| TypeScript typecheck | 合格 |
| frontend build | 合格 |
| stable指定のmacOS desktop smoke | 合格。build・bundle・起動・画面・IPC・DB・snapshot・speaker runtime準備 |
| feature付きlib Clippy | build成功。新規・変更箇所の新規警告は修正。他の既存警告は残る |
| 対象Rustのrustfmt | 合格。全体fmtは別の変更で不合格 |
| 実モデルの20ターン | 両mode不合格。legacy15ターン目、stable9ターン目にJSON契約不一致 |

queue UI等をまとめてBun実行した際には、別testのglobal mockがcancelConversationInputを上書きして1件失敗した。同じ対象を単独で再実行し合格。まとめて実行した失敗を消さず、個別結果と区別する。

## 全体gateの失敗

- `bun run check`: Personal State coreの106件（58＋17＋2＋5＋9＋15）は合格。その後size gateで停止し、後続は全体コマンドからは実行されていない。
- `bun run size:check`: 既存のratchet超過・未登録・旧ファイルのstale baseline・禁止include分割が多数。今回追加した対象moduleだけ初期登録をレビューし、既存の他のratchetをリセットしていない。
- `bun run clippy:check` と `bun run quality:check`: 実行時のworld_maintenance_contractに `super::retrospective` の解決エラーがありbuild停止。並行World作業に関係する。
- quality runtimeをlibに絞って実行: 1件合格、1件失敗。旧conversation runtimeが削除済みのためharnessが旧経路へ入れない。新queueの合格をこの旧harnessの合格とは扱わない。
- `cargo fmt --check`: 別作業のRustにも形式差分がある。対象ファイルのみ整形・再確認。全体を自動で書き換えていない。

今回の実装に起因したMemory ON上限時の読み上げ拒否、公開後のfollowup失敗の再生成、制御文流出、speech chunk数変更に伴う旧assertの不整合は修正して同じ試験を再実行した。chunk数固定のassertは、本文を連結した完全一致・非空・重複なしの確認へ変更。

## 再実行

```sh
SAAA_CONTEXT_TRIAL_REPORT_DIR="$PWD/spec/evidence/memory-context-first-trial" cargo test --manifest-path src-tauri/Cargo.toml --features conversation-queue-e2e --test conversation_context_stability --test conversation_queue_e2e --test conversation_queue_followup_failure -- --test-threads=1
SAAA_MEMORY_ENABLED=1 SAAA_CONTEXT_TRIAL_REPORT_DIR="$PWD/spec/evidence/memory-context-first-trial/memory-on" cargo test --manifest-path src-tauri/Cargo.toml --features conversation-queue-e2e --test conversation_context_stability legacy_and_stable_twenty_turn_queue_trials -- --test-threads=1
cargo test --manifest-path src-tauri/Cargo.toml --lib runtime::conversation_check::
SAAA_CONVERSATION_PREFIX_MODE=stable bun run desktop:smoke --report-dir spec/evidence/memory-context-first-trial/desktop-smoke-final
```

実モデルは現行のLARM address・profile・認証を使い、`SAAA_CONTEXT_LIVE_MODES=legacy,stable` と絶対report dirを指定して `real_model_context_trial -- --ignored --test-threads=1 --nocapture` を実行する。最初のmodeが失敗した場合は次modeを単独指定する。新規in-memory DBのみで実行する。失敗時にもstatus・成功済み合成会話・receiptを保存する。

[validation-results.json](validation-results.json)に実行ログの摘要・digestを残す。ログ元は作業環境の一時ファイル。全体の第三者変更を今回の完了条件のために巻き戻していない。
