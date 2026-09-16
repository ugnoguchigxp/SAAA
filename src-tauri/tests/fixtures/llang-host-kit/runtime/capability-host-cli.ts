// @bun
var __esm = (fn, res) => () => (fn && (res = fn(fn = 0)), res);

// src/atomic-file.ts
var init_atomic_file = () => {};

// src/contained-path.ts
import { realpath, stat } from "fs/promises";
import { isAbsolute, relative, resolve } from "path";
async function resolveContainedFile(rootPath, inputPath, label, options = {}) {
  const containmentLabel = options.containmentLabel ?? "workspace root";
  const lexicalRoot = resolve(rootPath);
  const lexicalTarget = resolve(lexicalRoot, inputPath);
  const lexicalRelative = containedRelativePath(lexicalRoot, lexicalTarget, label, containmentLabel);
  const canonicalRoot = await realpath(lexicalRoot);
  const canonicalTarget = await realpath(lexicalTarget);
  containedRelativePath(canonicalRoot, canonicalTarget, label, containmentLabel);
  if (options.rejectSymbolicLinks === true && resolve(canonicalRoot, lexicalRelative) !== canonicalTarget) {
    throw new Error(`${label} must not use a symbolic link`);
  }
  if (!(await stat(canonicalTarget)).isFile()) {
    throw new Error(`${label} must be a regular file`);
  }
  return lexicalTarget;
}
function containedRelativePath(rootPath, targetPath, label, containmentLabel = "workspace root") {
  const relation = relative(resolve(rootPath), resolve(targetPath)).replaceAll("\\", "/");
  if (relation === ".." || relation.startsWith("../") || isAbsolute(relation)) {
    throw new Error(`${label} must resolve inside the ${containmentLabel}`);
  }
  return relation;
}
var init_contained_path = () => {};

// src/wasm-contract.ts
import { createHash } from "crypto";
function digest(value) {
  return createHash("sha256").update(value).digest("hex");
}
function record(value, keys) {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new WasmError("INVALID_ARTIFACT", "expected an object");
  }
  for (const key of Object.keys(value)) {
    if (!keys.includes(key))
      throw new WasmError("INVALID_ARTIFACT", `unknown key ${key}`);
  }
  return value;
}
function parseContract(input) {
  const value = record(input, ["version", "fields"]);
  if (value.version !== 1 || !Array.isArray(value.fields) || value.fields.length === 0 || value.fields.length > WASM_LIMITS.fields) {
    throw new WasmError("UNSUPPORTED_TYPE", "invalid contract version or field count");
  }
  const fields = value.fields.map((raw) => {
    const f = record(raw, [
      "name",
      "kind",
      "values",
      "nullable",
      "undefinable",
      "optional"
    ]);
    if (typeof f.name !== "string" || !/^[A-Za-z_$][A-Za-z0-9_$]*$/.test(f.name) || f.name.length > 256 || f.kind !== "boolean" && f.kind !== "enum" && f.kind !== "string" || typeof f.nullable !== "boolean" || typeof f.undefinable !== "boolean" || typeof f.optional !== "boolean" || !Array.isArray(f.values) || f.values.length > WASM_LIMITS.values || f.values.some((v) => typeof v !== "string" || v.length > 4096)) {
      throw new WasmError("UNSUPPORTED_TYPE", "invalid field contract");
    }
    const values = f.values;
    if ((f.kind === "enum" ? values.length === 0 : values.length !== 0) || new Set(values).size !== values.length || values.some((v, i) => i > 0 && (values[i - 1] ?? "") >= v)) {
      throw new WasmError("UNSUPPORTED_TYPE", "invalid enum values");
    }
    return {
      name: f.name,
      kind: f.kind,
      values,
      nullable: f.nullable,
      undefinable: f.undefinable,
      optional: f.optional
    };
  });
  if (new Set(fields.map((f) => f.name)).size !== fields.length || fields.some((f, i) => i > 0 && (fields[i - 1]?.name ?? "") >= f.name)) {
    throw new WasmError("UNSUPPORTED_TYPE", "fields must be unique and sorted");
  }
  return { version: 1, fields };
}
function contractSlots(contract) {
  return contract.fields.flatMap((f) => {
    const slots = [];
    if (f.nullable || f.undefinable || f.optional || f.kind === "string")
      slots.push({ field: f.name, kind: "state" });
    if (f.kind !== "string")
      slots.push({ field: f.name, kind: "value" });
    return slots;
  });
}
function encodeInput(contract, input) {
  if (typeof input !== "object" || input === null || Array.isArray(input) || Object.getPrototypeOf(input) !== Object.prototype && Object.getPrototypeOf(input) !== null) {
    throw new WasmError("INVALID_INPUT", "expected a data record");
  }
  const descriptors = Object.getOwnPropertyDescriptors(input);
  if (Object.getOwnPropertySymbols(input).length || Object.entries(descriptors).some(([key, d]) => !contract.fields.some((f) => f.name === key) || !("value" in d))) {
    throw new WasmError("INVALID_INPUT", "unknown field or accessor");
  }
  const result = [];
  for (const f of contract.fields) {
    const d = Object.getOwnPropertyDescriptor(input, f.name);
    const v = d?.value;
    if (!d && !f.optional || v === undefined && !(f.undefinable || !d && f.optional) || v === null && !f.nullable) {
      throw new WasmError("INVALID_INPUT", `invalid missing/nullish field ${f.name}`);
    }
    const state = v === undefined ? 0 : v === null ? 1 : 2;
    let encoded = 0;
    if (state === 2) {
      if (f.kind === "boolean") {
        if (typeof v !== "boolean")
          throw new WasmError("INVALID_INPUT", `${f.name} must be boolean`);
        encoded = v ? 1 : 0;
      } else {
        if (typeof v !== "string")
          throw new WasmError("INVALID_INPUT", `${f.name} must be string`);
        if (f.kind === "enum") {
          encoded = f.values.indexOf(v);
          if (encoded < 0)
            throw new WasmError("INVALID_INPUT", `unknown enum ${f.name}`);
        }
      }
    }
    if (f.nullable || f.undefinable || f.optional || f.kind === "string")
      result.push(state);
    if (f.kind !== "string")
      result.push(encoded);
  }
  return result;
}
var WasmError, WASM_LIMITS;
var init_wasm_contract = __esm(() => {
  WasmError = class WasmError extends Error {
    code;
    constructor(code, message) {
      super(`${code}: ${message}`);
      this.code = code;
      this.name = "WasmError";
    }
  };
  WASM_LIMITS = { bytes: 1024 * 1024, fields: 64, values: 256 };
});

