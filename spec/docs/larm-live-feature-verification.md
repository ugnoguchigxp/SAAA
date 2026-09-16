# LARMの公開機能・実機検証（2026-09-16）

## 結論

全公開モデルと接続Profileを対象に検証した結果、すべてが稼働する状態ではない。通常のLLM、ASR、VOICEVOX WAV、Embedding、9種類の接続Profileは実応答を確認した。一方、Qwen TTS、PCM、voice一覧、管理メモリ拡張、特定のLLM要求に失敗が残る。

検証先はSAAAに保存済みの`http://192.168.0.130:9810`。稼働releaseは`be5e86272165f4e13e69f4da204b1425486c6070`、検証時のLARM repository HEADは`fff3d4af2f8ca7f7236b489fe140c2eeddeca95c`で、さらに未コミット変更があった。sourceの成功報告と稼働版の結果は区別する。配備・再起動・設定変更は行っていない。

SAAAの検証対象commitは`68af784f9ac1dbd1cdf9bcea8b84531cb63190c3`。Macから通常Bearer経路を呼び、Agent Connectionのscoped credential経路とSAAA自身のRustクライアントも検証した。秘密情報は記録に含めず、入力は検証用の短文・合成音声・無音だけを用いた。

## LLM：公開5モデル

canaryは`Reply with just OK.`、`max_tokens=256`、`temperature=0`、非stream JSONで、stopかつ可視本文が厳密にOKの場合だけ成功とした。それ以外は最大1024トークン。SSEでは本文、finish_reason、DONEを検査した。toolは副作用のないconnection_checkを要求し、引数value=OKを組み立て、合成のtool結果を返して最終回答まで確認した。Schemaはok=trueだけのJSONを要求した。

| 公開model | 厳密OK canary | SSE本文・終端 | SSE tool呼び出し | tool結果から回答 | JSON Schema |
| --- | --- | --- | --- | --- | --- |
| coding-default | 本文空 | 成功 | 成功 | 成功 | 成功 |
| decision-default | 成功 | 成功 | 成功 | 成功 | 成功 |
| qwen-agent-worker | 成功 | 成功 | 成功 | 成功 | 成功 |
| qwen-nightworker | 成功 | 成功 | 成功 | 成功 | 成功 |
| qwen3.8-kv-mem | 成功 | 成功 | 503 起動待ち期限超過 | 未到達 | 成功 |

- coding-defaultのcanaryは200・stop・生成4トークンで本文空だった。上限消費では説明できない。
- qwen-agent-workerは初回canary・Schemaが90秒でタイムアウト。再試行ではcanaryが約66.55秒、Schemaが約3.87秒で成功した。
- qwen3.8-kv-memも初回canaryは90秒でタイムアウトし、再試行は約12.20秒で成功した。tool要求は90秒タイムアウトの後、待ち時間を180秒へ広げた再試行でも約120秒で503 model_loading_timeout。tool非対応と断定せず、この要求の準備・割り当てが完了しない問題として扱う。tool往復の残りは未到達。
- 一部の接続Profile確認を推論試験と並行した。起動・swap・待ち行列・既存負荷を含む実測であり、純粋な生成速度や性能比較ではない。接続Profileの再試行は推論系列の終了後に行った。

## ASR・TTS

| 機能 | 実測結果 |
| --- | --- |
| qwen3-asr-1.7b / JSON | 成功。「接続を確認しました。」を認識 |
| qwen3-asr-1.7b / verbose_json | 成功。text、language=Japanese、durationを取得 |
| qwen3-asr-1.7b / 無音 | 成功。textが空で、架空の発話を返さない |
| voicevox-core / WAV | 4 voiceすべて成功。24 kHz・mono・16 bitの有効なWAV |
| voicevox-core / PCM | 502 upstream_response_format_mismatch |
| qwen3-tts-expressive / WAV | 9 voiceを要求。起動後は公開model名が400 invalid_modelで拒否される |
| qwen3-tts-expressive / PCM | 400 invalid_model |
| voice一覧GET | 両TTS modelともbodyなしGETが400 invalid_request。JSON bodyを要求する |

