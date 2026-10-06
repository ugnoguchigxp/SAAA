import type { FormEvent } from "react";
import type { MediaKind } from "./mediaApi";
export function MediaGenerationForm({
  kind,
  lockKind = false,
  prompt,
  busy,
  canCancel,
  cancelling,
  setKind,
  setPrompt,
  submit,
  cancel,
}: {
  kind: MediaKind;
  lockKind?: boolean;
  prompt: string;
  busy: boolean;
  canCancel: boolean;
  cancelling: boolean;
  setKind: (kind: MediaKind) => void;
  setPrompt: (prompt: string) => void;
  submit: (event?: FormEvent) => Promise<void>;
  cancel: () => Promise<void>;
}) {
  return (
    <form onSubmit={(event) => void submit(event)}>
      {!lockKind && (
        <label>
          作成するもの
          <select
            aria-label="作成するもの"
            value={kind}
            disabled={busy}
            onChange={(event) => setKind(event.currentTarget.value as MediaKind)}
          >
            <option value="image">画像</option>
            <option value="music">楽曲</option>
          </select>
        </label>
      )}
      <textarea
        aria-label="生成する内容"
        value={prompt}
        disabled={busy}
        onChange={(event) => setPrompt(event.currentTarget.value)}
        placeholder="作りたい画像や楽曲を説明してください"
        rows={2}
      />
      <button
        type="submit"
        disabled={busy || !prompt.trim() || new TextEncoder().encode(prompt).length > 16384}
      >
        生成する
      </button>
      {canCancel && (
        <button type="button" disabled={cancelling} onClick={() => void cancel()}>
          中止
        </button>
      )}
    </form>
  );
}
