import { Database } from "bun:sqlite";
import { existsSync, lstatSync } from "node:fs";
import { join } from "node:path";
import { z } from "zod";
import {
  modelProvidersSettingsSchema,
  routingSettingsSchema,
  validateSettingsDocuments,
} from "../../src/lib/schemas.ts";
import { RunnerError, type CanaryManifest } from "./schema.ts";
import {
  assertCurrentOwner,
  assertNoSymlinkComponents,
  modeBits,
  sameFileObject,
  stableFileIdentity,
} from "./io.ts";

export interface SettingsState {
  larmId: string;
  directPrimary: boolean;
  larmPrimary: boolean;
}

export const databaseIdentifier = z.string().min(1).max(160).regex(/^[A-Za-z0-9_-]+$/);
export const databaseProviderId = z.string().min(1).max(80).regex(/^[A-Za-z0-9_-]+$/);
export const runtimeStatus = z.enum(["running", "completed", "failed", "cancelled", "interrupted"]);
export const runtimeRowSchema = z.object({
  id: databaseIdentifier,
  conversation_id: databaseIdentifier,
  provider_id: databaseProviderId.nullable(),
  status: runtimeStatus,
}).strict();
export const providerSessionRowSchema = z.object({
  id: databaseIdentifier,
  runtime_run_id: databaseIdentifier.nullable(),
  provider_id: databaseProviderId,
  provider_kind: z.enum(["openai-compatible", "larm"]).nullable(),
  allocation_id: databaseIdentifier.nullable(),
  selected_runtime_id: databaseIdentifier.nullable(),
  request_id: databaseIdentifier.nullable(),
  fallback_used: z.union([z.literal(0), z.literal(1)]).nullable(),
  route_id: z.string().min(1).max(80).regex(/^[A-Za-z0-9._-]+$/).nullable(),
  selection_reason: z.enum(["primary", "other"]).nullable(),
  output_started: z.union([z.literal(0), z.literal(1)]).nullable(),
  failure_kind: z.enum([
    "authentication", "contract", "protocol", "request-too-large", "internal", "client-disconnected",
    "cancelled", "partial-output", "policy", "capacity", "unavailable", "draining", "upstream", "network",
    "timeout", "allocation-lost", "allocation-outcome-unknown", "not-ready",
  ]).nullable(),
  release_status: z.enum(["not-applicable", "not-started", "pending", "released", "failed", "deferred-to-ttl"]),
  status: runtimeStatus,
}).strict();

export type RuntimeRow = z.infer<typeof runtimeRowSchema>;
export type ProviderSessionRow = z.infer<typeof providerSessionRowSchema>;

export interface DatabaseSnapshot {
  runtimeIds: Set<string>;
  sessionIds: Set<string>;
}

export interface DatabaseObservation {
  runs: RuntimeRow[];
  sessions: ProviderSessionRow[];
}

export function openCanaryDatabase(dataDirectory: string): Database {
  const filename = join(dataDirectory, "saaa.sqlite3");
  let database: Database | undefined;
  try {
    assertNoSymlinkComponents(filename);
    for (const suffix of ["-wal", "-shm", "-journal"]) {
      const sidecar = `${filename}${suffix}`;
      if (!existsSync(sidecar)) continue;
      assertNoSymlinkComponents(sidecar);
      const sidecarInfo = lstatSync(sidecar);
      if (!sidecarInfo.isFile() || sidecarInfo.isSymbolicLink() || sidecarInfo.nlink !== 1) {
        throw new Error("invalid database sidecar");
      }
      assertCurrentOwner(sidecar);
    }
    const info = lstatSync(filename);
    if (!info.isFile() || info.isSymbolicLink() || info.nlink !== 1) throw new Error("invalid database");
    const initialIdentity = stableFileIdentity(info);
    assertCurrentOwner(filename);
    database = new Database(filename, { create: false, readonly: true });
    const openedInfo = lstatSync(filename);
    if (!sameFileObject(initialIdentity, stableFileIdentity(openedInfo))) throw new Error("database changed while opening");
    database.exec("PRAGMA query_only=ON; PRAGMA busy_timeout=1000;");
    const required: Record<string, string[]> = {
      settings_documents: ["namespace", "key", "schema_version", "value_json"],
      runtime_runs: ["id", "conversation_id", "provider_id", "status"],
      provider_sessions: [
        "id", "runtime_run_id", "provider_id", "provider_kind", "allocation_id",
        "selected_runtime_id", "request_id", "fallback_used", "route_id", "selection_reason", "output_started", "failure_kind",
        "release_status", "status",
      ],
    };
    for (const [table, columns] of Object.entries(required)) {
      const entry = database.query("SELECT type FROM sqlite_master WHERE name=?1").get(table) as { type: string } | null;
      if (entry?.type !== "table") throw new Error("missing table");
      const rows = database.query(`PRAGMA table_info('${table}')`).all() as Array<{ name: string }>;
      const names = new Set(rows.map((row) => row.name));
      if (columns.some((column) => !names.has(column))) throw new Error("missing schema");
    }
    return database;
  } catch {
    database?.close();
    throw new RunnerError(2, "database-schema-invalid", "failed");
  }
}

