# macOS VoiceProcessingIO による AEC と Other Audio Ducking 制御 実装計画

状態: 実装済み・実機受入未完了。Spotify 等の減音量、SAAA の TTS に対する AEC 効果、通常の会話経路の機器別結果は未記録。CLI は `cargo run --manifest-path src-tauri/Cargo.toml --bin vpio_probe -- --seconds 1`（Bluetooth 出力で試す場合は `--bluetooth` を追加）。対象: macOS 14 以降。確認環境: macOS 26.6.2 SDK。

2026-09-25 の単体確認: Bluetooth 出力では既定設定のままでは VPIO を開かず、`--bluetooth` 指定時は `aec=true`、`agc=false`、`ducking=min` で起動した。これは設定読み戻しの確認であり、他アプリの実際の減音量や AEC の効果を証明しない。Qwen Audio Agent の参照実装は同じ VPIO で再生・録音するが、ducking 制御と無減音の測定結果は含まない。

## 1. 結論

- AEC 採用判定: **条件付き採用**。SAAA の TTS 再生とマイク入力を**同じ VoiceProcessingIO（VPIO）インスタンス**に通す場合に限り採用する。マイクだけ VPIO に通しても、SAAA 自身の TTS はほとんど消えない。
- Ducking の最有力原因: WebView の `getUserMedia` に `echoCancellation: true` を渡していること（`src/lib/microphone.ts`）。WebKit はこの指定があると内部で VPIO を使ってマイクを開く。VPIO の既定設定は「advanced ducking 無効、ducking level Default」なので、マイクを開いている間は他アプリの音量が下がり、閉じると戻る。報告された症状と一致する。
- Ducking の制御: `kAUVoiceIOProperty_OtherAudioDuckingConfiguration`（macOS 14+）で `mEnableAdvancedDucking = false`、`mDuckingLevel = kAUVoiceIOOtherAudioDuckingLevelMin` を明示設定する。ただし**公開 API に ducking を完全に 0 にする値はない**。Min が最小。WebKit の内部 VPIO はアプリから設定できないため、WebView 経由のままでは制御できない。

## 2. Qwen Audio Agent の実装解析（`tui/native/macos-voice-io.swift`）

### 構成

| 項目 | 実装 |
| --- | --- |
| Unit | `kAudioUnitType_Output` / `kAudioUnitSubType_VoiceProcessingIO`（L222-241） |
| マイク有効化 | `kAudioOutputUnitProperty_EnableIO`、Input scope、bus 1（L251-262） |
| 再生形式 | `StreamFormat` Input scope bus 0 = 48kHz mono Int16（L269-279） |
| 録音形式 | `StreamFormat` Output scope bus 1 = 同じ 48kHz mono Int16（L280-290） |
| TTS 再生 | bus 0 に `SetRenderCallback` → `SampleQueue.read` で TTS PCM を供給（L292-306, L347-379） |
| マイク取得 | bus 1 に `SetInputCallback` → callback 内で `AudioUnitRender(bus 1)` を呼んで AEC 済み音声を取得（L308-322, L381-418） |
| ducking/AGC 設定 | **なし**（OS 既定のまま） |

### データフロー

```text
TTS PCM (24kHz Int16, stdin JSON/Base64)
  → 線形補間で 48kHz へ resample（play, L420-439）
  → SampleQueue
  → bus 0 render callback（VPIO が pull）
       VPIO 内部: スピーカーへ出力 ＋ 同じ信号を AEC の far-end reference として保持
Microphone
  → VPIO 内部 AEC（far-end reference を差し引く）
  → bus 1 output（AudioUnitRender で取得）
  → 48kHz → 16kHz resample → Base64 JSON → stdout → VAD/ASR
```

### なぜ AEC が成立するか

VPIO は自分の出力 bus 0 に流れた信号を正確なタイミングで知っており、それを reference にしてマイク信号から推定エコーを差し引く。再生と録音が同じ unit・同じ clock・同じデバイスなので、reference とエコーの時間ずれを VPIO が内部で推定できる。**TTS を VPIO 経由で再生することが AEC の必須条件**。rodio など別経路で再生した音は reference に入らないため、除去されないか効きが弱い（macOS 14 以降は他アプリ音もある程度抑える挙動が報告されているが、Apple は保証していない）。