VOICEVOXの対象はShikoku_Metan、Zundamon、Kasukabe_Tsumugi、Amehare_Hau。Qwen TTSの対象は実機設定にあるAiden、Ono_Anna、Vivian、Serena、Uncle_Fu、Dylan、Eric、Ryan、Sohee。Aidenの初回は90秒タイムアウトしたが、起動後の再試行では他のvoiceと同じinvalid_modelになった。公開名qwen3-tts-expressiveと、upstreamが受け付けるqwen3-tts等の名前が一致していない。voiceの音質・正しさはモデル拒否より後のため確認できない。

ASRへ渡したのはVOICEVOXで生成した約1.803秒の音声で、実マイクではない。音声bodyの形式・長さは検査したが、端末スピーカーでの聴取や音質評価は実施していない。PCMは成功bodyを取得できず、24 kHz s16leの実機保証は未確認。

## 接続ProfileとEmbedding

create → ready → get → health → claim → Provider health → renew →再claim →旧credentialの401 → release →失効credentialの401を確認した。初回はready待ち60秒、未readyの4件は180秒へ延長して再試行した。

| Profile | 最終結果 |
| --- | --- |
| asr-qwen | 成功 |
| coding-default | 成功 |
| contextstill-background | 成功 |
| contextstill-decision-default-canary | 成功 |
| contextstill-embedding | 成功 |
| deep-reasoning-35b | 成功 |
| nightworker-background | 成功 |
| saaa-qwen38-kv-mem | 成功 |
| tts-default | 成功 |
| tts-expressive | 失敗：connection_ready_timeout |

tts-expressiveはprobingからfailedへ遷移し、connection_ready_timeoutを返した。DELETEは204だが接続記録はfailedのまま残る。下位Allocationはreleasedを確認したため、これをリソース解放失敗とは判定しない。

contextstill-embeddingではqueryとpassageの両方を実行し、384次元・L2ノルム約1のベクトルを確認した。更新・解放後の旧credentialは401となった。意味検索の精度までは評価していない。

## SAAA自身の接続経路

`SAAA_LARM_CONTROL_URL=http://192.168.0.130:9810 cargo test --manifest-path crates/larm-session/Cargo.toml --test live -- --ignored --nocapture`が成功した（1件、約18.75秒）。decision・LLMの本文、TTS WAV、ASRの非空認識、セッション解放、解放後tokenの401まで確認した。

通常Harness discoveryとして、認証なしの/v1/servicesと/v1/services/asr/health、および広告されたASRへの無音multipart送信も成功した。このdescriptorが広告するのはASRだけであり、全機能の能力manifestではない。画面操作・会話UI・実スピーカーまでを通したE2E合格を意味しない。

## 管理メモリ・エラー・キャンセル

- 現在のLARM側personal-state-conformance.tsを、作成したsaaa-qwen38-kv-mem接続のscoped tokenで実行した。SAAAと同じaudience=saaa-desktop・client=saaa-coding-agent・LAN originでもcapability取得が401 unauthorizedになった。source作成に到達しない。cleanup側のforgetも401、接続のreleaseは成功した。source登録、measurement、view、attempt receipt、削除証跡を合格にはできない。
- Allocationなしのx-larm-attempt-id付き通常Chat要求は200となった。受領した仕様の400 allocation_requiredとは一致しない。独自保証を要求したのに通常推論の成功として見える問題が稼働版に残る。
- 存在しないmodelは404 model_not_found、不正なChat bodyは400 invalid_requestを確認。
- SSEの最初の行を受け取ってHTTP接続を閉じ、セッションを解放してcredentialの401を確認した。engineの計算が直ちに止まることまでは、この検証から保証しない。
- 明示Allocationのcreate/get/resolve/renew/deleteと、旧prepare/resolve/releaseも、自分で作成した検証用リソースについて成功した。同一decision runtimeへ2個目のAllocationを作ると409 resource_exhaustedとなり、上限1の拒否も観測した。

### SAAA側にも残る契約不一致

実機で409 resource_exhaustedを観測した。一方、crates/larm-session/src/http.rsは容量不足を429だけで判定し、409をlarm_http_rejectedへ落とす。src-tauri/src/providers/route_policy.rsではこれはContract扱いとなり、共有LARM音声セッションの自動fallback対象にならない。これは実機のHTTP応答とSAAAコードを照合した指摘であり、実クラウドへの切り替え試験ではない。409全体を再試行するのではなく、安定したerror codeを読んで容量不足だけを分類する必要がある。

## 制御APIの確認範囲

