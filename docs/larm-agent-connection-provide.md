# SAAA Agent Connection provide 契約

SAAA の会話、画像、音楽セッションは、それぞれ公開 selector `SAAA`、`SAAA-w-Image`、`SAAA-w-music` を使う。保存済みの旧具体 Profile ID や未対応の値は、実行時に `SAAA` として解釈する。設定値自体は書き換えない。

`POST /v1/agent-connections` が provide 要求である。通常会話の body は次の通り。画像と音楽では `profile` だけを対応する selector に変える。

```json
{"profile":"SAAA","audience":"saaa-desktop","client":"saaa-desktop","ttlSeconds":900,"allowFallback":false,"deploymentPolicy":"existing-only"}
```

作成要求には `Prefer: wait=300` とセッションごとの `Idempotency-Key` を付ける。任意の `GET /v3/agent-profiles?profile=<selector>` を実行したときだけ、その revision を `expectedCatalogRevision` に含める。具体 `agentProfile` は応答で検証する値であり、次の作成要求には使わない。

201 は ready と全 Provider の claimable を検証する。202 は Location の同じ Connection を期限付きで poll する。通常会話には llm、backchannel、asr、tts、embedding が各一件必要で、protocol、endpoint、model を確認する。画像と音楽の追加サービスは `services` で検証し、claim 対象にしない。claim 後の短期 credential、baseUrl、model を推論の正本とし、作成応答と矛盾した場合は推論を止める。終了時は DELETE で release する。

自己診断では TCP 到達、HTTP 到達、discovery、create、semantic readiness、claim、実推論を別項目で表示する。HTTP の 4xx 応答は到達不能として扱わない。API token と claim token は診断結果、SQLite、設定画面、ログに保存しない。