## 3. SAAA 自身の TTS と他アプリ音

- SAAA 自身の TTS: VPIO bus 0 経由で再生すれば、スピーカー使用時でも full-duplex / barge-in に実用的な精度が見込める。残響の多い部屋や大音量では残留エコーが出るため、VAD 側に「TTS 再生中は閾値を上げる」保険を残す。
- YouTube / Spotify など他アプリ: VPIO は reference を持たないため**確実な除去はできない**。system-wide の除去には ScreenCaptureKit / Core Audio Process Tap（macOS 14.2+）でシステム音を取り、自前の AEC に reference として渡す必要がある。今回の範囲外とする。

## 4. Qwen 実装の Realtime Safety

callback 内に次の問題がある。

| 場所 | 問題 |
| --- | --- |
| capture（L387） | `[Int16](repeating:)` の heap allocation を毎回実行 |
| capture（L406） | `shouldCapture()` で `NSLock` |
| capture（L408-416） | resample（配列確保）、`Data` 生成、Base64、Dictionary 生成、`DispatchQueue.async` |
| render（L357） | `SampleQueue.read` で `NSLock`、`Set<String>` の insert/remove、signals 配列確保 |
| render（L366-374） | `emit` の Dictionary 生成と async dispatch |
| `play`/`append` | 別スレッドが同じ lock を握るため、render callback が priority inversion で待つ可能性 |

JSON 化と stdout 書き込み自体は `outputQueue` 上なので callback 外だが、それ以外は realtime thread で避けるべき処理。SAAA では次の構造にする。

```text
CoreAudio callback（allocation・lock・ObjC/Swift ARC なし）
  → 事前確保バッファへ memcpy
  → lock-free SPSC ring buffer（mic 用 / playback 用に 1 本ずつ）
  → worker thread: resample、VAD、ASR 送信、イベント通知
```

- そのまま参考にできる: unit 生成、bus 0/1 の EnableIO・StreamFormat・callback 設定の手順、「両 bus を同一形式にする」方針、`done` を最終サンプル消費後に通知する考え方。
- 設計だけ参考: SampleQueue（SPSC ring に置き換え）、responseId 単位の started/ended 通知（atomic カウンタで worker に渡す）、`clear` による即時停止。
- 採用しない: callback 内 resample、線形補間 resampler、Base64/JSON の stdin/stdout IPC、NSLock。

## 5. Sample Rate / PCM

- Qwen: TTS 24kHz → 48kHz（線形補間）→ VPIO 48kHz → mic 48kHz → 16kHz（線形補間、anti-alias フィルタなし）→ ASR。線形補間のダウンサンプルは折り返し歪みが出るため、ASR 精度に不利。
- SAAA 現行: ASR は WebView の `AudioContext({ sampleRate: 16000 })` と AudioWorklet で 16kHz。TTS は rodio `SamplesBuffer`（例: 24kHz mono i16）で再生し、rodio/CoreAudio が出力デバイス rate へ変換する。

推奨:

- VPIO の client 形式は **48kHz mono Float32**。VPIO 内部はデバイスや経路で rate が変わるため、client 形式を固定して変換は VPIO/AudioConverter に任せる。Float32 は AudioConverter や VAD と相性がよく、クリップ処理も不要。Provider 境界だけ Int16 にする。
- canonical PCM を 48kHz にするのは妥当。変換は「TTS 24kHz → 48kHz を 1 回」「mic 48kHz → ASR 16kHz を 1 回」の各 1 回に抑える。
- resample は worker thread で `AudioConverter`（`kAudioConverterSampleRateConverterQuality_High` 程度）を使う。callback では行わない。
- ASR provider が 16kHz 固定なら、mic 側は 16kHz を client 形式に指定して VPIO 内部で変換させる案も検証する（変換 1 回で済む）。ただし bus 0 と bus 1 の形式を揃える必要があるため、その場合は TTS 側を 16kHz に落とすことになり音質が下がる。まず 48kHz で統一し、ASR 境界で 1 回変換する方を採用する。

## 6. Voice Processing の個別制御（SDK ヘッダ `AudioUnitProperties.h` で確認）

