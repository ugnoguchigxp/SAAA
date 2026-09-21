import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { z } from "zod";
const schema = z.object({
  stateInput: z.boolean(),
  graph: z.boolean(),
  freshToolContinuation: z.boolean(),
  verifiedStateAnswer: z.boolean(),
  answerMode: z.string(),
});
export function WorldProviderCapabilities({
  providerId,
  revision,
}: {
  providerId: string;
  revision: string;
}) {
  const [capabilities, setCapabilities] = useState<z.infer<typeof schema> | null>(null);
  useEffect(() => {
    let live = true;
    setCapabilities(null);
    void invoke("world_provider_capabilities", { providerId })
      .then((value) => {
        if (live) setCapabilities(schema.parse(value));
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [providerId, revision]);
  if (!capabilities) return null;
  return (
    <details>
      <summary>World情報への対応（保存済み設定）</summary>
      <ul>
        <li>現在状態: {capabilities.stateInput ? "対応" : "非対応"}</li>
        <li>関係グラフ: {capabilities.graph ? "対応" : "非対応"}</li>
        <li>Tool後の更新: {capabilities.freshToolContinuation ? "対応" : "非対応"}</li>
        <li>
          状態回答: {capabilities.verifiedStateAnswer ? "根拠照合後に表示" : "本体の確認結果を表示"}
        </li>
      </ul>
      <p>直近の送信結果は会話の対象欄で確認できます。</p>
    </details>
  );
}
