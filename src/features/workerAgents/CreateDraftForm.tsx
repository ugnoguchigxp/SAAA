import { useState } from "react";
import type { OutputKind, ProfileDraft } from "../../lib/generated/workerAgents";
import type { Labels } from "./labels";

const BUILTIN_TOOLS = ["web_search", "fetch_content"] as const;
const DEFAULT_INPUT_SCHEMA = '{"type":"object"}';

function parseJson(text: string): { ok: true; value: unknown } | { ok: false } {
  try {
    return { ok: true, value: JSON.parse(text) };
  } catch {
    return { ok: false };
  }
}

export function CreateDraftForm({
  labels,
  busy,
  onSave,
}: {
  labels: Labels;
  busy: boolean;
  onSave: (draft: ProfileDraft) => Promise<boolean>;
}) {
  const [open, setOpen] = useState(false);
  const [profileId, setProfileId] = useState("");
  const [purpose, setPurpose] = useState("");
  const [systemContext, setSystemContext] = useState("");
  const [tools, setTools] = useState<string[]>([]);
  const [outputKind, setOutputKind] = useState<OutputKind>("web_claims_v1");
  const [inputSchema, setInputSchema] = useState(DEFAULT_INPUT_SCHEMA);
  const [outputSchema, setOutputSchema] = useState("");
  const [formError, setFormError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);

  if (!open) {
    return (
      <button type="button" onClick={() => setOpen(true)}>
        {labels.createToggle}
      </button>
    );
  }

  async function submit() {
    setSaved(false);
    const input = parseJson(inputSchema);
    const output = outputKind === "json_v1" ? parseJson(outputSchema) : null;
    if (!input.ok || (output && !output.ok)) {
      setFormError(labels.invalidJson);
      return;
    }
    setFormError(null);
    const ok = await onSave({
      profileId,
      purpose,
      systemContext,
      skillRevisionIds: [],
      tools: tools.map((key) => ({ kind: "builtin", key, catalogRevisionId: null })),
      inputSchema: input.value,
      outputKind,
      outputSchema: output?.ok ? output.value : null,
      completion: { minItems: 1, sourcesMustBeHostRecorded: outputKind === "web_claims_v1" },
      limits: {
        maxSteps: 6,
        maxSameTierRetries: 1,
        deadlineMs: 40_000,
        syncWaitMs: 15_000,
        maxRestarts: 1,
      },
      tierPolicy: { maxTier: "local", cloud: "never" },
    });
    setSaved(ok);
  }

  return (
    <form
      className="wa-form"
      onSubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
    >
      <label>
        {labels.profileId}
        <input value={profileId} onChange={(event) => setProfileId(event.target.value)} />
      </label>
      <label>
        {labels.purposeInput}
        <textarea value={purpose} onChange={(event) => setPurpose(event.target.value)} />
      </label>
      <label>
        {labels.systemContext}
        <textarea
          value={systemContext}
          onChange={(event) => setSystemContext(event.target.value)}
        />
      </label>
      <fieldset>
        <legend>{labels.toolsField}</legend>
        {BUILTIN_TOOLS.map((key) => (
          <label key={key} className="wa-inline">
            <input
              type="checkbox"
              checked={tools.includes(key)}
              onChange={(event) =>
                setTools(event.target.checked ? [...tools, key] : tools.filter((t) => t !== key))
              }
            />
            {key}
          </label>
        ))}
      </fieldset>
      <label>
        {labels.outputKind}
        <select
          value={outputKind}
          onChange={(event) => setOutputKind(event.target.value as OutputKind)}
        >
          <option value="web_claims_v1">web_claims_v1</option>
          <option value="json_v1">json_v1</option>
        </select>
      </label>
      <label>
        {labels.inputSchema}
        <textarea value={inputSchema} onChange={(event) => setInputSchema(event.target.value)} />
      </label>
      {outputKind === "json_v1" ? (
        <label>
          {labels.outputSchema}
          <textarea
            value={outputSchema}
            onChange={(event) => setOutputSchema(event.target.value)}
          />
        </label>
      ) : null}
      {formError ? (
        <p className="wa-error" role="alert">
          {formError}
        </p>
      ) : null}
      {saved ? <p className="wa-muted">{labels.saved}</p> : null}
      <div className="wa-actions">
        <button type="submit" className="wa-primary" disabled={busy}>
          {labels.save}
        </button>
        <button type="button" onClick={() => setOpen(false)}>
          {labels.cancel}
        </button>
      </div>
    </form>
  );
}