| 機能 | API | 可否 | 推奨 |
| --- | --- | --- | --- |
| AEC 全体 | `kAUVoiceIOProperty_BypassVoiceProcessing`（2100） | ON/OFF のみ。bypass すると AEC・NS・AGC すべて止まる | 0（処理有効） |
| AGC | `kAUVoiceIOProperty_VoiceProcessingEnableAGC`（2101） | 個別に ON/OFF 可 | 0（OFF）。VAD 閾値と録音レベルを安定させるため |
| Noise Suppression | 公開 property なし | **個別制御不可**。AEC と一体 | 受け入れる。VAD 閾値を VPIO 有効時の値で再調整 |
| Other audio ducking | `kAUVoiceIOProperty_OtherAudioDuckingConfiguration`（2108, macOS 14+） | level を Default/Min/Mid/Max から選択。完全無効化は不可 | advanced=false, level=Min |
| 出力 mute | `kAUVoiceIOProperty_MuteOutput`（2104） | 可 | 使わない |
| mute 中の発話検出 | `kAUVoiceIOProperty_MutedSpeechActivityEventListener`（2106, macOS 14+） | 可 | 今回不要 |
| `kAUVoiceIOProperty_DuckNonVoiceAudio` | macOS では unavailable（iOS 7 で廃止） | 不可 | 使わない |

AVAudioEngine を使う場合の同等 API は `AVAudioInputNode.setVoiceProcessingEnabled(_:)`、`isVoiceProcessingAGCEnabled`、`isVoiceProcessingBypassed`、`voiceProcessingOtherAudioDuckingConfiguration`（macOS 14+）。

### Ducking の性質（WWDC23「What's new in voice processing」より）

- VPIO を使うと、他アプリの音声は既定で ducking される。macOS 14 で ducking 量を調整する API が追加された。
- `mEnableAdvancedDucking = true` は「どちらかが話している間だけ強く ducking し、無音時は戻す」動的方式。SAAA の要件（他アプリ音量を勝手に変えない）には合わないため false にする。
- 最小でも Min 相当の音量低下は残る。これを 0 にする公開 API はない。完全に 0 が必須なら、VPIO を使わない構成（別途 AEC ライブラリ、例: WebRTC APM）しかない。

### 推奨設定

```text
VoiceProcessingIO        enabled
BypassVoiceProcessing    0
AEC                      ON（暗黙）
Noise Suppression        ON（制御不可）
AGC                      OFF（VoiceProcessingEnableAGC = 0）
OtherAudioDucking        mEnableAdvancedDucking = false
                         mDuckingLevel = kAUVoiceIOOtherAudioDuckingLevelMin
設定タイミング            AudioUnitInitialize 前に設定し、Initialize 後に読み戻して検証
```

## 7. 現行 SAAA の該当箇所

| 機能 | 場所 |
| --- | --- |
| マイク取得 | `src/lib/microphone.ts`（`getUserMedia`、`echoCancellation: true`, `autoGainControl: false`, `noiseSuppression: false`） |
| 取得パイプライン | `src/features/voice/ambientVoiceCapture.ts`（`AudioContext` 16kHz、`/audio/voice-capture-processor.js` の AudioWorklet）、`ambientVoiceCaptureActions.ts`、`VoiceCaptureResources.ts` |
| VAD | `ambientVoiceCapture.ts` の `detector()` / `resetVoiceActivityDetector` |
| セッション・TTS 停止 | `src/features/voice/useAmbientVoiceSession.ts`（`stopSpeech`） |
| ASR | `src-tauri/src/voice/streaming_asr/`、`network_asr*`、`cloud_asr.rs` |
| TTS 再生 | `src-tauri/src/voice/http_audio/playback.rs`（rodio `OutputStream::try_default` / `Sink`）、`streaming_tts/`、`session/tts.rs`、`system_tts.rs` |
| エコー由来の自己発話抑止 | `src-tauri/src/providers/larm_voice/frontdesk_echo.rs`（テキストレベル） |
| Ducking 処理 | SAAA 独自の実装はない。WebKit の VPIO による OS 側 ducking と判断 |

現状の問題点: マイクは WebKit の VPIO、TTS は rodio と経路が分かれている。このため WebKit の AEC は SAAA の TTS を reference として持てず、ducking だけが発生している可能性が高い。

