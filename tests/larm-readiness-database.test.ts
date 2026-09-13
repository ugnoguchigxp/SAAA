import { afterEach, describe, expect, test } from "bun:test";
import { Database } from "bun:sqlite";
import { chmodSync, mkdtempSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  databaseObservation,
  databaseSnapshot,
  openCanaryDatabase,
  settingsState,
} from "../scripts/larm-readiness/database";
import { RunnerError, type CanaryManifest } from "../scripts/larm-readiness/schema";

const temporaryDirectories: string[] = [];

afterEach(() => {
  for (const directory of temporaryDirectories.splice(0)) rmSync(directory, { recursive: true, force: true });
});

function directory() {
  const path = realpathSync(mkdtempSync(join(tmpdir(), "saaa-larm-db-")));
  chmodSync(path, 0o700);
  temporaryDirectories.push(path);
  return path;
}

const manifest: CanaryManifest = {
  format: "saaa-larm-canary-manifest-v1",
  saaaCommit: "1234567",
  larmContractCommit: "7dca7c3",
  deploymentRevision: "revision-1",
  dataDirectory: "/tmp/data",
  metricsScope: "exclusive-window",
  larmProvider: {
    baseUrl: "http://127.0.0.1:9810",
    allocationTtlSeconds: 300,
    allocationStartupTimeoutSeconds: 300,
    allowFallbackByDefault: false,
    deploymentPolicy: "existing-only",
  },
  rollbackProvider: {
    id: "local-openai-compatible",
    location: "local",
    endpoint: "http://127.0.0.1:11434/v1",
    model: "test",
    credentialEnv: "SAAA_PROVIDER_LOCAL_OPENAI_COMPATIBLE_API_KEY",
    credentialRequired: false,
  },
};

function settingsJson(primary: "larm" | "direct") {
  const larm = {
    kind: "larm",
    id: "larm-local",
    enabled: true,
    label: "LARM",
    location: "local",
    baseUrl: "http://127.0.0.1:9810",
    tokenEnv: "LARM_API_TOKEN",
    allocationTtlSeconds: 300,
    allocationStartupTimeoutSeconds: 300,
    allowFallbackByDefault: false,
    deploymentPolicy: "existing-only",
  };
  const direct = {
    kind: "openai-compatible",
    id: "local-openai-compatible",
    enabled: true,
    label: "Local",
    location: "local",
    endpoint: "http://127.0.0.1:11434/v1",
    model: "test",
    authentication: "none",
  };
  const documents = [
    {
      namespace: "providers.model",
      key: "default",
      schema_version: 14,
      value_json: JSON.stringify({
        harness: { address: "http://127.0.0.1:9810" },
        providers: [larm, direct],
        reasoningEffort: "medium",
      }),
    },
    {
      namespace: "providers.agent",
      key: "codex-sdk",
      schema_version: 14,
      value_json: JSON.stringify({
        agentName: "SAAA",
        userName: "",
        enabled: false,
        provider: "codex-sdk",
        model: "",
        runtimeMode: "app-server",
        health: "unchecked",
        sandboxMode: "read-only",
        approvalPolicy: "never",
        networkEnabled: false,
        webSearchEnabled: false,
        workspacePolicy: "select-per-conversation",
      }),
    },
    {
      namespace: "routing.tasks",
      key: "default",
      schema_version: 14,
      value_json: JSON.stringify({
        conversationRespond: primary === "larm"
          ? { source: "provider", primaryProviderId: "larm-local", fallbackProviderIds: ["local-openai-compatible"], timeoutMs: 30_000 }
          : { source: "provider", primaryProviderId: "local-openai-compatible", fallbackProviderIds: [], timeoutMs: 30_000 },
        voiceTranscribe: { source: "harness", providerId: null, timeoutMs: 120_000 },
        voiceSpeak: { source: "harness", providerId: null, timeoutMs: 30_000 },
        codingAssist: { providerId: "codex-sdk", timeoutMs: 120_000, readOnly: true, networkEnabled: false, webSearchEnabled: false },
      }),
    },
    {
      namespace: "voice.runtime",
      key: "default",
      schema_version: 14,
      value_json: JSON.stringify({
        listeningEnabled: false,
        inputDeviceId: "default",
        outputDeviceId: "default",
        vadSensitivity: "medium",
        silenceTimeoutMs: 1500,
        allowedLanguages: ["ja"],
        autoSpeak: true,
      }),
    },
    {
      namespace: "security.runtime",
      key: "default",
      schema_version: 14,
      value_json: JSON.stringify({ localOnlyWhenSelected: true, diagnosticsRedaction: true }),
    },
    {
      namespace: "situation.runtime",
      key: "default",
      schema_version: 14,
      value_json: JSON.stringify({
        enabled: false,
        sampleIntervalMs: 2_000,
        calendarEnabled: false,
        retentionDays: 7,
        maxLedgerEntries: 10_000,
        heartbeatIntervalMs: 300_000,
        sensitiveApplicationCategories: true,
      }),
    },
    {
      namespace: "ui.preferences",
      key: "default",
      schema_version: 14,
      value_json: JSON.stringify({ language: "system", timeZone: "system", lengthUnit: "metric", weightUnit: "kilogram", currency: "JPY" }),
    },
  ];
  return documents;
}