稼働OpenAPIには51 path・54 operationがある。health、ready、activity、metrics、OpenAPI、3世代のProfile catalog、サービス情報、context status、モデル一覧、runtime一覧・個別詳細・inspection、release convergence、runtime release一覧、deployment状態の計63要求を検証した。62件が200、voicevox-ttsのdeployment状態だけ404 runtime_not_found（release catalog entryなし）だった。推論用runtimeとしてのVOICEVOXは別途WAV成功を確認しており、この404を音声機能の失敗とは扱わない。

すべての管理用mutationを実行したわけではない。artifactのstage、releaseのstage、deploymentのplan/activate/rollback、再起動を伴うKV復元試験は、共有稼働環境を変更するため実施していない。context登録・view生成等は上記personal-state認証で阻まれた経路も含め、未検証のものを成功扱いしない。15種類のruntime構成の存在と、8公開model・10接続Profileとして提供される機能も区別した。

## LARM側への確認事項

1. 新releaseの配備identityを固定してvoice一覧・PCM修正を実機へ反映する。
2. qwen3-tts-expressiveの公開名からupstream modelへの対応を修正し、通常推論とProfile readinessの両方を通す。
3. coding-defaultの短文canaryがstop・少数トークン・本文空となる条件を調査する。
4. KVメモリ用modelが直前の要求に成功しても次のtool要求でmodel_loading_timeoutとなる条件を調査する。
5. scoped tokenによるpersonal-state capabilityと、Allocationなしattempt headerの拒否を実機確認する。
6. 容量不足のHTTP status・error codeを固定し、SAAA側の分類と合わせる。

認証情報を除いた個々の検証結果は[機械可読の検証記録](verification/larm-live-20260916.json)に保存した。単一の成功率へまとめると、正常な拒否・準備待ち・未到達・実機不良が混ざるため、機能別の表を判定の正本とする。

検証ログから追跡できる接続25件を終了時に再確認し、24件がreleased、1件がfailedかつ下位Allocation releasedで、未解放の検証用リソースは見つからなかった。SAAA Rustテストとpersonal-state用接続も、それぞれの終了処理でrelease成功を確認した。検証開始時と終了時のrelease identityは同一だった。


## Qwen TTS・KVメモリLLMの2点再確認

LARM側の修正報告後、この2点だけを再検証した。稼働releaseは依然be5e862で、配備操作は実施していない。

- Qwen TTS：公開名qwen3-tts-expressive、voice=Ono_Anna、WAV要求は約0.21秒で400 invalid_model。repositoryのconfig.production.yamlはdefault_modelとmodelsのキーが公開名へ変更済みで、release設定のproviderConfigRevisionもqwen-tts-0.6b-v2-public-modelとなっている。しかし実機のupstream設定は0.6B-CustomVoiceのままである。
- さらに、実機upstreamのapi/routers/openai_compatible.py:148のMODEL_MAPPINGはtts-1、tts-1-hd、qwen3-ttsと各言語aliasを固定定義し、233行目でこの辞書にないmodelを400で拒否する。YAMLのmodelsを参照して許可する処理ではない。このupstream実装のままなら、YAMLのキー変更だけでは拒否を解消しない。APIのmodel alias受け入れ、またはGatewayのupstream model変換も確認・修正が必要である。
- KVメモリLLM：SSE tool呼び出しは約15.02秒で成功し、正しい引数・tool_calls終端・DONEを確認した。しかし合成tool結果を返した次の要求は約120.06秒で503 model_loading_timeoutとなり、往復全体は不合格。
- repositoryのlarm-daemon.serviceにはLARM_CONNECTION_READY_TIMEOUT_SECONDS=300が存在する。daemon側は同じ設定値をAgent Connectionのready待ちとModel Brokerのstartup待ちへ渡す。稼働プロセスの環境にはこのoverrideがなく、現コードの既定値は120秒である。今回の120秒失敗は未反映の状態と整合し、300秒へ延長した効果はまだ実機検証できていない。

### 同じ2点の再実行（2026-09-16T04:03:24.901498+00:00）

Qwen TTSは約0.22秒で400 invalid_model。KVメモリLLMのSSE tool呼び出しは約15.66秒で成功したが、tool結果を返した後の要求は約120.06秒で503 model_loading_timeoutとなった。今回も往復全体は不合格。稼働releaseはbe5e862のままで、稼働プロセスにはLARM_CONNECTION_READY_TIMEOUT_SECONDSのoverrideがない。配備や設定変更は実施していない。