// src/wasm-artifact.ts
import { readFile, stat as stat2 } from "fs/promises";
function parseManifest(input) {
  const m = record(input, [
    "version",
    "profile",
    "export",
    "contract",
    "compiler",
    "backend",
    "options",
    "irHash",
    "provenance",
    "wasmHash",
    "file"
  ]);
  if (m.version !== 1 || m.profile !== "predicate-i32-v1" || m.export !== "evaluate" || m.options !== "mvp-no-optimization" || typeof m.compiler !== "string" || !m.compiler || typeof m.backend !== "string" || !m.backend || typeof m.irHash !== "string" || !/^[a-f0-9]{64}$/.test(m.irHash) || typeof m.wasmHash !== "string" || !/^[a-f0-9]{64}$/.test(m.wasmHash) || m.file !== `${m.wasmHash}.wasm`) {
    throw new WasmError("INVALID_ARTIFACT", "unsupported manifest or invalid hash");
  }
  const p = record(m.provenance, [
    "source",
    "concept",
    "predicate",
    "fingerprint",
    "conceptHash",
    "sourceHash",
    "typeHash",
    "testHash",
    "promptHash",
    "contextVersion",
    "contextHash"
  ]);
  const provenance = {};
  for (const k of [
    "source",
    "concept",
    "predicate",
    "fingerprint",
    "conceptHash",
    "sourceHash",
    "typeHash",
    "testHash",
    "promptHash",
    "contextHash"
  ]) {
    const v = p[k];
    if (typeof v !== "string" || !v || v.length > 4096 || (k.endsWith("Hash") || k === "fingerprint") && !/^[a-f0-9]{64}$/.test(v))
      throw new WasmError("INVALID_ARTIFACT", `invalid provenance ${k}`);
    provenance[k] = v;
  }
  if (p.contextVersion !== 1)
    throw new WasmError("INVALID_ARTIFACT", "invalid context version");
  provenance.contextVersion = 1;
  return {
    version: 1,
    profile: "predicate-i32-v1",
    export: "evaluate",
    contract: parseContract(m.contract),
    compiler: m.compiler,
    backend: m.backend,
    options: "mvp-no-optimization",
    irHash: m.irHash,
    provenance,
    wasmHash: m.wasmHash,
    file: String(m.file)
  };
}
async function readBounded(path) {
  if ((await stat2(path)).size > WASM_LIMITS.bytes)
    throw new WasmError("INVALID_ARTIFACT", "file exceeds size limit");
  const bytes = await readFile(path);
  if (bytes.length > WASM_LIMITS.bytes)
    throw new WasmError("INVALID_ARTIFACT", "file exceeds size limit");
  return new Uint8Array(bytes);
}
var init_wasm_artifact = __esm(() => {
  init_atomic_file();
  init_contained_path();
  init_wasm_contract();
});

