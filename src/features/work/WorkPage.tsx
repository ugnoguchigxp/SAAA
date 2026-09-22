import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { AppIcon } from "../../components/AppIcon";
import { INTERACTIVE_HTML_FIXTURE } from "../chat/artifacts/artifactPreviewApi";
import { useArtifactWorkspace } from "../chat/artifacts/ArtifactDrawer";
import { uiApi } from "../chat/ui/api";
import { codingApi, type CodingSettings, type CodingSnapshot } from "../coding/api";
import { stewardApi, stewardErrorMessage } from "../coding/stewardApi";
import { workApi, type ArtifactSummary } from "./api";
import "../workspacePages.css";

type StewardTask = Awaited<ReturnType<typeof stewardApi.listTasks>>[number];
type WorkTab = "queue" | "completed";

const terminalStates = new Set(["done", "failed", "cancelled", "outcome_unknown"]);
const terminalJobStates = new Set(["settled", "failed", "interrupted", "outcome_unknown"]);

function formatTime(value: string, locale: string): string {
  const numeric = Number(value);
  const date = new Date(Number.isFinite(numeric) ? numeric : value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString(locale);
}

export function WorkPage({
  conversationId,
  onOpenSettings,
}: {
  conversationId?: string;
  onOpenSettings: () => void;
}) {
  const { t, i18n } = useTranslation();
  const artifactWorkspace = useArtifactWorkspace();
  const [tab, setTab] = useState<WorkTab>("queue");
  const [tasks, setTasks] = useState<StewardTask[]>([]);
  const [coding, setCoding] = useState<CodingSettings | null>(null);
  const [snapshot, setSnapshot] = useState<CodingSnapshot | null>(null);
  const [artifacts, setArtifacts] = useState<ArtifactSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [savingPermission, setSavingPermission] = useState(false);
  const [openingArtifact, setOpeningArtifact] = useState<string | null>(null);
  const [error, setError] = useState("");

  const refresh = useCallback(async () => {
    if (!conversationId) {
      setLoading(false);
      return;
    }
    try {
      const [nextTasks, nextCoding, nextSnapshot, nextArtifacts] = await Promise.all([
        stewardApi.listTasks(conversationId),
        codingApi.settings(),
        codingApi.snapshot(conversationId),
        workApi.artifacts(),
      ]);
      setTasks(nextTasks);
      setCoding(nextCoding);
      setSnapshot(nextSnapshot);
      setArtifacts(nextArtifacts);
      setError("");
    } catch (cause) {
      setError(stewardErrorMessage(cause));
    } finally {
      setLoading(false);
    }
  }, [conversationId]);

  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => void refresh(), 2_000);
    return () => window.clearInterval(timer);
  }, [refresh]);

  const queue = useMemo(
    () =>
      tasks
        .filter((task) => !terminalStates.has(task.loopState))
        .sort((a, b) => a.queueRank - b.queueRank || a.createdAt.localeCompare(b.createdAt)),
    [tasks],
  );
  const completed = useMemo(
    () =>
      tasks
        .filter((task) => terminalStates.has(task.loopState))
        .sort((a, b) => b.updatedAt.localeCompare(a.updatedAt)),
    [tasks],
  );
  const taskJobIds = useMemo(
    () => new Set(tasks.flatMap((task) => (task.codingJobId ? [task.codingJobId] : []))),
    [tasks],
  );
  const standaloneJobs = useMemo(
    () => (snapshot?.jobs ?? []).filter((job) => !taskJobIds.has(job.jobId)),
    [snapshot, taskJobIds],
  );
  const activeStandaloneJobs = standaloneJobs.filter((job) => !terminalJobStates.has(job.state));
  const completedStandaloneJobs = standaloneJobs.filter((job) => terminalJobStates.has(job.state));

  async function toggleCoding(enabled: boolean) {
    if (!coding) return;
    setSavingPermission(true);
    try {
      const next = { ...coding, enabled };
      await codingApi.save(next);
      setCoding(next);
      setError("");
    } catch (cause) {
      setError(String(cause));
    } finally {
      setSavingPermission(false);
    }
  }

  async function move(taskId: string, delta: -1 | 1) {
    if (!conversationId) return;
    const reorderable = queue.filter((task) => task.loopState === "queued");
    const index = reorderable.findIndex((task) => task.taskId === taskId);
    const target = index + delta;
    if (index < 0 || target < 0 || target >= reorderable.length) return;
    const ordered = reorderable.map((task) => task.taskId);
    [ordered[index], ordered[target]] = [ordered[target], ordered[index]];
    try {
      await stewardApi.reorderQueue(conversationId, ordered);
      await refresh();
    } catch (cause) {
      setError(stewardErrorMessage(cause));
    }
  }

  async function openArtifact(artifact: ArtifactSummary) {
    setOpeningArtifact(artifact.instanceId);
    try {
      const instance = await uiApi.load(artifact.instanceId);
      artifactWorkspace?.open(instance, artifact.conversationId);
      setError("");
    } catch (cause) {
      setError(String(cause));
    } finally {
      setOpeningArtifact(null);
    }
  }

  return (
    <section className="workspace-page work-page" aria-label={t("navigation.work")}>
      <div
        className="workspace-page-header workspace-page-toolbar"
        role="toolbar"
        aria-label={t("navigation.work")}
      >
        <button
          type="button"
          className="workspace-secondary-button workspace-symbol-button"
          aria-label={t("common.refresh")}
          title={t("common.refresh")}
          onClick={() => void refresh()}
        >
          <AppIcon name="refresh" />
        </button>
      </div>
      <div className="workspace-page-content work-page-content">
        <div className="work-toolbar">
          <div className="workspace-tabs" role="tablist" aria-label={t("workPage.tabsLabel")}>
            <button
              type="button"
              role="tab"
              aria-selected={tab === "queue"}
              onClick={() => setTab("queue")}
            >
              {t("workPage.queue")} <span>{queue.length + activeStandaloneJobs.length}</span>
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={tab === "completed"}
              onClick={() => setTab("completed")}
            >
              {t("workPage.completed")}{" "}
              <span>{completed.length + completedStandaloneJobs.length + artifacts.length}</span>
            </button>
          </div>
          <button
            type="button"
            className="workspace-text-button"
            onClick={() =>
              artifactWorkspace?.openInteractivePreview({
                artifactId: INTERACTIVE_HTML_FIXTURE.artifactId,
                revisionId: INTERACTIVE_HTML_FIXTURE.revisionId,
                title: t("genui.openInteractivePreview"),
              })
            }
          >
            {t("genui.openInteractivePreview")}
          </button>
          {coding ? (
            <label className="work-permission-toggle">
              <input
                type="checkbox"
                checked={coding.enabled}
                disabled={savingPermission}
                onChange={(event) => void toggleCoding(event.currentTarget.checked)}
              />
              {coding.profile === "codex-sdk-v1"
                ? t("workPage.codexPermission")
                : t("workPage.toolPermission")}
            </label>
          ) : null}
        </div>
        {coding && coding.profile !== "codex-sdk-v1" ? (
          <div className="work-settings-notice">
            <span>{t("workPage.notCodexProfile")}</span>
            <button type="button" className="workspace-text-button" onClick={onOpenSettings}>
              {t("navigation.settings")}
            </button>
          </div>
        ) : null}
        {error ? (
          <p className="workspace-error" role="alert">
            {error}
          </p>
        ) : null}
        {loading ? <p className="workspace-empty">{t("common.loading")}</p> : null}

        {tab === "queue" && !loading ? (
          queue.length || activeStandaloneJobs.length ? (
            <div className="work-list" role="list">
              {queue.map((task, index) => (
                <article className="work-card" role="listitem" key={task.taskId}>
                  <div className="work-rank-controls" aria-label={t("workPage.reorderLabel")}>
                    <span>{index + 1}</span>
                    <button
                      type="button"
                      disabled={
                        task.loopState !== "queued" ||
                        queue
                          .filter((item) => item.loopState === "queued")
                          .findIndex((item) => item.taskId === task.taskId) === 0
                      }
                      onClick={() => void move(task.taskId, -1)}
                      aria-label={t("workPage.moveUp")}
                    >
                      ↑
                    </button>
                    <button
                      type="button"
                      disabled={
                        task.loopState !== "queued" ||
                        queue
                          .filter((item) => item.loopState === "queued")
                          .findIndex((item) => item.taskId === task.taskId) ===
                          queue.filter((item) => item.loopState === "queued").length - 1
                      }
                      onClick={() => void move(task.taskId, 1)}
                      aria-label={t("workPage.moveDown")}
                    >
                      ↓
                    </button>
                  </div>
                  <div className="work-card-main">
                    <div>
                      <strong>{task.summary || task.goalId}</strong>
                      <span className="state-chip">{task.loopState}</span>
                    </div>
                    <p>
                      {task.workspaceId} ·{" "}
                      {formatTime(task.createdAt, i18n.resolvedLanguage ?? i18n.language)}
                    </p>
                    {task.evidenceReason ? <p>{task.evidenceReason}</p> : null}
                  </div>
                </article>
              ))}
              {activeStandaloneJobs.map((job) => (
                <article className="work-card" role="listitem" key={job.jobId}>
                  <div className="work-rank-controls">
                    <span>—</span>
                  </div>
                  <div className="work-card-main">
                    <div>
                      <strong>{job.workspace}</strong>
                      <span className="state-chip">{job.state}</span>
                    </div>
                    <p>{job.result?.summary ?? job.jobId}</p>
                  </div>
                </article>
              ))}
            </div>
          ) : (
            <p className="workspace-empty">{t("workPage.queueEmpty")}</p>
          )
        ) : null}

        {tab === "completed" && !loading ? (
          <div className="completed-layout">
            <section>
              <h2>{t("workPage.finishedTasks")}</h2>
              {completed.length ? (
                <div className="work-list" role="list">
                  {completed.map((task) => (
                    <article className="work-card completed" role="listitem" key={task.taskId}>
                      <div className="work-card-main">
                        <div>
                          <strong>{task.summary || task.goalId}</strong>
                          <span className="state-chip">{task.loopState}</span>
                        </div>
                        <p>{formatTime(task.updatedAt, i18n.resolvedLanguage ?? i18n.language)}</p>
                        {task.evidenceReason ? <p>{task.evidenceReason}</p> : null}
                        {task.artifactRefs.length ? (
                          <p>
                            {t("workPage.references", { count: task.artifactRefs.length })}:{" "}
                            {task.artifactRefs.join(", ")}
                          </p>
                        ) : null}
                      </div>
                    </article>
                  ))}
                </div>
              ) : null}
              {completedStandaloneJobs.length ? (
                <div className="work-list" role="list">
                  {completedStandaloneJobs.map((job) => (
                    <article className="work-card completed" role="listitem" key={job.jobId}>
                      <div className="work-card-main">
                        <div>
                          <strong>{job.workspace}</strong>
                          <span className="state-chip">{job.state}</span>
                        </div>
                        <p>{job.result?.summary ?? job.result?.error ?? job.jobId}</p>
                      </div>
                    </article>
                  ))}
                </div>
              ) : null}
              {!completed.length && !completedStandaloneJobs.length ? (
                <p className="workspace-empty">{t("workPage.completedEmpty")}</p>
              ) : null}
            </section>
            <section>
              <h2>{t("workPage.artifacts")}</h2>
              {artifacts.length ? (
                <div className="artifact-result-grid">
                  {artifacts.map((artifact) => (
                    <button
                      key={artifact.instanceId}
                      type="button"
                      onClick={() => void openArtifact(artifact)}
                      disabled={openingArtifact === artifact.instanceId}
                    >
                      <strong>{artifact.name ?? artifact.summary}</strong>
                      <span>
                        {formatTime(artifact.createdAt, i18n.resolvedLanguage ?? i18n.language)}
                      </span>
                      <small>{artifact.summary}</small>
                    </button>
                  ))}
                </div>
              ) : (
                <p className="workspace-empty">{t("workPage.artifactsEmpty")}</p>
              )}
            </section>
          </div>
        ) : null}
        {snapshot?.workspace === null && tab === "queue" && !loading ? (
          <div className="work-settings-notice">
            <span>{t("workPage.workspaceMissing")}</span>
            <button type="button" className="workspace-text-button" onClick={onOpenSettings}>
              {t("navigation.settings")}
            </button>
          </div>
        ) : null}
      </div>
    </section>
  );
}
