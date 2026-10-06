# 自己診断 v2 実装契約

作成日: 2026-10-06。旧計画 `.archived/saaa-startup-self-diagnosis-implementation-plan.md` の置き換え。製品コンセプトの正本（SAAAの全体コンセプト Page）は、この作業中に参照できなかった。コンセプト変更は含めていない。Capability の一覧は、Page 確認後に見直す。

## 目的

部品の死活ではなく「ユーザーの機能が今使えるか、その根拠は何か」に答える。

## 旧実装の欠陥

1. 起動時の高速診断はプロバイダを `skipped` で返し、`overall` は `skipped` を無視したため、何も検証せず全体が `ok` になった。
2. storage / settings / llm / voice / harness / memory の死活確認だけで、会話・聞き取り・発話を見ていない。
3. 音声契約（TTS のみなら ASR 発話を作らない、再生中の人の声は届く）を検査していない。
4. `runtime_runs` や `audit_events` の実際の失敗を使っていない。
5. 全体に1つのタイムアウトしかなく、1件の停止が全結果を潰した。
6. 結果が自由文で、原因の型も対処もなかった。記録容量の検査は 0 バイトを固定で渡していた。

## 契約

- 機能: `storage` `conversation` `voice-listen` `voice-speak` `voice-echo` `memory` `coding`（optional）。
- 状態: `ready` / `degraded` / `unavailable` / `unverified` / `disabled`。
- 証拠: 層（`static` 設定・構成 / `observed` 実際の動作記録 / `probe` 実機確認）、結果、型付き理由、経路、有効期限。
- 集約規則はコード（`diagnosis/aggregate.rs`）とそのテストが正本。要点は `diagnosis/README.md` の不変条件。
- 全体判定: Core（storage, conversation）が `unavailable` なら `unavailable`。他の `unavailable` / `degraded` があれば `degraded`。Core・Standard に `unverified` があれば `unverified`。それ以外は `ready`。
- 実行範囲: `quick`（起動時・5分ごと・画面を開いた時。プロバイダへの送信なし。LARM のカタログ読み取りのみ行う）、`full`（全プロバイダと LARM を実際に呼ぶ）、`capability`（1機能に関わる全 check）。
- IPC: `get_diagnosis_report`、`run_diagnosis(scope)`、イベント `diagnosis-updated`。`run_fast_diagnosis` は廃止。凍結済みの RuntimeEvent 契約には、型名 `Diagnosis*` で追加される。

## 検証

- `cargo test --lib diagnosis` と `cargo test --test ipc_contract_bindings`。
- `bun test tests/diagnosis-*.test.ts*`、`bun run typecheck`、`bun run lint`、`bun run format:check`。
- 音声契約の自己検査: `voice::audio_backend::echo_reference` のテストと `diagnosis::checks::voice` のテスト。

## 未実施・残件

- 設定変更（プロバイダ削除、LARM アドレス消去）直後は、実機診断の証拠が失効する30分まで古い結果が残る。

- 実アプリでの起動から診断公開までの確認（`tauri dev` の再ビルドループは未切り分け）。
- ターミナルエージェント、用途別クラウド連携、LARM 画像・音楽サービスの専用 check。`coding` は記録のみ。
- 実行失敗を契機にした自動再診断（現状は5分ごとの quick と画面を開いた時）。