// src/prompt-source.ts
function fail(message) {
  throw new WasmError("INVALID_PROMPT", message);
}
function stringValue(value, label) {
  if (typeof value !== "string" || !value.trim() || value.length > 4096)
    fail(`invalid ${label}`);
  return value;
}
function list(value, label) {
  if (!Array.isArray(value) || value.length > 128)
    fail(`invalid ${label}`);
  return value;
}
function identifier(value) {
  const id = stringValue(value, "id");
  if (!/^[A-Za-z][A-Za-z0-9_-]{0,63}$/.test(id))
    fail("invalid id");
  return id;
}
function unique(values) {
  if (new Set(values).size !== values.length)
    fail("duplicate id");
}
function parseRequirement(input) {
  const r = record(input, ["id", "level", "text"]);
  if (r.level !== "must" && r.level !== "must-not" && r.level !== "should")
    fail("invalid requirement level");
  return {
    id: identifier(r.id),
    level: r.level,
    text: stringValue(r.text, "requirement")
  };
}
function parseMeaning(input) {
  const s = record(input, ["intent", "requirements", "unresolvedWhen"]);
  const requirements = list(s.requirements, "requirements").map(parseRequirement);
  if (!requirements.length)
    fail("at least one requirement is required");
  unique(requirements.map((r) => r.id));
  return {
    intent: stringValue(s.intent, "intent"),
    requirements,
    unresolvedWhen: list(s.unresolvedWhen, "unresolvedWhen").map((v) => stringValue(v, "unresolvedWhen"))
  };
}
function exampleInput(example) {
  const input = { ...example.input };
  for (const key of example.undefinedFields) {
    if (Object.hasOwn(input, key))
      fail("undefined field also has a value");
    Object.defineProperty(input, key, { value: undefined, enumerable: true });
  }
  return input;
}
function parsePromptSource(input) {
  const s = record(input, [
    "version",
    "kind",
    "id",
    "intent",
    "requirements",
    "unresolvedWhen",
    "profile",
    "contract",
    "examples"
  ]);
  if (s.version !== 1 || s.kind !== "predicate" || s.profile !== "predicate-i32-v1")
    fail("unsupported source version/kind/profile");
  const meaning = parseMeaning({
    intent: s.intent,
    requirements: s.requirements,
    unresolvedWhen: s.unresolvedWhen
  });
  const contract = parseContract(s.contract);
  const examples = list(s.examples, "examples").map((v) => {
    const e = record(v, ["id", "input", "undefinedFields", "expected"]);
    if (typeof e.expected !== "boolean")
      fail("expected must be boolean");
    const input2 = record(e.input, contract.fields.map((f) => f.name));
    for (const v2 of Object.values(input2))
      if (v2 !== null && typeof v2 !== "boolean" && typeof v2 !== "string")
        fail("example values must be JSON scalar values");
    const example = {
      id: identifier(e.id),
      input: input2,
      expected: e.expected,
      undefinedFields: list(e.undefinedFields, "undefinedFields").map((v2) => stringValue(v2, "field"))
    };
    unique(example.undefinedFields);
    encodeInput(contract, exampleInput(example));
    return example;
  });
  unique(examples.map((e) => e.id));
  if (!examples.some((e) => e.expected) || !examples.some((e) => !e.expected))
    fail("provide independent positive and negative examples");
  const source = {
    version: 1,
    kind: "predicate",
    id: identifier(s.id),
    ...meaning,
    profile: "predicate-i32-v1",
    contract,
    examples
  };
  if (Buffer.byteLength(JSON.stringify(source)) > 1024 * 1024)
    fail("source exceeds size limit");
  return source;
}
function canonical(value) {
  if (Array.isArray(value))
    return `[${value.map(canonical).join(",")}]`;
  if (value !== null && typeof value === "object")
    return `{${Object.entries(value).sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0).map(([k, v]) => `${JSON.stringify(k)}:${canonical(v)}`).join(",")}}`;
  const text = JSON.stringify(value);
  if (text === undefined)
    fail("non-JSON value");
  return text;
}
function contentHash(value) {
  return digest(canonical(value));
}
var init_prompt_source = __esm(() => {
  init_atomic_file();
  init_wasm_artifact();
  init_wasm_contract();
});

// src/semantic-limits.ts
function assertKnownKeys(value, allowed, path) {
  const allowedSet = new Set(allowed);
  const unknown = Object.keys(value).find((key) => !allowedSet.has(key));
  if (unknown !== undefined) {
    throw new Error(`${path} contains unknown field ${unknown}`);
  }
}
function validateDiagnostics(input, path) {
  if (!Array.isArray(input) || !input.every((item) => typeof item === "string")) {
    throw new Error(`${path} must be an array of strings`);
  }
  if (input.length > SEMANTIC_LIMITS.diagnostics) {
    throw new Error(`${path} must contain at most ${SEMANTIC_LIMITS.diagnostics} items`);
  }
  input.forEach((diagnostic, index) => {
    if (diagnostic.length > SEMANTIC_LIMITS.diagnosticCharacters) {
      throw new Error(`${path}[${index}] must contain at most ${SEMANTIC_LIMITS.diagnosticCharacters} characters`);
    }
  });
  return input;
}
var SEMANTIC_LIMITS;
var init_semantic_limits = __esm(() => {
  SEMANTIC_LIMITS = {
    predicateExpressionNodes: 256,
    predicateExpressionDepth: 32,
    predicateConditions: 64,
    propertyPathSegments: 8,
    propertySegmentCharacters: 128,
    diagnostics: 32,
    diagnosticCharacters: 2000,
    externalJsonBytes: 2 * 1024 * 1024,
    lockBytes: 16 * 1024 * 1024,
    responseItems: 64,
    responseContentItems: 64
  };
});

