# ADR 0002: Situation signal and privacy boundary

- Status: Accepted
- Date: 2026-08-26
- Milestone: MVP 1 — Situation Shadow Mode

## Decision

MVP 1は、ユーザーが明示的に有効化した場合だけ、次のread-only Hard SignalをSituation Runtimeへ入力する。

| Signal | Source | Runtimeへ渡す値 | Persisted value | Health / fallback |
|---|---|---|---|---|
| Foreground Application | macOS `NSWorkspace.frontmostApplication` | bounded category | category由来のevidence codeのみ | `unsupported` / `degraded` |
| Conversation | SAAA runtime lifecycle | idle / user-input / model-running / agent-running | stable stateとreason code | always local |
| Microphone | SAAA Push-to-talk / STT lifecycle | inactive / SAAA capturing / SAAA transcribing | stable stateとreason code | always local |
| Audio | SAAA TTS lifecycle | silent / SAAA speaking | stable stateとreason code | always local |
| Calendar | optional platform adapter | free / busy / meeting-likely / unavailable | healthとcoarse reason codeのみ | MVP 1 macOS buildは`unsupported` |

Foreground adapterはbundle identifierをadapter内部だけで参照し、`communication | coding | writing | browser | media | sensitive | other | unknown`へ変換した直後にraw valueを破棄する。Window title、process list、executable path、document nameは取得しない。

Calendar adapterは既定で無効とする。現在のnative dependencyとpermission boundaryのまま、EventKitの件名・参加者・場所・URLをRust contractへ入れずに安全にbundleする経路を確証できなかったため、MVP 1では明示的な`unsupported` adapterを採用した。Calendar opt-in時もApplication全体を失敗させず、Foreground categoryとSAAA所有lifecycleだけでdegraded動作する。

System-wide microphone / audio activityは、private API、process scraping、Accessibility、Screen Recording、audio captureを使わずに安定して取得する経路を確証できなかったため、MVP 1へ含めない。MVP 2でSystem Audioを扱う場合も、別のpermission reviewとADRを必要とする。

## Data minimization

SQLiteへ保存できるSituation payloadを次に限定する。

```text
scene
confidence
user attention class
audio environment class
bounded evidence code + weight
signal source + health
shadow attention decision
fixed actual execution NONE
fixed actual presentation SILENT
rule / policy version
timestamp and entry kind
structured user verdict
```

次はSQLite、backup、diagnostics、log、UIへ保存・転送しない。

```text
raw bundle identifier
process name / process list
window title
screen pixels
Calendar title / attendee / location / note / URL
audio sample / transcript content
conversation prompt / response
workspace path / Codex thread id
```

`evidence_json`と`decision_reasons_json`は英数字とhyphenだけのbounded codeにvalidationし、free textを拒否する。user feedbackは`accurate | inaccurate | unsure`とoptional corrected sceneだけで、free-text noteを持たない。

## Lifecycle and retention

- Monitoringはdefault off。
- offまたはpause時はplatform pollerを起動しない。
- enable時もsampling intervalは最低500ms、既定2秒。
- SQLiteへはtransition、decision変更、既定5分heartbeatだけを保存し、raw sampleを保存しない。
- retentionは既定7日、最大30日、entry上限は10,000。
- event queueは64件、UI historyは直近100件。
- Situation adapter failureはMVP 0のChat、Voice、Codexを停止しない。

## Shadow safety

MVP 1のPolicy出力はすべて次を満たす。

```text
mode                shadow
actualExecution     NONE
actualPresentation  SILENT
```

Situation moduleからModel Provider、Codex、TTS、Notification、Application Adapterへの呼び出し経路を作らない。既存の明示的なChat、Voice、Codex操作はSituation decisionでguardしない。

## Consequences

- macOSではAccessibility permissionなしにForeground categoryを利用できる。
- Calendarと外部mic/audioが利用できない環境でもMVP 1のminimum signal setが成立する。
- Meeting推定の精度は限定されるが、false interventionは発生しない。
- 後続MVPは、評価台帳の結果を確認してから実行Policyへ昇格させる必要がある。
