import { useEffect, useMemo, useState } from "react";
import type { CalibrationParameters, SituationReviewSnapshot } from "../../../lib/contracts";
import { calibrationParametersSchema } from "../../../lib/schemas";
import {
  createSituationCalibrationCandidate,
  decideSituationCalibration,
  getSituationReviewSnapshot,
  runSituationCalibration,
} from "../../../lib/runtime";

const defaultParameters: CalibrationParameters = {
  classificationMinConfidence: 70,
  lowConfidenceMax: 45,
  enterSampleCount: 3,
  exitSampleCount: 5,
  cooldownMs: 10_000,
  inputActiveMaxMs: 30_000,
  inputRecentMaxMs: 300_000,
};

const decisionReasons = [
  "wrong-scene",
  "stale-signal",
  "unstable-transition",
  "unwanted-suggestion",
  "missed-meeting-candidate",
  "insufficient-evidence",
] as const;

type ReplayMetrics = {
  fixtureSetVersion: string;
  sampleCount: number;
  expectedSceneMatches: number;
  baselineExpectedSceneMatches: number;
  expectedAttentionSamples: number;
  expectedAttentionMatches: number;
  baselineExpectedAttentionMatches: number;
  shadowPolicyCounts: Record<string, number>;
};

export function SituationReview() {
  const [snapshot, setSnapshot] = useState<SituationReviewSnapshot | null>(null);
  const [parameters, setParameters] = useState(defaultParameters);
  const [reasonCode, setReasonCode] = useState<(typeof decisionReasons)[number]>("insufficient-evidence");
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [workingId, setWorkingId] = useState<string | null>(null);
  const replayMetrics = useMemo(
    () => decodeReplayMetrics(snapshot?.latestRun?.metricsJson ?? null),
    [snapshot?.latestRun?.metricsJson],
  );
  const parametersValid = calibrationParametersSchema.safeParse(parameters).success;
  const inputBoundariesOrdered = parameters.inputActiveMaxMs < parameters.inputRecentMaxMs;

  useEffect(() => {
    let cancelled = false;
    void getSituationReviewSnapshot()
      .then((next) => { if (!cancelled) setSnapshot(next); })
      .catch((cause) => { if (!cancelled) setError(toMessage(cause)); });
    return () => { cancelled = true; };
  }, []);

  async function replay(id: string) {
    if (workingId) return;
    setWorkingId(id);
    setError(null);
    try {
      const latestRun = await runSituationCalibration(id);
      setSnapshot((current) => current ? { ...current, latestRun } : current);
    } catch (cause) {
      setError(toMessage(cause));
    } finally {
      setWorkingId(null);
    }
  }

  async function createCandidate() {
    if (creating) return;
    setCreating(true);
    setError(null);
    try {
      await createSituationCalibrationCandidate(parameters);
      setSnapshot(await getSituationReviewSnapshot());
    } catch (cause) {
      setError(toMessage(cause));
    } finally {
      setCreating(false);
    }
  }

  async function decide(id: string, decision: "accept" | "reject" | "rollback") {
    if (workingId || !window.confirm(`${decision} this calibration profile?`)) return;
    setWorkingId(id);
    setError(null);
    try {
      setSnapshot(await decideSituationCalibration(id, decision, reasonCode));
    } catch (cause) {
      setError(toMessage(cause));
    } finally {
      setWorkingId(null);
    }
  }

  if (!snapshot) {
    return error
      ? <p className="error-banner" role="alert">{error}</p>
      : <p className="situation-loading">Reviewを読み込んでいます…</p>;
  }

  return <div className="situation-content">
    {error && <p className="error-banner" role="alert">{error}</p>}
    <section className="situation-card">
      <h2>Calibration review</h2>
      <p>Active rule: <strong>{snapshot.activeProfile.ruleVersion}</strong></p>
      <div className="settings-summary-grid">
        <div><span>Quality samples</span><strong>{snapshot.quality.sampleCount}</strong></div>
        <div><span>Pending feedback</span><strong>{snapshot.feedbackQueue.length}</strong></div>
        <div><span>Flapping rate</span><strong>{formatRate(snapshot.quality.flappingRate)}</strong></div>
        <div><span>Stale signal rate</span><strong>{formatRate(snapshot.quality.staleRate)}</strong></div>
      </div>
      {snapshot.quality.sampleCount < 20 && <p className="settings-help">Insufficient data: rates remain unavailable until 20 samples are persisted.</p>}
    </section>

    <section className="situation-card">
      <h2>Create candidate</h2>
      <div className="settings-form-grid">
        <Parameter label="Classification confidence" value={parameters.classificationMinConfidence} min={50} max={95} onChange={(value) => setParameters((current) => ({ ...current, classificationMinConfidence: value }))} />
        <Parameter label="Low-confidence maximum" value={parameters.lowConfidenceMax} min={0} max={60} onChange={(value) => setParameters((current) => ({ ...current, lowConfidenceMax: value }))} />
        <Parameter label="Enter samples" value={parameters.enterSampleCount} min={1} max={10} onChange={(value) => setParameters((current) => ({ ...current, enterSampleCount: value }))} />
        <Parameter label="Exit samples" value={parameters.exitSampleCount} min={1} max={20} onChange={(value) => setParameters((current) => ({ ...current, exitSampleCount: value }))} />
        <Parameter label="Cooldown (ms)" value={parameters.cooldownMs} min={0} max={60_000} step={500} onChange={(value) => setParameters((current) => ({ ...current, cooldownMs: value }))} />
        <Parameter label="Input active max (ms)" value={parameters.inputActiveMaxMs} min={5_000} max={120_000} step={5_000} onChange={(value) => setParameters((current) => ({ ...current, inputActiveMaxMs: value }))} />
        <Parameter label="Input recent max (ms)" value={parameters.inputRecentMaxMs} min={60_000} max={1_800_000} step={30_000} onChange={(value) => setParameters((current) => ({ ...current, inputRecentMaxMs: value }))} />
      </div>
      {!inputBoundariesOrdered && <p className="settings-help">Input active maximum must be lower than input recent maximum.</p>}
      {!parametersValid && inputBoundariesOrdered && <p className="settings-help">Check every threshold range. Low-confidence maximum must remain lower than classification confidence.</p>}
      <button className="save-button situation-toggle" disabled={!parametersValid || creating || Boolean(workingId)} onClick={() => void createCandidate()}>{creating ? "Creating…" : "Create candidate"}</button>
    </section>

    <section className="situation-card">
      <div className="section-heading">
        <h2>Profile history</h2>
        <label className="settings-field">Decision reason
          <select value={reasonCode} onChange={(event) => setReasonCode(event.currentTarget.value as typeof reasonCode)}>
            {decisionReasons.map((reason) => <option key={reason} value={reason}>{reason}</option>)}
          </select>
        </label>
      </div>
      <div className="situation-profile-list">
        {snapshot.candidates.map((profile) => <article key={profile.id}>
          <div><strong>{profile.ruleVersion}</strong><span>{profile.status}</span></div>
          <div className="profile-actions">
            {profile.status === "candidate" && <>
              <button className="secondary-button" disabled={Boolean(workingId)} onClick={() => void replay(profile.id)}>{workingId === profile.id ? "Running…" : "Replay"}</button>
              <button className="send-button" disabled={Boolean(workingId)} onClick={() => void decide(profile.id, "accept")}>Accept</button>
              <button className="stop-button" disabled={Boolean(workingId)} onClick={() => void decide(profile.id, "reject")}>Reject</button>
            </>}
            {profile.status === "superseded" && <button className="secondary-button" disabled={Boolean(workingId)} onClick={() => void decide(profile.id, "rollback")}>Rollback</button>}
          </div>
        </article>)}
      </div>
    </section>

    {snapshot.latestRun && <section className="situation-card">
      <h2>Latest replay</h2>
      {replayMetrics ? <div className="settings-summary-grid">
        <div><span>Fixture</span><strong>{replayMetrics.fixtureSetVersion}</strong></div>
        <div><span>Samples</span><strong>{replayMetrics.sampleCount}</strong></div>
        <div><span>Candidate matches</span><strong>{replayMetrics.expectedSceneMatches}</strong></div>
        <div><span>Baseline matches</span><strong>{replayMetrics.baselineExpectedSceneMatches}</strong></div>
        <div><span>Attention matches</span><strong>{replayMetrics.expectedAttentionMatches}/{replayMetrics.expectedAttentionSamples}</strong></div>
        <div><span>Baseline attention</span><strong>{replayMetrics.baselineExpectedAttentionMatches}/{replayMetrics.expectedAttentionSamples}</strong></div>
      </div> : <p className="settings-help">Replay did not produce readable comparison metrics.</p>}
    </section>}
  </div>;
}