// src/ir.ts
function parsePredicateExpression(input, path = "expression") {
  return parseExpression(input, path, { nodes: 0 }, 1);
}
function parseExpression(input, path, state, depth) {
  state.nodes += 1;
  if (state.nodes > SEMANTIC_LIMITS.predicateExpressionNodes) {
    throw new Error(`${path} exceeds the ${SEMANTIC_LIMITS.predicateExpressionNodes} node limit`);
  }
  if (depth > SEMANTIC_LIMITS.predicateExpressionDepth) {
    throw new Error(`${path} exceeds the ${SEMANTIC_LIMITS.predicateExpressionDepth} level depth limit`);
  }
  const value = expectRecord(input, path);
  const kind = expectString(value.kind, `${path}.kind`);
  switch (kind) {
    case "all":
    case "any": {
      assertKnownKeys(value, ["kind", "conditions"], path);
      if (!Array.isArray(value.conditions) || value.conditions.length === 0) {
        throw new Error(`${path}.conditions must be a non-empty array`);
      }
      if (value.conditions.length > SEMANTIC_LIMITS.predicateConditions) {
        throw new Error(`${path}.conditions must contain at most ${SEMANTIC_LIMITS.predicateConditions} items`);
      }
      return {
        kind,
        conditions: value.conditions.map((condition, index) => parseExpression(condition, `${path}.conditions[${index}]`, state, depth + 1))
      };
    }
    case "not":
      assertKnownKeys(value, ["kind", "condition"], path);
      return {
        kind,
        condition: parseExpression(value.condition, `${path}.condition`, state, depth + 1)
      };
    case "equals":
      assertKnownKeys(value, ["kind", "property", "value"], path);
      return {
        kind,
        property: expectPropertyPath(value.property, `${path}.property`),
        value: expectLiteral(value.value, `${path}.value`)
      };
    case "present":
      assertKnownKeys(value, ["kind", "property"], path);
      return {
        kind,
        property: expectPropertyPath(value.property, `${path}.property`)
      };
    default:
      throw new Error(`${path}.kind is not supported: ${kind}`);
  }
}
function expectRecord(value, path) {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error(`${path} must be an object`);
  }
  return value;
}
function expectString(value, path) {
  if (typeof value !== "string" || value.length === 0) {
    throw new Error(`${path} must be a non-empty string`);
  }
  return value;
}
function expectIdentifier(value, path) {
  const identifier2 = expectString(value, path);
  if (!identifierPattern.test(identifier2)) {
    throw new Error(`${path} must be a valid TypeScript identifier`);
  }
  return identifier2;
}
function expectPropertyPath(value, path) {
  if (!Array.isArray(value) || value.length === 0) {
    throw new Error(`${path} must be a non-empty array`);
  }
  if (value.length > SEMANTIC_LIMITS.propertyPathSegments) {
    throw new Error(`${path} must contain at most ${SEMANTIC_LIMITS.propertyPathSegments} segments`);
  }
  return value.map((part, index) => {
    const identifier2 = expectIdentifier(part, `${path}[${index}]`);
    if (identifier2.length > SEMANTIC_LIMITS.propertySegmentCharacters) {
      throw new Error(`${path}[${index}] must contain at most ${SEMANTIC_LIMITS.propertySegmentCharacters} characters`);
    }
    return identifier2;
  });
}
function expectLiteral(value, path) {
  if (value === null || typeof value === "string" || typeof value === "number" && Number.isFinite(value) || typeof value === "boolean") {
    return value;
  }
  throw new Error(`${path} must be a JSON primitive`);
}
var identifierPattern;
var init_ir = __esm(() => {
  init_semantic_limits();
  identifierPattern = /^[A-Za-z_$][A-Za-z0-9_$]*$/;
});

// src/elaboration-result.ts
function parseElaborationResult(input) {
  if (typeof input !== "object" || input === null || Array.isArray(input))
    throw new Error("elaboration must be an object");
  const value = input;
  assertKnownKeys(value, ["outcome", "body", "diagnostics"], "elaboration");
  const diagnostics = validateDiagnostics(value.diagnostics, "elaboration.diagnostics");
  if (value.outcome === "unresolved") {
    if (value.body !== null) {
      throw new Error("unresolved elaboration must have a null body");
    }
    return { outcome: "unresolved", body: null, diagnostics };
  }
  if (value.outcome === "resolved") {
    if (value.body === null) {
      throw new Error("resolved elaboration must have a body");
    }
    return {
      outcome: "resolved",
      body: parsePredicateExpression(value.body, "elaboration.body"),
      diagnostics
    };
  }
  throw new Error("elaboration.outcome must be resolved or unresolved");
}
var init_elaboration_result = __esm(() => {
  init_ir();
  init_semantic_limits();
});

// src/wasm-core.ts
function lowerPredicate(input, contractInput) {
  const contract = parseContract(contractInput);
  const slots = contractSlots(contract);
  const expression = parsePredicateExpression(input);
  function lower(e) {
    if ("conditions" in e)
      return { kind: e.kind, bodies: e.conditions.map(lower) };
    if (e.kind === "not")
      return { kind: "not", body: lower(e.condition) };
    if (e.property.length !== 1)
      throw new WasmError("UNSUPPORTED_TYPE", "nested paths are unsupported");
    const field = contract.fields.find((f) => f.name === e.property[0]);
    if (!field)
      throw new WasmError("INVALID_IR", "unknown field");
    const state = slots.findIndex((s) => s.field === field.name && s.kind === "state");
    const value = slots.findIndex((s) => s.field === field.name && s.kind === "value");
    if (e.kind === "present") {
      if (!(field.nullable || field.undefinable || field.optional))
        throw new WasmError("INVALID_IR", "present requires nullish field");
      return { kind: "compare", slot: state, value: 2 };
    }
    if (e.value === null) {
      if (!field.nullable)
        throw new WasmError("INVALID_IR", "null is not allowed");
      return { kind: "compare", slot: state, value: 1 };
    }
    const literal = field.kind === "boolean" && typeof e.value === "boolean" ? Number(e.value) : field.kind === "enum" && typeof e.value === "string" ? field.values.indexOf(e.value) : -1;
    if (literal < 0 || value < 0)
      throw new WasmError("UNSUPPORTED_TYPE", "unsupported equality literal");
    const equal = { kind: "compare", slot: value, value: literal };
    return state < 0 ? equal : {
      kind: "all",
      bodies: [{ kind: "compare", slot: state, value: 2 }, equal]
    };
  }
  return lower(expression);
}
var init_wasm_core = __esm(() => {
  init_ir();
  init_wasm_contract();
});