## 8. 原因の確認手順（実装前に実施）

1. Spotify 等を再生しながら SAAA の音声入力を開始し、音量低下を確認する（現状の再現）。
2. 開発ビルドで `echoCancellation: false` にして同じ操作をする。音量低下が消えれば WebKit の VPIO が原因と確定する。
3. `log stream --predicate 'subsystem == "com.apple.coreaudio" OR process == "coreaudiod"' | grep -i -E "vpio|voiceprocessing|duck"` で開始時に VPIO が作られるかを見る。
4. 補足: `echoCancellation: false` は ASR 凍結パスの変更になるため、恒久対応としては行わない。確認は一時的な変更で行い、コミットしない。

## 9. 推奨アーキテクチャ

```text
Rust (src-tauri)
 └─ voice::audio_backend  (trait AudioBackend)
      ├─ MacOSVoiceProcessingBackend   ← 今回
      │    ├─ VPIO (bus0 in: playback / bus1 out: processed mic)
      │    │     設定: AGC=0, Ducking{advanced=false, level=Min}
      │    ├─ render callback  ← SPSC ring ← TTS worker (24k→48k AudioConverter)
      │    ├─ input  callback  → SPSC ring → capture worker
      │    │                                   ├─ 48k→16k AudioConverter
      │    │                                   ├─ VAD（barge-in 判定）
      │    │                                   └─ streaming ASR
      │    └─ device/route 変更 listener → 再構築
      └─ （将来）WindowsBackend / LinuxBackend、フォールバックとして現行 WebView + rodio

barge-in:
 VAD が TTS 再生中に発話を検出
  → playback ring を clear（callback は次周期から無音）
  → TTS 生成を cancel → 会話状態を interrupted に遷移
```

実装言語: Rust から直接 AudioUnit を叩く（`coreaudio-sys` または `objc2-audio-toolbox`）か、小さな Swift/C の静的ライブラリを FFI で呼ぶ。callback 内に Swift ARC や ObjC メッセージを入れないため、**callback は C か Rust で書く**。Qwen のような別プロセス＋JSON IPC は採用しない。

## 10. 実装フェーズ

| Phase | 内容 | 完了条件 |
| --- | --- | --- |
| 0 | 第 8 節の原因確認 | ducking の原因が WebKit VPIO と確定 |
| 1 | `MacOSVoiceProcessingBackend` 単体（CLI の検証バイナリ）。VPIO 構築、AGC/ducking 設定と読み戻し、SPSC ring、WAV 出力 | Spotify 再生中に起動して音量低下が Min 相当に留まる。TTS 再生中の録音で TTS が大きく減衰している（ERLE を測定） |
| 2 | TTS 再生経路を backend へ（`http_audio/playback.rs`、`streaming_tts`、`session/tts.rs`）。rodio は非 macOS とフォールバックに残す | 既存 TTS テストが通る。再生開始・終了イベントが従来と同じ |
| 3 | マイク経路を backend へ。WebView の `getUserMedia` を macOS では使わず、Rust 側 capture worker から ASR に直接流す。VAD を Rust に移すか、PCM を IPC で WebView の VAD に渡すかを決める | ASR 回帰テストと `voice::streaming_asr` テストが通る |
| 4 | barge-in（TTS 再生中の VAD 検出で停止）と設定 UI（AEC ON/OFF、ducking level） | 「明日の予定は──」に割り込んだ発話だけが ASR される |
| 5 | device 切替・Bluetooth・sample rate 変化への対応、desktop smoke | 下記リスク項目の手動確認表を完了 |

Phase 3 は ASR 凍結パス（`src/lib/microphone.ts`、`ambientVoiceCapture.ts`、`ambientVoiceCaptureActions.ts` など）の変更になる。ユーザーの明示承認を得てから着手し、`bun test tests/ambient-voice-session.test.tsx tests/voice-capture-races.test.ts tests/voice-asr-packet-sender.test.ts` と `cargo test --manifest-path src-tauri/Cargo.toml voice::streaming_asr` を通したうえで `bun run freeze:accept:asr --reason "..."` を実行する。TTS 経路が initial-response 凍結対象に含まれる場合は `bun run quality:check` と `bun run desktop:smoke` を実行して `freeze:accept:initial-response` を行う。