function Parameter({ label, value, min, max, step = 1, onChange }: { label: string; value: number; min: number; max: number; step?: number; onChange: (value: number) => void }) {
  return <label className="settings-field">{label}<input type="number" value={value} min={min} max={max} step={step} onChange={(event) => { const next = event.currentTarget.valueAsNumber; if (Number.isFinite(next)) onChange(next); }} /></label>;
}

export function decodeReplayMetrics(value: string | null): ReplayMetrics | null {
  if (!value) return null;
  try {
    const parsed = JSON.parse(value) as Partial<ReplayMetrics>;
    if (
      typeof parsed.fixtureSetVersion !== "string" ||
      !validCount(parsed.sampleCount) ||
      !validCount(parsed.expectedSceneMatches) ||
      !validCount(parsed.baselineExpectedSceneMatches) ||
      !validCount(parsed.expectedAttentionSamples) ||
      !validCount(parsed.expectedAttentionMatches) ||
      !validCount(parsed.baselineExpectedAttentionMatches) ||
      parsed.expectedAttentionMatches > parsed.expectedAttentionSamples ||
      parsed.baselineExpectedAttentionMatches > parsed.expectedAttentionSamples ||
      parsed.expectedSceneMatches > parsed.sampleCount ||
      parsed.baselineExpectedSceneMatches > parsed.sampleCount ||
      typeof parsed.shadowPolicyCounts !== "object" ||
      parsed.shadowPolicyCounts === null ||
      Object.values(parsed.shadowPolicyCounts).some((count) => !validCount(count))
    ) return null;
    return parsed as ReplayMetrics;
  } catch {
    return null;
  }
}

function validCount(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}

function formatRate(value: number | null): string {
  return value === null ? "Insufficient data" : `${(value * 100).toFixed(1)}%`;
}

function toMessage(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}
