import { terminalErrorMessage } from "./terminalErrors";
import { useState } from "react";
import { z } from "zod";
import { codingApi } from "./api";
const nativeQuestions = z
  .array(
    z.object({
      question: z.string(),
      options: z
        .array(z.object({ label: z.string(), description: z.string().optional() }))
        .optional(),
      multiSelect: z.boolean().optional(),
    }),
  )
  .max(8);
export type TerminalQuestionData = {
  questionId: string;
  kind: string;
  state: string;
  input: Record<string, unknown>;
};
export function TerminalQuestion({
  question,
  conversationId,
  jobId,
  revision,
  onSaved,
  onError,
}: {
  question: TerminalQuestionData;
  conversationId: string;
  jobId: string;
  revision: number;
  onSaved: () => void;
  onError: (error: string) => void;
}) {
  const [text, setText] = useState("");
  const [answers, setAnswers] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const parsed = nativeQuestions.safeParse(question.input.questions);
  const native = parsed.success ? parsed.data : [];
  async function send(answer: unknown) {
    setBusy(true);
    try {
      await codingApi.answer(conversationId, jobId, revision, question.questionId, answer);
      onSaved();
    } catch (error) {
      onError(terminalErrorMessage(error));
    } finally {
      setBusy(false);
    }
  }
  const ready = question.state === "awaiting_user";
  if (question.kind === "permission")
    return (
      <section aria-label="操作の確認">
        <p>この操作を許可しますか？ この1回だけに適用します。</p>
        <pre>{JSON.stringify(question.input, null, 2)}</pre>
        <button disabled={!ready || busy} onClick={() => void send("approve")}>
          この操作を許可
        </button>
        <button disabled={!ready || busy} onClick={() => void send("deny")}>
          許可せず停止
        </button>
        {!ready && <p>CLIの停止を確認しています。</p>}
      </section>
    );
  return (
    <section aria-label="実装中の質問">
      {native.length ? (
        native.map((q) => (
          <label key={q.question}>
            {q.question}
            <input
              value={answers[q.question] ?? ""}
              onChange={(e) => setAnswers({ ...answers, [q.question]: e.target.value })}
            />
            {q.options?.map((option) => (
              <button
                key={option.label}
                onClick={() => setAnswers({ ...answers, [q.question]: option.label })}
              >
                {option.label}
              </button>
            ))}
          </label>
        ))
      ) : (
        <label>
          {typeof question.input.question === "string"
            ? question.input.question
            : "作業を進めるために回答してください"}
          <textarea value={text} onChange={(e) => setText(e.target.value)} />
          {Array.isArray(question.input.options) &&
            question.input.options
              .filter((v): v is string => typeof v === "string")
              .map((option) => (
                <button key={option} onClick={() => setText(option)}>
                  {option}
                </button>
              ))}
        </label>
      )}
      <button
        disabled={
          !ready ||
          busy ||
          (native.length ? native.some((q) => !answers[q.question]?.trim()) : !text.trim())
        }
        onClick={() => void send(native.length ? answers : text)}
      >
        回答して続行
      </button>
      {!ready && <p>CLIの停止を確認しています。</p>}
    </section>
  );
}
