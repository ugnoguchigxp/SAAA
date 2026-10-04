import { terminalErrorMessage } from "./terminalErrors";
import { codingApi, type CodingSnapshot } from "./api";
import { TerminalQuestion } from "./TerminalQuestion";
import { TerminalVerification } from "./TerminalVerification";
export function CodingJobActions({
  conversationId,
  job,
  onSaved,
  onError,
}: {
  conversationId: string;
  job: CodingSnapshot["jobs"][number];
  onSaved: () => void;
  onError: (error: string) => void;
}) {
  async function action(run: () => Promise<unknown>) {
    try {
      await run();
      onSaved();
    } catch (error) {
      onError(terminalErrorMessage(error));
    }
  }
  return (
    <div className="coding-job-actions">
      {job.result?.error && <p role="alert">{job.result.error}</p>}
      {job.terminal && (
        <>
          <button onClick={() => void action(() => codingApi.progress(conversationId, job.jobId))}>
            進捗の端末を開く
          </button>
          {job.terminal.phase === "review" && (
            <button
              onClick={() =>
                void action(() => codingApi.complete(conversationId, job.jobId, job.revision))
              }
            >
              結果を確認し、完了として受け入れる
            </button>
          )}
          {job.terminal.questions.map((question) => (
            <TerminalQuestion
              key={question.questionId}
              question={question}
              conversationId={conversationId}
              jobId={job.jobId}
              revision={job.revision}
              onSaved={onSaved}
              onError={onError}
            />
          ))}
          {job.terminal.verification != null && (
            <TerminalVerification value={job.terminal.verification} />
          )}
          {job.state === "outcome_unknown" && (
            <button
              onClick={() =>
                void action(() => codingApi.recover(conversationId, job.jobId, job.revision))
              }
            >
              状態を確認して停止
            </button>
          )}
        </>
      )}
      {["queued", "running", "awaiting_user"].includes(job.state) && (
        <button
          onClick={() =>
            void action(() => codingApi.cancel(conversationId, job.jobId, job.revision))
          }
        >
          実装を停止
        </button>
      )}
    </div>
  );
}