// src/prompt-resolution.ts
function evaluatePromptIR(e, input) {
  if (e.kind === "all")
    return e.conditions.every((c) => evaluatePromptIR(c, input));
  if (e.kind === "any")
    return e.conditions.some((c) => evaluatePromptIR(c, input));
  if (e.kind === "not")
    return !evaluatePromptIR(e.condition, input);
  if (!("property" in e) || e.property.length !== 1)
    fail("unsupported path");
  const value = input[e.property[0]];
  return e.kind === "present" ? value !== undefined && value !== null : value === e.value;
}
function verifyExamples(source, body) {
  lowerPredicate(body, source.contract);
  const results = source.examples.map((e) => ({
    id: e.id,
    expected: e.expected,
    actual: evaluatePromptIR(body, exampleInput(e))
  }));
  const failed = results.filter((r) => r.actual !== r.expected);
  if (failed.length)
    throw new WasmError("EXAMPLE_MISMATCH", `failed independent examples: ${failed.map((e) => e.id).join(", ")}`);
  return results;
}
function parseResolver(input) {
  const r = record(input, ["provider", "model", "responseId", "usage"]);
  let usage = null;
  if (r.usage !== null) {
    const u = record(r.usage, ["inputTokens", "outputTokens", "totalTokens"]);
    for (const key of ["inputTokens", "outputTokens", "totalTokens"])
      if (!Number.isSafeInteger(u[key]) || Number(u[key]) < 0)
        fail("invalid usage");
    usage = {
      inputTokens: Number(u.inputTokens),
      outputTokens: Number(u.outputTokens),
      totalTokens: Number(u.totalTokens)
    };
    if (usage.totalTokens !== usage.inputTokens + usage.outputTokens)
      fail("invalid total token count");
  }
  return {
    provider: stringValue(r.provider, "provider"),
    model: stringValue(r.model, "model"),
    responseId: stringValue(r.responseId, "responseId"),
    usage
  };
}
function parseStoredLock(input) {
  const r = record(input, [
    "version",
    "protocol",
    "sourceHash",
    "irHash",
    "body",
    "resolver",
    "diagnostics",
    "checksum"
  ]);
  if (r.version !== 1 || r.protocol !== RESOLUTION_PROTOCOL)
    fail("unsupported resolution protocol");
  if (typeof r.sourceHash !== "string" || !/^[a-f0-9]{64}$/.test(r.sourceHash))
    fail("invalid source hash");
  const body = parsePredicateExpression(r.body);
  if (r.irHash !== contentHash(body))
    fail("IR hash mismatch");
  const result = parseElaborationResult({
    outcome: "resolved",
    body,
    diagnostics: r.diagnostics
  });
  const unsigned = {
    version: 1,
    protocol: RESOLUTION_PROTOCOL,
    sourceHash: r.sourceHash,
    irHash: contentHash(body),
    body,
    resolver: parseResolver(r.resolver),
    diagnostics: result.diagnostics
  };
  if (r.checksum !== contentHash(unsigned))
    fail("lock checksum mismatch");
  return { ...unsigned, checksum: contentHash(unsigned) };
}
function parseResolutionLock(input, source) {
  const lock = parseStoredLock(input);
  if (lock.sourceHash !== contentHash(source))
    throw new WasmError("STALE_LOCK", "source changed; explicitly resolve again");
  verifyExamples(source, lock.body);
  return lock;
}
var RESOLUTION_PROTOCOL = "prompt-predicate-v1";
var init_prompt_resolution = __esm(() => {
  init_atomic_file();
  init_elaboration_result();
  init_ir();
  init_prompt_source();
  init_wasm_contract();
  init_wasm_core();
});

// src/capability-package.ts
import {
  link,
  lstat,
  mkdir,
  realpath as realpath2,
  rm,
  unlink,
  writeFile
} from "fs/promises";
import { basename, dirname, isAbsolute as isAbsolute2, relative as relative2, resolve as resolve2 } from "path";

// src/capability-tests.ts
init_prompt_source();
init_wasm_contract();
function invalid(message) {
  throw new WasmError("INVALID_CAPABILITY", message);
}
function parseCapabilitySuite(input, source) {
  const s = record(input, ["version", "sourceRevision", "cases"]);
  if (s.version !== 1 || s.sourceRevision !== contentHash(source))
    invalid("suite version or source revision mismatch");
  const known = new Set(source.requirements.map((r) => r.id));
  const cases = list(s.cases, "cases").map((raw) => {
    const c = record(raw, [
      "id",
      "requirementIds",
      "input",
      "undefinedFields",
      "expected"
    ]);
    const requirementIds = list(c.requirementIds, "requirementIds").map(identifier);
    unique(requirementIds);
    if (!requirementIds.length || requirementIds.some((id) => !known.has(id)))
      invalid("unknown or missing requirement id");
    if (!c.input || typeof c.input !== "object" || Array.isArray(c.input))
      invalid("test input must be an object");
    const undefinedFields = list(c.undefinedFields, "undefinedFields").map(identifier);
    unique(undefinedFields);
    const e = record(c.expected, ["kind", "value", "code"]);
    let expected;
    if (e.kind === "value" && typeof e.value === "boolean" && !Object.hasOwn(e, "code"))
      expected = { kind: "value", value: e.value };
    else if (e.kind === "error" && e.code === "INVALID_INPUT" && !Object.hasOwn(e, "value"))
      expected = { kind: "error", code: "INVALID_INPUT" };
    else
      return invalid("unsupported expectation");
    const result = {
      id: identifier(c.id),
      requirementIds,
      input: c.input,
      undefinedFields,
      expected
    };
    const value = caseInput(result);
    if (expected.kind === "value")
      encodeInput(source.contract, value);
    else {
      let rejected = false;
      try {
        encodeInput(source.contract, value);
      } catch (error) {
        if (!(error instanceof WasmError) || error.code !== "INVALID_INPUT")
          throw error;
        rejected = true;
      }
      if (!rejected)
        invalid("error expectation requires an invalid input");
    }
    return result;
  });
  unique(cases.map((c) => c.id));
  for (const value of [true, false])
    if (!cases.some((c) => c.expected.kind === "value" && c.expected.value === value))
      invalid("suite needs positive and negative cases");
  for (const r of source.requirements)
    if (r.level !== "should" && !cases.some((c) => c.requirementIds.includes(r.id)))
      invalid(`uncovered requirement ${r.id}`);
  return { version: 1, sourceRevision: contentHash(source), cases };
}
function caseInput(c) {
  return exampleInput({ ...c, id: "case", expected: false });
}
function runCapabilityCases(source, suite, evaluate) {
  const cases = [
    ...source.examples.map((c) => ({
      ...c,
      requirementIds: [],
      expected: { kind: "value", value: c.expected },
      origin: "source"
    })),
    ...suite.cases.map((c) => ({ ...c, origin: "suite" }))
  ];
  return cases.map((c) => {
    let actual;
    try {
      actual = { kind: "value", value: evaluate(caseInput(c)) };
    } catch (e) {
      actual = {
        kind: "error",
        code: e instanceof WasmError ? e.code : "EXECUTION_ERROR"
      };
    }
    const status = actual.kind === "error" && actual.code !== "INVALID_INPUT" ? "error" : JSON.stringify(actual) === JSON.stringify(c.expected) ? "pass" : "fail";
    return {
      id: c.id,
      origin: c.origin,
      requirementIds: c.requirementIds,
      expected: c.expected,
      actual,
      status
    };
  });
}