## 11. 変更対象（予定）

- 新規: `src-tauri/src/voice/audio_backend/{mod.rs, macos_vpio.rs, ring.rs, converter.rs}`
- `src-tauri/Cargo.toml`（`coreaudio-sys` または `objc2-audio-toolbox`、lock-free ring 用 crate）
- `src-tauri/src/voice/http_audio/playback.rs`、`streaming_tts/runtime/*`、`session/tts.rs`、`system_tts.rs`
- `src-tauri/src/voice/streaming_asr/*`（入力元を backend に）
- `src/lib/microphone.ts`、`src/features/voice/ambientVoiceCapture*.ts`、`useAmbientVoiceSession.ts`、`VoiceCaptureResources.ts`（macOS で native backend を使う分岐）
- `src-tauri/src/voice_commands.rs`、IPC 契約（`tests/fixtures/ipc-receivers.json`）
- 設定: `src/features/settings/settingsDefaults.ts`、`src-tauri/src/persistence/settings_defaults.rs`、i18n
- `src-tauri/src/providers/larm_voice/frontdesk_echo.rs`（AEC 有効時のテキスト側抑止の緩和）

## 12. リスク

- Realtime safety: callback 内の allocation・lock・ログ出力は音切れの原因になる。callback は memcpy と atomic 更新のみ。CI に callback 内 allocation を検出するテスト（custom allocator のカウンタ）を入れる。
- Device switching: 既定デバイス変更時に VPIO は停止または形式変更する。`kAudioHardwarePropertyDefaultInputDevice` / `DefaultOutputDevice` と `kAudioUnitProperty_StreamFormat` の変更を監視し、unit を作り直す。
- Bluetooth / AirPods: VPIO で入力を開くと HFP に切り替わり、出力音質が下がる（16kHz 前後）。AirPods 使用時は AEC の必要性が低いため、「Bluetooth ヘッドセットでは VPIO を使わない」選択肢を設定に入れる。
- Sample rate changes: デバイス rate は変わり得る。client 形式を 48kHz に固定し、変換は VPIO と worker 側 AudioConverter に任せる。
- Speaker latency: 外部スピーカーや AirPlay は遅延が大きく、AEC の追従が悪い。AirPlay 出力では AEC 無効を既定にする。
- AGC: 既定で有効な場合があるため、明示的に 0 を設定し、Initialize 後に読み戻して確認する。
- Ducking: Min でも完全な 0 にはならない。完全に不要なら VPIO 以外の AEC が必要。macOS 13 以前は ducking 設定 API がないため、backend は macOS 14 未満では無効にする。
- Feedback / echo: AEC の収束前（開始直後の数百 ms）は残留エコーが出る。TTS 開始直後は barge-in の閾値を上げる。
- TTS interruption: ring の clear と TTS 生成の cancel が同期しないと、停止後に数十 ms の音が漏れる。clear は世代番号（atomic）で行い、古い世代のデータを worker 側で捨てる。
- WebView との二重 VPIO: 移行中に WebKit の `getUserMedia(echoCancellation: true)` と native VPIO が同時に動くと ducking が重なり、AEC も干渉する。macOS native backend 有効時は WebView 側でマイクを開かないことを保証する。native を開けないとき（macOS 14 未満、AirPlay、Bluetooth 既定）は `echoCancellation: false` で開く。
- rodio 再生中にマイクを始めた発話: その発話の再生は rodio のままなので、同じ VPIO の far-end reference に入らず AEC は効かない。次の発話から VPIO 再生に乗る。これは許容する。

## 13. 参照

- Qwen Audio Agent `tui/native/macos-voice-io.swift`（ローカル: `../qwen-audio-agent/`）
- macOS SDK `AudioToolbox.framework/Headers/AudioUnitProperties.h`（`kAUVoiceIOProperty_*`、`AUVoiceIOOtherAudioDuckingConfiguration`）
- Apple WWDC23 "What's new in voice processing"
- Apple Developer Documentation: `kAudioUnitSubType_VoiceProcessingIO`、`AVAudioInputNode.voiceProcessingOtherAudioDuckingConfiguration`