function createDatabase(dataDirectory: string, options?: { settings?: boolean; run?: boolean }) {
  const filename = join(dataDirectory, "saaa.sqlite3");
  const database = new Database(filename);
  database.exec(`
    CREATE TABLE settings_documents (namespace TEXT, key TEXT, schema_version INTEGER, value_json TEXT);
    CREATE TABLE runtime_runs (id TEXT, conversation_id TEXT, provider_id TEXT, status TEXT);
    CREATE TABLE provider_sessions (
      id TEXT, runtime_run_id TEXT, provider_id TEXT, provider_kind TEXT, allocation_id TEXT,
      selected_runtime_id TEXT, request_id TEXT, fallback_used INTEGER, route_id TEXT, selection_reason TEXT,
      output_started INTEGER, failure_kind TEXT, release_status TEXT, status TEXT
    );
  `);
  if (options?.settings !== false) {
    const insert = database.query("INSERT INTO settings_documents (namespace, key, schema_version, value_json) VALUES (?1, ?2, ?3, ?4)");
    for (const document of settingsJson("larm")) {
      insert.run(document.namespace, document.key, document.schema_version, document.value_json);
    }
  }
  if (options?.run) {
    database.query("INSERT INTO runtime_runs (id, conversation_id, provider_id, status) VALUES (?1, ?2, ?3, ?4)")
      .run("run-1", "conversation-1", "larm-local", "completed");
    database.query(`INSERT INTO provider_sessions (
      id, runtime_run_id, provider_id, provider_kind, allocation_id, selected_runtime_id, request_id,
      fallback_used, route_id, selection_reason, output_started, failure_kind, release_status, status
    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)`).run(
      "session-1", "run-1", "larm-local", "larm", "alloc-1", "qwen-general", "req-1",
      0, "llm-default", "primary", 1, null, "released", "completed",
    );
  }
  database.close();
  chmodSync(filename, 0o600);
}

describe("LARM canary database observations", () => {
  test("opens a bounded schema and reports new runs after the snapshot", () => {
    const dataDirectory = directory();
    createDatabase(dataDirectory, { run: true });
    const database = openCanaryDatabase(dataDirectory);
    const settings = settingsState(database, { ...manifest, dataDirectory });
    expect(settings.larmId).toBe("larm-local");
    expect(settings.larmPrimary).toBe(true);
    const snapshot = databaseSnapshot(database);
    expect(snapshot.runtimeIds.has("run-1")).toBe(true);
    expect(databaseObservation(database, snapshot).runs).toEqual([]);
    database.close();
  });

  test("reports runs inserted after the snapshot was taken", () => {
    const dataDirectory = directory();
    createDatabase(dataDirectory, { run: true });
    const first = openCanaryDatabase(dataDirectory);
    const snapshot = databaseSnapshot(first);
    first.close();
    const writable = new Database(join(dataDirectory, "saaa.sqlite3"));
    writable.query("INSERT INTO runtime_runs (id, conversation_id, provider_id, status) VALUES (?1, ?2, ?3, ?4)")
      .run("run-2", "conversation-1", "larm-local", "cancelled");
    writable.close();
    const second = openCanaryDatabase(dataDirectory);
    expect(databaseObservation(second, snapshot).runs.map((row) => row.id)).toEqual(["run-2"]);
    second.close();
  });

  test("rejects settings that are not the isolated LARM canary pair", () => {
    const dataDirectory = directory();
    createDatabase(dataDirectory, { settings: false });
    const database = openCanaryDatabase(dataDirectory);
    expect(() => settingsState(database, { ...manifest, dataDirectory })).toThrow(RunnerError);
    database.close();
  });

  test("rejects a missing database", () => {
    expect(() => openCanaryDatabase(directory())).toThrow(RunnerError);
  });
});