// src/capability-report.ts
init_prompt_source();
init_wasm_contract();
function expectation(input, actual = false) {
  const e = record(input, ["kind", "value", "code"]);
  if (e.kind === "value" && typeof e.value === "boolean" && !Object.hasOwn(e, "code"))
    return { kind: "value", value: e.value };
  if (e.kind === "error" && !Object.hasOwn(e, "value") && (actual || e.code === "INVALID_INPUT"))
    return { kind: "error", code: stringValue(e.code, "error code") };
  return invalid("invalid report expectation");
}
function parseCapabilityReport(input) {
  const r = record(input, [
    "version",
    "verifier",
    "packageHash",
    "status",
    "acceptance",
    "apiCalls",
    "results",
    "requirements",
    "unchecked",
    "passed",
    "failed",
    "errors",
    "diagnostics"
  ]);
  if (r.version !== 1 || r.verifier !== "capability-predicate-v1" || r.acceptance !== "not-run" || r.apiCalls !== 0 || r.packageHash !== null && (typeof r.packageHash !== "string" || !/^[a-f0-9]{64}$/.test(r.packageHash)))
    invalid("invalid report contract");
  if (!Array.isArray(r.results) || r.results.length > 256)
    invalid("invalid report results");
  const results = r.results.map((raw) => {
    const c = record(raw, [
      "id",
      "origin",
      "requirementIds",
      "expected",
      "actual",
      "status"
    ]);
    if (c.origin !== "source" && c.origin !== "suite")
      invalid("invalid test origin");
    const requirementIds = list(c.requirementIds, "requirementIds").map(identifier);
    unique(requirementIds);
    const expected = expectation(c.expected);
    const actual = expectation(c.actual, true);
    const status2 = actual.kind === "error" && actual.code !== "INVALID_INPUT" ? "error" : JSON.stringify(actual) === JSON.stringify(expected) ? "pass" : "fail";
    if (status2 !== c.status)
      invalid("inconsistent test status");
    return {
      id: identifier(c.id),
      origin: c.origin,
      requirementIds,
      expected,
      actual,
      status: status2
    };
  });
  unique(results.map((c) => `${c.origin}:${c.id}`));
  const requirements = list(r.requirements, "requirements").map((raw) => {
    const q = record(raw, ["id", "caseIds"]);
    const caseIds = list(q.caseIds, "caseIds").map(identifier);
    unique(caseIds);
    return { id: identifier(q.id), caseIds };
  });
  unique(requirements.map((q) => q.id));
  if (results.length) {
    if (r.packageHash === null)
      invalid("results require a package hash");
    for (const c of results) {
      if (c.origin === "source" ? c.requirementIds.length !== 0 : !c.requirementIds.length || c.requirementIds.some((id) => !requirements.some((q) => q.id === id)))
        invalid("invalid requirement mapping");
    }
    for (const q of requirements) {
      const ids = results.filter((c) => c.origin === "suite" && c.requirementIds.includes(q.id)).map((c) => c.id);
      if (JSON.stringify(ids) !== JSON.stringify(q.caseIds))
        invalid("inconsistent requirement mapping");
    }
  }
  if (!Array.isArray(r.unchecked) || r.unchecked.length > 130)
    invalid("invalid unchecked items");
  const unchecked = r.unchecked.map((v) => stringValue(v, "unchecked"));
  if (!unchecked.includes("SAAA acceptance"))
    invalid("missing unchecked acceptance");
  const diagnostics = list(r.diagnostics, "diagnostics").map((v) => stringValue(v, "diagnostic"));
  const passed = results.filter((c) => c.status === "pass").length;
  const failed = results.filter((c) => c.status === "fail").length;
  const errors = results.filter((c) => c.status === "error").length + diagnostics.length;
  const status = errors ? "error" : failed ? "fail" : "pass";
  if (r.passed !== passed || r.failed !== failed || r.errors !== errors || r.status !== status || !errors && (!results.length || r.packageHash === null))
    invalid("inconsistent report totals");
  return {
    version: 1,
    verifier: "capability-predicate-v1",
    packageHash: r.packageHash,
    status,
    acceptance: "not-run",
    apiCalls: 0,
    results,
    requirements,
    unchecked,
    passed,
    failed,
    errors,
    diagnostics
  };
}

