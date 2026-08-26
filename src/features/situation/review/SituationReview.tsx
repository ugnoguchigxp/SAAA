import { useEffect, useState } from "react";
import type { SituationReviewSnapshot } from "../../../lib/contracts";
import { decideSituationCalibration, getSituationReviewSnapshot, runSituationCalibration } from "../../../lib/runtime";

export function SituationReview() {
  const [snapshot, setSnapshot] = useState<SituationReviewSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => { void getSituationReviewSnapshot().then(setSnapshot).catch((cause) => setError(String(cause))); }, []);
  async function decide(id: string, decision: "accept" | "reject" | "rollback") { if (!window.confirm(`${decision} this calibration profile?`)) return; try { setSnapshot(await decideSituationCalibration(id, decision, "insufficient-evidence")); } catch (cause) { setError(String(cause)); } }
  if (error) return <p className="error-banner" role="alert">{error}</p>;
  if (!snapshot) return <p className="situation-loading">Reviewを読み込んでいます…</p>;
  return <div className="situation-content"><section className="situation-card"><h2>Calibration review</h2><p>Active rule: <strong>{snapshot.activeProfile.ruleVersion}</strong></p><p>{snapshot.quality.sampleCount < 20 ? "Insufficient data" : `Flapping ${snapshot.quality.flappingRate}`}</p><p>Pending feedback: {snapshot.feedbackQueue.length}</p></section><section className="situation-card"><h2>Profile history</h2>{snapshot.candidates.map((profile) => <article key={profile.id}><strong>{profile.ruleVersion}</strong> · {profile.status} {profile.status === "candidate" && <><button onClick={() => void runSituationCalibration(profile.id)}>Replay</button><button onClick={() => void decide(profile.id, "accept")}>Accept</button><button onClick={() => void decide(profile.id, "reject")}>Reject</button></>} {profile.status === "superseded" && <button onClick={() => void decide(profile.id, "rollback")}>Rollback</button>}</article>)}</section>{snapshot.latestRun && <section className="situation-card"><h2>Latest replay</h2><pre>{snapshot.latestRun.metricsJson}</pre></section>}</div>;
}
