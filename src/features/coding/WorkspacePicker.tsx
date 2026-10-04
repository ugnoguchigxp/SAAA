import { terminalErrorMessage } from "./terminalErrors";
import { useState } from "react";
import { codingApi } from "./api";
export function WorkspacePicker({
  conversationId,
  current,
  onSaved,
  onError,
}: {
  conversationId: string;
  current?: string;
  onSaved: () => void;
  onError: (message: string) => void;
}) {
  const [path, setPath] = useState("");
  const [busy, setBusy] = useState(false);
  async function save() {
    setBusy(true);
    try {
      await codingApi.workspace(conversationId, path);
      onSaved();
    } catch (error) {
      onError(terminalErrorMessage(error));
    } finally {
      setBusy(false);
    }
  }
  return (
    <details>
      <summary>実装先: {current ?? "未選択"}</summary>
      <label>
        ローカルGitフォルダーの絶対パス
        <input
          value={path}
          onChange={(e) => setPath(e.target.value)}
          placeholder="/Users/…/project"
        />
      </label>
      <button disabled={busy || !path.trim()} onClick={() => void save()}>
        このフォルダーを選択
      </button>
      <p>選択後、会話で実装を依頼してください。</p>
    </details>
  );
}