// src/capability-package.ts
init_contained_path();
init_prompt_resolution();
init_prompt_source();
init_wasm_artifact();
init_wasm_contract();
var CAPABILITY_VERIFIER = "capability-predicate-v1";
var roles = ["source", "lock", "build", "wasm", "tests"];
function parseCapabilityMetadata(input) {
  const m = record(input, [
    "id",
    "release",
    "purpose",
    "useWhen",
    "doNotUseWhen"
  ]);
  return {
    id: identifier(m.id),
    release: identifier(m.release),
    purpose: stringValue(m.purpose, "purpose"),
    useWhen: stringValue(m.useWhen, "useWhen"),
    doNotUseWhen: stringValue(m.doNotUseWhen, "doNotUseWhen")
  };
}
function parseCapabilityManifest(input) {
  const m = record(input, [
    "version",
    "metadata",
    "profile",
    "output",
    "permissions",
    "files"
  ]);
  if (m.version !== 1 || m.profile !== "predicate-i32-v1" || m.output !== "boolean" || !Array.isArray(m.permissions) || m.permissions.length)
    invalid("unsupported capability contract");
  const f = record(m.files, [...roles]);
  const files = {};
  for (const role of roles) {
    const r = record(f[role], ["path", "hash"]);
    if (typeof r.path !== "string" || !/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/.test(r.path) || r.path === "capability.json")
      invalid("invalid package path");
    if (typeof r.hash !== "string" || !/^[a-f0-9]{64}$/.test(r.hash))
      invalid("invalid file hash");
    files[role] = { path: r.path, hash: r.hash };
  }
  if (new Set(roles.map((r) => files[r].path)).size !== roles.length)
    invalid("duplicate package path");
  if (files.lock.path !== `${files.source.path}.lock.json`)
    invalid("lock path must follow source path");
  return {
    version: 1,
    metadata: parseCapabilityMetadata(m.metadata),
    profile: "predicate-i32-v1",
    output: "boolean",
    permissions: [],
    files
  };
}
function json(bytes) {
  return JSON.parse(new TextDecoder().decode(bytes));
}
async function plainBytes(path) {
  const info = await lstat(path);
  if (!info.isFile() || info.isSymbolicLink())
    invalid("expected a regular non-symlink file");
  return readBounded(path);
}
async function containedBytes(root, path) {
  const file = await resolveContainedFile(root, path, "capability file", {
    rejectSymbolicLinks: true
  });
  return plainBytes(file);
}
async function readCapability(path) {
  const raw = await plainBytes(path);
  const manifest = parseCapabilityManifest(json(raw));
  const root = dirname(resolve2(path));
  const files = {};
  for (const role of roles) {
    const ref = manifest.files[role];
    files[role] = await containedBytes(root, ref.path);
    if (digest(files[role]) !== ref.hash)
      invalid(`${role} hash mismatch`);
  }
  const source = parsePromptSource(json(files.source));
  const lock = parseResolutionLock(json(files.lock), source);
  const build = parseManifest(json(files.build));
  const suite = parseCapabilitySuite(json(files.tests), source);
  if (manifest.metadata.id !== source.id || build.file !== manifest.files.wasm.path || build.wasmHash !== digest(files.wasm) || build.provenance.sourceHash !== contentHash(source) || build.provenance.fingerprint !== lock.checksum || build.irHash !== digest(JSON.stringify(lock.body)) || contentHash(build.contract) !== contentHash(source.contract) || build.provenance.testHash !== contentHash(source.examples) || build.provenance.source !== manifest.files.source.path || build.provenance.concept !== source.id || build.provenance.predicate !== source.id || build.provenance.typeHash !== contentHash(source.contract) || build.provenance.promptHash !== contentHash(RESOLUTION_PROTOCOL) || build.provenance.contextHash !== contentHash({ profile: source.profile }) || build.provenance.conceptHash !== contentHash({
    intent: source.intent,
    requirements: source.requirements,
    unresolvedWhen: source.unresolvedWhen
  }))
    invalid("source/lock/build linkage mismatch");
  if (digest(await plainBytes(path)) !== digest(raw))
    invalid("manifest changed while reading");
  return {
    manifest,
    packageHash: contentHash(manifest),
    source,
    lock,
    build,
    suite,
    bytes: files.wasm
  };
}
async function runIsolatedCapability(snapshot) {
  return new Promise((resolveResult, reject) => {
    const worker = new Worker(new URL("./capability-worker.ts", import.meta.url).href);
    const timer = setTimeout(() => {
      worker.terminate();
      reject(new WasmError("EXECUTION_TIMEOUT", "capability verification exceeded 10 seconds"));
    }, 1e4);
    function finish() {
      clearTimeout(timer);
      worker.terminate();
    }
    worker.onerror = (event) => {
      finish();
      reject(new Error(event.message));
    };
    worker.onmessage = (event) => {
      finish();
      if (event.data.error)
        reject(new Error(event.data.error));
      else
        resolveResult(event.data.results);
    };
    worker.postMessage({
      manifest: snapshot.build,
      bytes: snapshot.bytes,
      source: snapshot.source,
      suite: snapshot.suite
    });
  });
}
async function verifyCapability(path) {
  const report = {
    version: 1,
    verifier: CAPABILITY_VERIFIER,
    packageHash: null,
    status: "error",
    acceptance: "not-run",
    apiCalls: 0,
    results: [],
    requirements: [],
    unchecked: [
      "SAAA acceptance",
      "Natural-language completeness is not proven by case coverage"
    ],
    passed: 0,
    failed: 0,
    errors: 0,
    diagnostics: []
  };
  try {
    const snapshot = await readCapability(path);
    report.packageHash = snapshot.packageHash;
    report.requirements = snapshot.source.requirements.map((r) => ({
      id: r.id,
      caseIds: snapshot.suite.cases.filter((c) => c.requirementIds.includes(r.id)).map((c) => c.id)
    }));
    report.unchecked.push(...report.requirements.filter((r) => !r.caseIds.length).map((r) => `requirement:${r.id}`));
    report.results = await runIsolatedCapability(snapshot);
    if ((await readCapability(path)).packageHash !== snapshot.packageHash)
      invalid("package changed during verification");
  } catch (error) {
    report.diagnostics.push((error instanceof Error ? error.message : String(error)).slice(0, 4096) || "Unknown verification error");
  }
  report.passed = report.results.filter((r) => r.status === "pass").length;
  report.failed = report.results.filter((r) => r.status === "fail").length;
  report.errors = report.results.filter((r) => r.status === "error").length + report.diagnostics.length;
  report.status = report.errors ? "error" : report.failed ? "fail" : "pass";
  return parseCapabilityReport(report);
}

