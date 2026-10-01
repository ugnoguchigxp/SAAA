# ContextStillへの実装依頼: Worldモデルの永続根拠契約

作成日: 2026-10-01。依頼元: SAAA。対象: ContextStillの公開読取API。API名とwire形式は以下の提案を双方で確定する。現行APIが不足していると確定した一覧ではなく、SAAAが必要とする契約である。

## 目的と完了条件

会話から抽出した因果仮説について、関連Episodeと原資料を少量取得し、引用と適用条件を保持してSAAAのWorldへ登録する。更新・削除・権限失効後には古い根拠を使わない。ContextStill側の実装完了後、SAAA側の型付きadapter・根拠検証・失効処理の実装と双方の結合試験が必要。ContextStill実装だけでは全体完了にならない。

現行`memory_recall_v1`の結果はSAAAで`transient_only`と判定する。`search_episodes`/fetchの契約は別途照合する。名前変更、updatedAt、repoPath一致、検索score、取得recordの存在だけでは永続利用を許可しない。

## 提供してほしい契約

1. 認証主体と許可Scopeをサーバーで検証するcapability API。契約version、対応機能、page/本文上限を返す。user/projectの識別子と所有者は認証済みの値を使う。repoPathは検索条件に限り、認可根拠にしない。権限変更時にpermission revisionが変わる。
2. Episode検索。最大件数とScopeを必須にし、各候補にstable ID・immutable revision・型を返す。検索summaryは原資料ではない。scoreは関連度であり因果確実性ではない。
3. 版指定fetch。Episode本文、作成/更新時刻、適用条件、結果・未検証事項、原資料refs、lineageを返す。原資料もID/immutable revisionで必要範囲だけ取得できる。版が変わった場合は旧版を黙って最新本文に差し替えない。
4. 版付き根拠のbatch再検証。要求した各ID/revisionについてcurrent/changed/deleted/revoked/not_foundを返す。失敗や欠落をcurrentと扱わない。本文とScopeの認可状態を確認できる。検証時刻、permission revision、失効/有効期限を返す。再検証後の競合に備え、SAAAは短い期限と送信直前の確認を使う。
5. Scopeごとの変更feed。追加/更新/削除/権限失効を、安定したcursorとページングで取得できる。snapshot watermark、順序、重複・保持期間、cursor期限切れ時の再同期手順を定義する。途中の保留Episodeに依存せず削除イベントを受領できる。検索だけでは全件追跡を保証できない。
6. 原資料引用の検証。resource ID/revision、UTF-8 byteの[start,end)範囲、本文digest（SHA-256・UTF-8・改行を含む原文を明記）、完全/部分/欠落状態を提供する。範囲外・非文字境界を拒否する。要約から原文引用を捏造しない。
7. lineage。Episode、元会話、派生要約の共通出所を識別する。原資料が削除された場合、派生Episodeの利用可否を再検証とfeedに反映する。lineage IDを独立証拠数や権限証明に転用しない。

## wire形式の提案

```json
{
  "contract_version": "world_evidence_v1",
  "resource": {"kind": "episode", "id": "opaque-id", "revision": "opaque-immutable-version"},
  "authorization": {"scope_kind": "project", "scope_id": "opaque-scope", "permission_revision": "opaque-version"},
  "availability": "complete",
  "content": "source text",
  "digest": {"algorithm": "sha256", "encoding": "utf-8", "value": "hex"},
  "refs": [{"kind": "raw", "id": "opaque-id", "revision": "opaque-version", "start_byte": 0, "end_byte": 12}],
  "lineage_ids": ["opaque-lineage"],
  "verified_at": "RFC3339",
  "valid_until": "RFC3339"
}
```

これは説明用の提案であり、未実装のAPIを既存契約として使用しない。時刻だけで最新版性を証明しない。permission tokenや秘密値を返却本文・ログへ出さない。認可不能時は本文を返さない。巨大本文は範囲fetchで取得し、ContextStill側で無断に新規LLM生成を起動しない。

## SQLiteと処理境界

ContextStillとSAAAは別DB・別writer所有者。ContextStillのAPIはContextStillの既存単一writer経由で必要な内部更新を行う。SAAAは公開APIの読取結果を自身の既存SqliteWriterで保存する。別プロセスの直接write、DBファイル共有、ATTACH、外部CLIから本番DB writeを新設しない。WAL/busy_timeoutだけを単一writer保証としない。

feedの一pageをSAAAで永続受領し、その同じ短いtransactionで受領cursorを進める。内容の採用checkpointは別管理。再取得はevent IDまたはscope/resource/revision/event種別のoperation keyで冪等化する。API/LocalLLM待機中にDB transactionやwriter lockを保持しない。プロセス再起動・期限切れlease・重複配信・commit済み応答喪失でも取りこぼさない。

## 受入試験

| ケース | 必須結果 |
| --- | --- |
| 同じIDの本文更新 | immutable revision変更。旧版再検証はchanged。引用の差し替えなし |
| 原資料とその要約を同時取得 | lineageで同一出所と判別。独立証拠として重複加算しない |
| delete/撤回/Scope権限失効 | feedと再検証が失効を返す。本文再取得不可。SAAAが依存関係と回答利用を停止 |
| fetch後・commit前に更新 | 再検証で検出し保留/再取得。旧版をactive採用しない |
| 取得後・回答送信前に失効 | 送信前確認でWorldブロックを除去。ログと送信内容一致 |
| 空検索・部分本文・欠落ref・不正byte範囲 | no_change/保留/明確なerror。根拠を捏造しない |
| page受領後にクラッシュ、重複page | 再起動後のcursor再開。失効を落とさず重複登録しない |
| 保留Episodeの後に削除通知 | 採用保留が後続の削除通知を塞がない |
| cursor期限切れ | 明示エラーと再同期。黙って新cursorへ進めない |
| 他user/projectのIDやLLM生成Scope | server側で拒否。情報漏洩0 |
| APIタイムアウト/サービス停止 | SAAA会話経路は継続。外部根拠は未確認扱い。cloud fallbackなし |

## 依頼先から返してほしい成果

既存APIで充足済みの項目と根拠、追加した公開schema/認可・削除・cursor契約、互換性方針、隔離DB fixture、再起動/競合/失効の試験結果を返してほしい。未充足は明示する。SAAAはcapability確認から順に接続し、旧契約の判定を保持する。

依頼書の作成はContextStillへの送信や別repo変更を意味しない。別repoの実装・設定変更はそのプロジェクトで別途行う。

根拠: [SAAA実装計画](saaa-world-continuous-maintenance-implementation-plan.md) §6、`src-tauri/src/memory/personal_state/world/evidence_eligibility.rs`、既存single SqliteWriterの所有権/短期transaction契約。
