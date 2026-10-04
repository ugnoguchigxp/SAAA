import type { MediaOutput } from "./mediaApi";
export function MediaArtifacts({
  output,
  files,
  prompt,
  fetching,
  error,
  retry,
}: {
  output: MediaOutput;
  files: Record<number, string>;
  prompt: string;
  fetching: boolean;
  error: string | null;
  retry: () => Promise<void>;
}) {
  return (
    <>
      {" "}
      {output?.result && (
        <div className="media-generation-artifacts">
          <p>生成成功 · {output.result!.model}</p>
          {fetching && <p role="status">成果物を取得中…</p>}
          {output.result!.artifacts.map((artifact, index) => (
            <div key={artifact.id}>
              {files[index] &&
                (output.result?.kind === "image" ? (
                  <img src={files[index]} alt={prompt} />
                ) : (
                  <audio controls src={files[index]} />
                ))}
              {files[index] && (
                <a
                  href={files[index]}
                  download={`${artifact.id}.${artifact.mimeType.split("/")[1] === "mpeg" ? "mp3" : artifact.mimeType.split("/")[1]}`}
                >
                  保存
                </a>
              )}
            </div>
          ))}
          {error && !fetching && (
            <button type="button" onClick={() => void retry()}>
              成果物の取得を再試行
            </button>
          )}
        </div>
      )}
    </>
  );
}