// src/capability-host.ts
init_wasm_contract();
var HOST_PROTOCOL = "llang-host-v1";
function parseHostRequest(raw) {
  const r = record(raw, [
    "protocol",
    "requestId",
    "operation",
    "packageHash",
    "input",
    "undefinedFields",
    "timeoutMs"
  ]);
  if (r.protocol !== HOST_PROTOCOL || typeof r.requestId !== "string" || !/^[a-zA-Z0-9_-]{1,128}$/.test(r.requestId) || !["inspect", "verify", "invoke"].includes(String(r.operation)) || typeof r.packageHash !== "string" || !/^[a-f0-9]{64}$/.test(r.packageHash))
    throw new Error("invalid host request");
  if (r.operation === "invoke") {
    if (!r.input || typeof r.input !== "object" || Array.isArray(r.input) || !Array.isArray(r.undefinedFields) || r.undefinedFields.some((f) => typeof f !== "string" || !/^[a-zA-Z_$][a-zA-Z0-9_$]*$/.test(f)) || new Set(r.undefinedFields).size !== r.undefinedFields.length || r.undefinedFields.some((f) => Object.hasOwn(r.input, f)) || !Number.isSafeInteger(r.timeoutMs) || Number(r.timeoutMs) < 1 || Number(r.timeoutMs) > 1e4)
      throw new Error("invalid invocation envelope");
  } else if (["input", "undefinedFields", "timeoutMs"].some((k) => Object.hasOwn(r, k)))
    throw new Error("unexpected invocation fields");
  return r;
}
async function invokeSnapshot(snapshot, input, timeoutMs) {
  return new Promise((resolve3, reject) => {
    const worker = new Worker(new URL("./capability-invoke-worker.ts", import.meta.url).href);
    const finish = () => {
      clearTimeout(timer);
      worker.terminate();
    };
    const timer = setTimeout(() => {
      finish();
      reject(new WasmError("EXECUTION_TIMEOUT", "invocation timed out"));
    }, timeoutMs);
    worker.onerror = () => {
      finish();
      reject(new Error("worker execution failed"));
    };
    worker.onmessage = (event) => {
      finish();
      if (typeof event.data.value !== "boolean" || event.data.error)
        reject(new Error("invalid worker result"));
      else
        resolve3(event.data.value);
    };
    worker.postMessage({ build: snapshot.build, bytes: snapshot.bytes, input });
  });
}
async function runHostRequest(manifestPath, raw) {
  const started = Date.now();
  let request;
  let packageHash = null;
  const base = () => ({
    protocol: HOST_PROTOCOL,
    requestId: request?.requestId ?? null,
    packageHash,
    elapsedMs: Date.now() - started,
    apiCalls: 0
  });
  try {
    request = parseHostRequest(raw);
  } catch {
    return { ...base(), status: "error", error: "invalid-request" };
  }
  try {
    const snapshot = await readCapability(manifestPath);
    packageHash = snapshot.packageHash;
    if (packageHash !== request.packageHash)
      return { ...base(), status: "error", error: "package-mismatch" };
    let result;
    if (request.operation === "invoke") {
      const input = caseInput({
        input: request.input ?? {},
        undefinedFields: request.undefinedFields ?? []
      });
      encodeInput(snapshot.source.contract, input);
      result = {
        value: await invokeSnapshot(snapshot, input, request.timeoutMs ?? 1e4)
      };
    } else if (request.operation === "verify") {
      const report = await verifyCapability(manifestPath);
      if (report.packageHash !== packageHash)
        return { ...base(), status: "error", error: "package-mismatch" };
      result = report;
    } else {
      result = {
        manifest: snapshot.manifest,
        contract: snapshot.source.contract,
        requirements: snapshot.source.requirements,
        verification: "not-run",
        acceptance: "not-run"
      };
    }
    if ((await readCapability(manifestPath)).packageHash !== packageHash)
      return { ...base(), status: "error", error: "package-mismatch" };
    return { ...base(), status: "ok", result };
  } catch (error) {
    return {
      ...base(),
      status: "error",
      error: error instanceof WasmError && error.code === "INVALID_INPUT" ? "invalid-input" : error instanceof WasmError && error.code === "EXECUTION_TIMEOUT" ? "timeout" : "execution-error"
    };
  }
}

// src/capability-host-cli.ts
if (import.meta.main) {
  try {
    if (process.argv.length !== 3)
      throw new Error("expected manifest path");
    const reader = Bun.stdin.stream().getReader();
    const chunks = [];
    let size = 0;
    for (;; ) {
      const { done, value } = await reader.read();
      if (done)
        break;
      size += value.byteLength;
      if (size > 64 * 1024) {
        await reader.cancel();
        throw new Error("stdin exceeds 64 KiB");
      }
      chunks.push(value);
    }
    const request = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    const response = JSON.stringify(await runHostRequest(process.argv[2], request));
    if (Buffer.byteLength(response) > 1024 * 1024)
      throw new Error("stdout exceeds 1 MiB");
    process.stdout.write(`${response}
`);
  } catch {
    process.stderr.write(`Invalid host transport input or output
`);
    process.exitCode = 2;
  }
}