export function settingsState(database: Database, manifest: CanaryManifest): SettingsState {
  let transactionOpen = false;
  try {
    database.exec("BEGIN");
    transactionOpen = true;
    const bound = database.query(
      `SELECT COUNT(*) AS count,
              COALESCE(MAX(length(CAST(value_json AS BLOB))),0) AS maximum,
              COALESCE(SUM(length(CAST(value_json AS BLOB))),0) AS total
       FROM settings_documents`,
    ).get() as { count: number; maximum: number; total: number } | null;
    if (
      bound === null
      || !Number.isSafeInteger(bound.count)
      || !Number.isSafeInteger(bound.maximum)
      || !Number.isSafeInteger(bound.total)
      || bound.count > 1_000
      || bound.maximum > 256 * 1_024
      || bound.total > 1024 * 1_024
    ) {
      throw new Error("settings bounds exceeded");
    }
    const rows = database.query(
      "SELECT namespace,key,schema_version,value_json FROM settings_documents ORDER BY namespace,key",
    ).all() as Array<{ namespace: string; key: string; schema_version: number; value_json: string }>;
    const documents = rows.map((row) => ({
      namespace: row.namespace,
      key: row.key,
      schemaVersion: row.schema_version,
      valueJson: JSON.parse(row.value_json) as unknown,
    }));
    validateSettingsDocuments(documents);
    const providersDocument = documents.find((document) => document.namespace === "providers.model");
    const routingDocument = documents.find((document) => document.namespace === "routing.tasks");
    if (providersDocument === undefined || routingDocument === undefined) throw new Error("missing settings");
    const providers = modelProvidersSettingsSchema.parse(providersDocument.valueJson).providers;
    const routing = routingSettingsSchema.parse(routingDocument.valueJson);
    if (providers.length !== 2) throw new Error("unexpected providers");
    const larm = providers.find((provider) => provider.kind === "larm");
    const direct = providers.find((provider) => provider.kind === "openai-compatible" && provider.id === "local-openai-compatible");
    if (
      larm === undefined
      || direct === undefined
      || !larm.enabled
      || !direct.enabled
      || larm.baseUrl !== manifest.larmProvider.baseUrl
      || larm.allocationTtlSeconds !== manifest.larmProvider.allocationTtlSeconds
      || larm.allocationStartupTimeoutSeconds !== manifest.larmProvider.allocationStartupTimeoutSeconds
      || larm.allowFallbackByDefault !== false
      || larm.deploymentPolicy !== "existing-only"
      || direct.location !== manifest.rollbackProvider.location
      || direct.endpoint !== manifest.rollbackProvider.endpoint
      || direct.model !== manifest.rollbackProvider.model
    ) {
      throw new Error("settings mismatch");
    }
    const route = routing.conversationRespond;
    const state = {
      larmId: larm.id,
      directPrimary: route.primaryProviderId === direct.id && route.fallbackProviderIds.length === 0,
      larmPrimary: route.primaryProviderId === larm.id
        && route.fallbackProviderIds.length === 1
        && route.fallbackProviderIds[0] === direct.id,
    };
    database.exec("COMMIT");
    transactionOpen = false;
    return state;
  } catch {
    if (transactionOpen) {
      try {
        database.exec("ROLLBACK");
      } catch {
        // The connection is closed by the caller; keep the strict database error.
      }
    }
    throw new RunnerError(2, "database-schema-invalid", "failed");
  }
}

export function databaseSnapshot(database: Database): DatabaseSnapshot {
  let transactionOpen = false;
  try {
    database.exec("BEGIN");
    transactionOpen = true;
    const runtimeRows = database.query("SELECT id FROM runtime_runs LIMIT 10001").all() as Array<{ id: string }>;
    const sessionRows = database.query("SELECT id FROM provider_sessions LIMIT 10001").all() as Array<{ id: string }>;
    if (runtimeRows.length > 10_000 || sessionRows.length > 10_000) throw new Error("snapshot too large");
    const parsedRuntimeRows = z.array(z.object({ id: databaseIdentifier }).strict()).max(10_000).parse(runtimeRows);
    const parsedSessionRows = z.array(z.object({ id: databaseIdentifier }).strict()).max(10_000).parse(sessionRows);
    const snapshot = {
      runtimeIds: new Set(parsedRuntimeRows.map((row) => row.id)),
      sessionIds: new Set(parsedSessionRows.map((row) => row.id)),
    };
    database.exec("COMMIT");
    transactionOpen = false;
    return snapshot;
  } catch {
    if (transactionOpen) {
      try {
        database.exec("ROLLBACK");
      } catch {
        // The connection is closed by the caller; do not replace the bounded schema error.
      }
    }
    throw new RunnerError(2, "database-schema-invalid", "failed");
  }
}

export function databaseObservation(database: Database, snapshot: DatabaseSnapshot): DatabaseObservation {
  let transactionOpen = false;
  try {
    database.exec("BEGIN");
    transactionOpen = true;
    const rawRuns = database.query(
      "SELECT id,conversation_id,provider_id,status FROM runtime_runs LIMIT 10001",
    ).all();
    const rawSessions = database.query(
      `SELECT id,runtime_run_id,provider_id,provider_kind,allocation_id,selected_runtime_id,request_id,
              fallback_used,route_id,selection_reason,output_started,failure_kind,release_status,status
       FROM provider_sessions LIMIT 10001`,
    ).all();
    if (rawRuns.length > 10_000 || rawSessions.length > 10_000) throw new Error("observation too large");
    const allRuns = z.array(runtimeRowSchema).max(10_000).parse(rawRuns);
    const allSessions = z.array(providerSessionRowSchema).max(10_000).parse(rawSessions);
    const observation = {
      runs: allRuns.filter((row) => !snapshot.runtimeIds.has(row.id)),
      sessions: allSessions.filter((row) => !snapshot.sessionIds.has(row.id)),
    };
    database.exec("COMMIT");
    transactionOpen = false;
    return observation;
  } catch {
    if (transactionOpen) {
      try {
        database.exec("ROLLBACK");
      } catch {
        // The connection is closed by the caller; do not replace the bounded schema error.
      }
    }
    throw new RunnerError(2, "database-schema-invalid", "failed");
  }
}
