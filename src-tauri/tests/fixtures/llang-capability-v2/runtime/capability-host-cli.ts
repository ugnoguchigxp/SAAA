// @bun
var __esm = (fn, res) => () => (fn && (res = fn(fn = 0)), res);

// ../L-Lang/src/atomic-file.ts
var init_atomic_file = () => {};

// ../L-Lang/src/contained-path.ts
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

// ../L-Lang/src/unicode-length.ts
function unicodeScalarLength(value) {
  return [...value].length;
}

// ../L-Lang/src/wasm-contract.ts
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
    if (typeof f.name !== "string" || !/^[A-Za-z_$][A-Za-z0-9_$]*$/.test(f.name) || f.name.length > 256 || f.kind !== "boolean" && f.kind !== "enum" && f.kind !== "string" || typeof f.nullable !== "boolean" || typeof f.undefinable !== "boolean" || typeof f.optional !== "boolean" || !Array.isArray(f.values) || f.values.length > WASM_LIMITS.values || f.values.some((v) => typeof v !== "string" || unicodeScalarLength(v) > 4096)) {
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

// ../L-Lang/src/wasm-artifact.ts
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

// ../L-Lang/src/prompt-source.ts
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

// ../L-Lang/src/semantic-limits.ts
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
    typescriptSourceBytes: 2 * 1024 * 1024,
    externalJsonBytes: 2 * 1024 * 1024,
    lockBytes: 16 * 1024 * 1024,
    responseItems: 64,
    responseContentItems: 64
  };
});

// ../L-Lang/src/ir.ts
function parsePredicateExpression(input, path = "expression") {
  return parseExpression(input, path, { nodes: 0 }, 1, [path]);
}
function parseExpression(input, path, state, depth, diagnosticPath) {
  state.nodes += 1;
  if (state.nodes > SEMANTIC_LIMITS.predicateExpressionNodes) {
    throw new PredicateProfileError(`${path} exceeds the ${SEMANTIC_LIMITS.predicateExpressionNodes} node limit`);
  }
  if (depth > SEMANTIC_LIMITS.predicateExpressionDepth) {
    throw new PredicateProfileError(`${path} exceeds the ${SEMANTIC_LIMITS.predicateExpressionDepth} level depth limit`);
  }
  const value = expectRecord(input, path);
  const kind = expectString(value.kind, `${path}.kind`);
  switch (kind) {
    case "all":
    case "any": {
      assertExpressionKeys(value, ["kind", "conditions"], path, diagnosticPath);
      if (!Array.isArray(value.conditions) || value.conditions.length === 0) {
        throw new Error(`${path}.conditions must be a non-empty array`);
      }
      if (value.conditions.length > SEMANTIC_LIMITS.predicateConditions) {
        throw new PredicateProfileError(`${path}.conditions must contain at most ${SEMANTIC_LIMITS.predicateConditions} items`);
      }
      return {
        kind,
        conditions: value.conditions.map((condition, index) => parseExpression(condition, `${path}.conditions[${index}]`, state, depth + 1, [...diagnosticPath, "conditions", index]))
      };
    }
    case "not":
      assertExpressionKeys(value, ["kind", "condition"], path, diagnosticPath);
      return {
        kind,
        condition: parseExpression(value.condition, `${path}.condition`, state, depth + 1, [...diagnosticPath, "condition"])
      };
    case "equals":
      assertExpressionKeys(value, ["kind", "property", "value"], path, diagnosticPath);
      return {
        kind,
        property: expectPropertyPath(value.property, `${path}.property`),
        value: expectLiteral(value.value, `${path}.value`)
      };
    case "present":
      assertExpressionKeys(value, ["kind", "property"], path, diagnosticPath);
      return {
        kind,
        property: expectPropertyPath(value.property, `${path}.property`)
      };
    default:
      throw new Error(`${path}.kind is not supported: ${kind}`);
  }
}
function assertExpressionKeys(value, allowed, path, diagnosticPath) {
  const allowedSet = new Set(allowed);
  const unknown = Object.keys(value).find((key) => !allowedSet.has(key));
  if (unknown !== undefined)
    throw new PredicateStructureError(`${path} contains unknown field ${unknown}`, [...diagnosticPath, unknown]);
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
var PredicateProfileError, PredicateStructureError, identifierPattern;
var init_ir = __esm(() => {
  init_semantic_limits();
  PredicateProfileError = class PredicateProfileError extends Error {
  };
  PredicateStructureError = class PredicateStructureError extends Error {
    diagnosticPath;
    constructor(message, diagnosticPath) {
      super(message);
      this.diagnosticPath = diagnosticPath;
    }
  };
  identifierPattern = /^[A-Za-z_$][A-Za-z0-9_$]*$/;
});

// ../L-Lang/src/elaboration-result.ts
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

// ../L-Lang/src/stable-hash.ts
import { createHash as createHash2 } from "crypto";
function fingerprintFor(input) {
  return sha256(stableJson(input));
}
function sha256(value) {
  return createHash2("sha256").update(value).digest("hex");
}
function stableJson(value) {
  if (Array.isArray(value))
    return `[${value.map(stableJson).join(",")}]`;
  if (typeof value === "object" && value !== null) {
    const record2 = value;
    return `{${Object.keys(record2).sort().map((key) => `${JSON.stringify(key)}:${stableJson(record2[key])}`).join(",")}}`;
  }
  return JSON.stringify(value);
}
var init_stable_hash = () => {};

// ../L-Lang/src/canonical-type-ir.ts
var init_canonical_type_ir = __esm(() => {
  init_stable_hash();
  init_wasm_contract();
});

// ../L-Lang/src/wasm-core.ts
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
  init_canonical_type_ir();
  init_wasm_contract();
});

// ../L-Lang/src/prompt-resolution.ts
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

// ../L-Lang/src/wasm-runtime.ts
async function instantiateWasmPredicate(manifest, bytes) {
  if (digest(bytes) !== manifest.wasmHash)
    throw new WasmError("ARTIFACT_MISMATCH", "Wasm digest differs from manifest");
  assertStatelessWasmBinary(bytes);
  const module = await WebAssembly.compile(bytes);
  const bindings = WebAssembly.Module.customSections(module, "llang.contract");
  if (bindings.length !== 1 || new TextDecoder().decode(bindings[0]) !== digest(JSON.stringify(manifest.contract)))
    throw new WasmError("ARTIFACT_MISMATCH", "ABI contract does not match Wasm");
  const exports = WebAssembly.Module.exports(module);
  if (WebAssembly.Module.imports(module).length || exports.length !== 1 || exports[0]?.name !== manifest.export || exports[0]?.kind !== "function") {
    throw new WasmError("INVALID_ARTIFACT", "unexpected import/export contract");
  }
  const instance = await WebAssembly.instantiate(module, {});
  const fn = instance.exports[manifest.export];
  if (typeof fn !== "function" || fn.length !== contractSlots(manifest.contract).length)
    throw new WasmError("INVALID_ARTIFACT", "invalid evaluate signature");
  return {
    evaluate(input) {
      const result = fn(...encodeInput(manifest.contract, input));
      if (result !== 0 && result !== 1)
        throw new WasmError("INVALID_ARTIFACT", "expected boolean result");
      return result === 1;
    }
  };
}
function assertStatelessWasmBinary(bytes) {
  if (bytes.length < 8 || bytes[0] !== 0 || bytes[1] !== 97 || bytes[2] !== 115 || bytes[3] !== 109 || bytes[4] !== 1 || bytes[5] !== 0 || bytes[6] !== 0 || bytes[7] !== 0) {
    throw new WasmError("INVALID_ARTIFACT", "invalid Wasm header");
  }
  const forbiddenSections = new Set([4, 5, 6, 8, 9, 11, 12]);
  let offset = 8;
  while (offset < bytes.length) {
    const section = bytes[offset];
    if (section === undefined || section > 12) {
      throw new WasmError("INVALID_ARTIFACT", "invalid Wasm section");
    }
    offset += 1;
    const size = readVarUint32(bytes, offset);
    offset = size.next;
    if (forbiddenSections.has(section)) {
      throw new WasmError("INVALID_ARTIFACT", "stateful Wasm sections are not allowed");
    }
    offset += size.value;
    if (offset > bytes.length) {
      throw new WasmError("INVALID_ARTIFACT", "truncated Wasm section");
    }
  }
}
function readVarUint32(bytes, start) {
  let value = 0;
  let shift = 0;
  for (let offset = start;offset < bytes.length && shift <= 28; offset++) {
    const byte = bytes[offset];
    if (byte === undefined)
      break;
    value |= (byte & 127) << shift;
    if ((byte & 128) === 0)
      return { value: value >>> 0, next: offset + 1 };
    shift += 7;
  }
  throw new WasmError("INVALID_ARTIFACT", "invalid Wasm section size");
}
var init_wasm_runtime = __esm(() => {
  init_wasm_artifact();
  init_wasm_contract();
});

// ../L-Lang/src/capability-package.ts
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

// ../L-Lang/src/capability-tests.ts
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

// ../L-Lang/src/capability-report.ts
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

// ../L-Lang/src/capability-package.ts
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

// ../L-Lang/src/llang-capability-contracts.ts
import { lstat as lstat2, readFile as readFile2 } from "fs/promises";
import { dirname as dirname2, resolve as resolve3 } from "path";

// ../L-Lang/src/llang-artifact.ts
init_contained_path();
init_wasm_artifact();
init_wasm_contract();

// ../L-Lang/src/llang-jsonc.ts
import { TextDecoder as TextDecoder2 } from "util";

// ../L-Lang/node_modules/jsonc-parser/lib/esm/impl/scanner.js
function createScanner(text, ignoreTrivia = false) {
  const len = text.length;
  let pos = 0, value = "", tokenOffset = 0, token = 16, lineNumber = 0, lineStartOffset = 0, tokenLineStartOffset = 0, prevTokenLineStartOffset = 0, scanError = 0;
  function scanHexDigits(count, exact) {
    let digits = 0;
    let value2 = 0;
    while (digits < count || !exact) {
      let ch = text.charCodeAt(pos);
      if (ch >= 48 && ch <= 57) {
        value2 = value2 * 16 + ch - 48;
      } else if (ch >= 65 && ch <= 70) {
        value2 = value2 * 16 + ch - 65 + 10;
      } else if (ch >= 97 && ch <= 102) {
        value2 = value2 * 16 + ch - 97 + 10;
      } else {
        break;
      }
      pos++;
      digits++;
    }
    if (digits < count) {
      value2 = -1;
    }
    return value2;
  }
  function setPosition(newPosition) {
    pos = newPosition;
    value = "";
    tokenOffset = 0;
    token = 16;
    scanError = 0;
  }
  function scanNumber() {
    let start = pos;
    if (text.charCodeAt(pos) === 48) {
      pos++;
    } else {
      pos++;
      while (pos < text.length && isDigit(text.charCodeAt(pos))) {
        pos++;
      }
    }
    if (pos < text.length && text.charCodeAt(pos) === 46) {
      pos++;
      if (pos < text.length && isDigit(text.charCodeAt(pos))) {
        pos++;
        while (pos < text.length && isDigit(text.charCodeAt(pos))) {
          pos++;
        }
      } else {
        scanError = 3;
        return text.substring(start, pos);
      }
    }
    let end = pos;
    if (pos < text.length && (text.charCodeAt(pos) === 69 || text.charCodeAt(pos) === 101)) {
      pos++;
      if (pos < text.length && text.charCodeAt(pos) === 43 || text.charCodeAt(pos) === 45) {
        pos++;
      }
      if (pos < text.length && isDigit(text.charCodeAt(pos))) {
        pos++;
        while (pos < text.length && isDigit(text.charCodeAt(pos))) {
          pos++;
        }
        end = pos;
      } else {
        scanError = 3;
      }
    }
    return text.substring(start, end);
  }
  function scanString() {
    let result = "", start = pos;
    while (true) {
      if (pos >= len) {
        result += text.substring(start, pos);
        scanError = 2;
        break;
      }
      const ch = text.charCodeAt(pos);
      if (ch === 34) {
        result += text.substring(start, pos);
        pos++;
        break;
      }
      if (ch === 92) {
        result += text.substring(start, pos);
        pos++;
        if (pos >= len) {
          scanError = 2;
          break;
        }
        const ch2 = text.charCodeAt(pos++);
        switch (ch2) {
          case 34:
            result += '"';
            break;
          case 92:
            result += "\\";
            break;
          case 47:
            result += "/";
            break;
          case 98:
            result += "\b";
            break;
          case 102:
            result += "\f";
            break;
          case 110:
            result += `
`;
            break;
          case 114:
            result += "\r";
            break;
          case 116:
            result += "\t";
            break;
          case 117:
            const ch3 = scanHexDigits(4, true);
            if (ch3 >= 0) {
              result += String.fromCharCode(ch3);
            } else {
              scanError = 4;
            }
            break;
          default:
            scanError = 5;
        }
        start = pos;
        continue;
      }
      if (ch >= 0 && ch <= 31) {
        if (isLineBreak(ch)) {
          result += text.substring(start, pos);
          scanError = 2;
          break;
        } else {
          scanError = 6;
        }
      }
      pos++;
    }
    return result;
  }
  function scanNext() {
    value = "";
    scanError = 0;
    tokenOffset = pos;
    lineStartOffset = lineNumber;
    prevTokenLineStartOffset = tokenLineStartOffset;
    if (pos >= len) {
      tokenOffset = len;
      return token = 17;
    }
    let code = text.charCodeAt(pos);
    if (isWhiteSpace(code)) {
      do {
        pos++;
        value += String.fromCharCode(code);
        code = text.charCodeAt(pos);
      } while (isWhiteSpace(code));
      return token = 15;
    }
    if (isLineBreak(code)) {
      pos++;
      value += String.fromCharCode(code);
      if (code === 13 && text.charCodeAt(pos) === 10) {
        pos++;
        value += `
`;
      }
      lineNumber++;
      tokenLineStartOffset = pos;
      return token = 14;
    }
    switch (code) {
      case 123:
        pos++;
        return token = 1;
      case 125:
        pos++;
        return token = 2;
      case 91:
        pos++;
        return token = 3;
      case 93:
        pos++;
        return token = 4;
      case 58:
        pos++;
        return token = 6;
      case 44:
        pos++;
        return token = 5;
      case 34:
        pos++;
        value = scanString();
        return token = 10;
      case 47:
        const start = pos - 1;
        if (text.charCodeAt(pos + 1) === 47) {
          pos += 2;
          while (pos < len) {
            if (isLineBreak(text.charCodeAt(pos))) {
              break;
            }
            pos++;
          }
          value = text.substring(start, pos);
          return token = 12;
        }
        if (text.charCodeAt(pos + 1) === 42) {
          pos += 2;
          const safeLength = len - 1;
          let commentClosed = false;
          while (pos < safeLength) {
            const ch = text.charCodeAt(pos);
            if (ch === 42 && text.charCodeAt(pos + 1) === 47) {
              pos += 2;
              commentClosed = true;
              break;
            }
            pos++;
            if (isLineBreak(ch)) {
              if (ch === 13 && text.charCodeAt(pos) === 10) {
                pos++;
              }
              lineNumber++;
              tokenLineStartOffset = pos;
            }
          }
          if (!commentClosed) {
            pos++;
            scanError = 1;
          }
          value = text.substring(start, pos);
          return token = 13;
        }
        value += String.fromCharCode(code);
        pos++;
        return token = 16;
      case 45:
        value += String.fromCharCode(code);
        pos++;
        if (pos === len || !isDigit(text.charCodeAt(pos))) {
          return token = 16;
        }
      case 48:
      case 49:
      case 50:
      case 51:
      case 52:
      case 53:
      case 54:
      case 55:
      case 56:
      case 57:
        value += scanNumber();
        return token = 11;
      default:
        while (pos < len && isUnknownContentCharacter(code)) {
          pos++;
          code = text.charCodeAt(pos);
        }
        if (tokenOffset !== pos) {
          value = text.substring(tokenOffset, pos);
          switch (value) {
            case "true":
              return token = 8;
            case "false":
              return token = 9;
            case "null":
              return token = 7;
          }
          return token = 16;
        }
        value += String.fromCharCode(code);
        pos++;
        return token = 16;
    }
  }
  function isUnknownContentCharacter(code) {
    if (isWhiteSpace(code) || isLineBreak(code)) {
      return false;
    }
    switch (code) {
      case 125:
      case 93:
      case 123:
      case 91:
      case 34:
      case 58:
      case 44:
      case 47:
        return false;
    }
    return true;
  }
  function scanNextNonTrivia() {
    let result;
    do {
      result = scanNext();
    } while (result >= 12 && result <= 15);
    return result;
  }
  return {
    setPosition,
    getPosition: () => pos,
    scan: ignoreTrivia ? scanNextNonTrivia : scanNext,
    getToken: () => token,
    getTokenValue: () => value,
    getTokenOffset: () => tokenOffset,
    getTokenLength: () => pos - tokenOffset,
    getTokenStartLine: () => lineStartOffset,
    getTokenStartCharacter: () => tokenOffset - prevTokenLineStartOffset,
    getTokenError: () => scanError
  };
}
function isWhiteSpace(ch) {
  return ch === 32 || ch === 9;
}
function isLineBreak(ch) {
  return ch === 10 || ch === 13;
}
function isDigit(ch) {
  return ch >= 48 && ch <= 57;
}
var CharacterCodes;
(function(CharacterCodes2) {
  CharacterCodes2[CharacterCodes2["lineFeed"] = 10] = "lineFeed";
  CharacterCodes2[CharacterCodes2["carriageReturn"] = 13] = "carriageReturn";
  CharacterCodes2[CharacterCodes2["space"] = 32] = "space";
  CharacterCodes2[CharacterCodes2["_0"] = 48] = "_0";
  CharacterCodes2[CharacterCodes2["_1"] = 49] = "_1";
  CharacterCodes2[CharacterCodes2["_2"] = 50] = "_2";
  CharacterCodes2[CharacterCodes2["_3"] = 51] = "_3";
  CharacterCodes2[CharacterCodes2["_4"] = 52] = "_4";
  CharacterCodes2[CharacterCodes2["_5"] = 53] = "_5";
  CharacterCodes2[CharacterCodes2["_6"] = 54] = "_6";
  CharacterCodes2[CharacterCodes2["_7"] = 55] = "_7";
  CharacterCodes2[CharacterCodes2["_8"] = 56] = "_8";
  CharacterCodes2[CharacterCodes2["_9"] = 57] = "_9";
  CharacterCodes2[CharacterCodes2["a"] = 97] = "a";
  CharacterCodes2[CharacterCodes2["b"] = 98] = "b";
  CharacterCodes2[CharacterCodes2["c"] = 99] = "c";
  CharacterCodes2[CharacterCodes2["d"] = 100] = "d";
  CharacterCodes2[CharacterCodes2["e"] = 101] = "e";
  CharacterCodes2[CharacterCodes2["f"] = 102] = "f";
  CharacterCodes2[CharacterCodes2["g"] = 103] = "g";
  CharacterCodes2[CharacterCodes2["h"] = 104] = "h";
  CharacterCodes2[CharacterCodes2["i"] = 105] = "i";
  CharacterCodes2[CharacterCodes2["j"] = 106] = "j";
  CharacterCodes2[CharacterCodes2["k"] = 107] = "k";
  CharacterCodes2[CharacterCodes2["l"] = 108] = "l";
  CharacterCodes2[CharacterCodes2["m"] = 109] = "m";
  CharacterCodes2[CharacterCodes2["n"] = 110] = "n";
  CharacterCodes2[CharacterCodes2["o"] = 111] = "o";
  CharacterCodes2[CharacterCodes2["p"] = 112] = "p";
  CharacterCodes2[CharacterCodes2["q"] = 113] = "q";
  CharacterCodes2[CharacterCodes2["r"] = 114] = "r";
  CharacterCodes2[CharacterCodes2["s"] = 115] = "s";
  CharacterCodes2[CharacterCodes2["t"] = 116] = "t";
  CharacterCodes2[CharacterCodes2["u"] = 117] = "u";
  CharacterCodes2[CharacterCodes2["v"] = 118] = "v";
  CharacterCodes2[CharacterCodes2["w"] = 119] = "w";
  CharacterCodes2[CharacterCodes2["x"] = 120] = "x";
  CharacterCodes2[CharacterCodes2["y"] = 121] = "y";
  CharacterCodes2[CharacterCodes2["z"] = 122] = "z";
  CharacterCodes2[CharacterCodes2["A"] = 65] = "A";
  CharacterCodes2[CharacterCodes2["B"] = 66] = "B";
  CharacterCodes2[CharacterCodes2["C"] = 67] = "C";
  CharacterCodes2[CharacterCodes2["D"] = 68] = "D";
  CharacterCodes2[CharacterCodes2["E"] = 69] = "E";
  CharacterCodes2[CharacterCodes2["F"] = 70] = "F";
  CharacterCodes2[CharacterCodes2["G"] = 71] = "G";
  CharacterCodes2[CharacterCodes2["H"] = 72] = "H";
  CharacterCodes2[CharacterCodes2["I"] = 73] = "I";
  CharacterCodes2[CharacterCodes2["J"] = 74] = "J";
  CharacterCodes2[CharacterCodes2["K"] = 75] = "K";
  CharacterCodes2[CharacterCodes2["L"] = 76] = "L";
  CharacterCodes2[CharacterCodes2["M"] = 77] = "M";
  CharacterCodes2[CharacterCodes2["N"] = 78] = "N";
  CharacterCodes2[CharacterCodes2["O"] = 79] = "O";
  CharacterCodes2[CharacterCodes2["P"] = 80] = "P";
  CharacterCodes2[CharacterCodes2["Q"] = 81] = "Q";
  CharacterCodes2[CharacterCodes2["R"] = 82] = "R";
  CharacterCodes2[CharacterCodes2["S"] = 83] = "S";
  CharacterCodes2[CharacterCodes2["T"] = 84] = "T";
  CharacterCodes2[CharacterCodes2["U"] = 85] = "U";
  CharacterCodes2[CharacterCodes2["V"] = 86] = "V";
  CharacterCodes2[CharacterCodes2["W"] = 87] = "W";
  CharacterCodes2[CharacterCodes2["X"] = 88] = "X";
  CharacterCodes2[CharacterCodes2["Y"] = 89] = "Y";
  CharacterCodes2[CharacterCodes2["Z"] = 90] = "Z";
  CharacterCodes2[CharacterCodes2["asterisk"] = 42] = "asterisk";
  CharacterCodes2[CharacterCodes2["backslash"] = 92] = "backslash";
  CharacterCodes2[CharacterCodes2["closeBrace"] = 125] = "closeBrace";
  CharacterCodes2[CharacterCodes2["closeBracket"] = 93] = "closeBracket";
  CharacterCodes2[CharacterCodes2["colon"] = 58] = "colon";
  CharacterCodes2[CharacterCodes2["comma"] = 44] = "comma";
  CharacterCodes2[CharacterCodes2["dot"] = 46] = "dot";
  CharacterCodes2[CharacterCodes2["doubleQuote"] = 34] = "doubleQuote";
  CharacterCodes2[CharacterCodes2["minus"] = 45] = "minus";
  CharacterCodes2[CharacterCodes2["openBrace"] = 123] = "openBrace";
  CharacterCodes2[CharacterCodes2["openBracket"] = 91] = "openBracket";
  CharacterCodes2[CharacterCodes2["plus"] = 43] = "plus";
  CharacterCodes2[CharacterCodes2["slash"] = 47] = "slash";
  CharacterCodes2[CharacterCodes2["formFeed"] = 12] = "formFeed";
  CharacterCodes2[CharacterCodes2["tab"] = 9] = "tab";
})(CharacterCodes || (CharacterCodes = {}));

// ../L-Lang/node_modules/jsonc-parser/lib/esm/impl/string-intern.js
var cachedSpaces = new Array(20).fill(0).map((_, index) => {
  return " ".repeat(index);
});
var maxCachedValues = 200;
var cachedBreakLinesWithSpaces = {
  " ": {
    "\n": new Array(maxCachedValues).fill(0).map((_, index) => {
      return `
` + " ".repeat(index);
    }),
    "\r": new Array(maxCachedValues).fill(0).map((_, index) => {
      return "\r" + " ".repeat(index);
    }),
    "\r\n": new Array(maxCachedValues).fill(0).map((_, index) => {
      return `\r
` + " ".repeat(index);
    })
  },
  "\t": {
    "\n": new Array(maxCachedValues).fill(0).map((_, index) => {
      return `
` + "\t".repeat(index);
    }),
    "\r": new Array(maxCachedValues).fill(0).map((_, index) => {
      return "\r" + "\t".repeat(index);
    }),
    "\r\n": new Array(maxCachedValues).fill(0).map((_, index) => {
      return `\r
` + "\t".repeat(index);
    })
  }
};

// ../L-Lang/node_modules/jsonc-parser/lib/esm/impl/parser.js
var ParseOptions;
(function(ParseOptions2) {
  ParseOptions2.DEFAULT = {
    allowTrailingComma: false
  };
})(ParseOptions || (ParseOptions = {}));
function parseTree(text, errors = [], options = ParseOptions.DEFAULT) {
  let currentParent = { type: "array", offset: -1, length: -1, children: [], parent: undefined };
  function ensurePropertyComplete(endOffset) {
    if (currentParent.type === "property") {
      currentParent.length = endOffset - currentParent.offset;
      currentParent = currentParent.parent;
    }
  }
  function onValue(valueNode) {
    currentParent.children.push(valueNode);
    return valueNode;
  }
  const visitor = {
    onObjectBegin: (offset) => {
      currentParent = onValue({ type: "object", offset, length: -1, parent: currentParent, children: [] });
    },
    onObjectProperty: (name, offset, length) => {
      currentParent = onValue({ type: "property", offset, length: -1, parent: currentParent, children: [] });
      currentParent.children.push({ type: "string", value: name, offset, length, parent: currentParent });
    },
    onObjectEnd: (offset, length) => {
      ensurePropertyComplete(offset + length);
      currentParent.length = offset + length - currentParent.offset;
      currentParent = currentParent.parent;
      ensurePropertyComplete(offset + length);
    },
    onArrayBegin: (offset, length) => {
      currentParent = onValue({ type: "array", offset, length: -1, parent: currentParent, children: [] });
    },
    onArrayEnd: (offset, length) => {
      currentParent.length = offset + length - currentParent.offset;
      currentParent = currentParent.parent;
      ensurePropertyComplete(offset + length);
    },
    onLiteralValue: (value, offset, length) => {
      onValue({ type: getNodeType(value), offset, length, parent: currentParent, value });
      ensurePropertyComplete(offset + length);
    },
    onSeparator: (sep, offset, length) => {
      if (currentParent.type === "property") {
        if (sep === ":") {
          currentParent.colonOffset = offset;
        } else if (sep === ",") {
          ensurePropertyComplete(offset);
        }
      }
    },
    onError: (error, offset, length) => {
      errors.push({ error, offset, length });
    }
  };
  visit(text, visitor, options);
  const result = currentParent.children[0];
  if (result) {
    delete result.parent;
  }
  return result;
}
function findNodeAtLocation(root, path) {
  if (!root) {
    return;
  }
  let node = root;
  for (let segment of path) {
    if (typeof segment === "string") {
      if (node.type !== "object" || !Array.isArray(node.children)) {
        return;
      }
      let found = false;
      for (const propertyNode of node.children) {
        if (Array.isArray(propertyNode.children) && propertyNode.children[0].value === segment && propertyNode.children.length === 2) {
          node = propertyNode.children[1];
          found = true;
          break;
        }
      }
      if (!found) {
        return;
      }
    } else {
      const index = segment;
      if (node.type !== "array" || index < 0 || !Array.isArray(node.children) || index >= node.children.length) {
        return;
      }
      node = node.children[index];
    }
  }
  return node;
}
function getNodeValue(node) {
  switch (node.type) {
    case "array":
      return node.children.map(getNodeValue);
    case "object":
      const obj = Object.create(null);
      for (let prop of node.children) {
        const valueNode = prop.children[1];
        if (valueNode) {
          obj[prop.children[0].value] = getNodeValue(valueNode);
        }
      }
      return obj;
    case "null":
    case "string":
    case "number":
    case "boolean":
      return node.value;
    default:
      return;
  }
}
function visit(text, visitor, options = ParseOptions.DEFAULT) {
  const _scanner = createScanner(text, false);
  const _jsonPath = [];
  let suppressedCallbacks = 0;
  function toNoArgVisit(visitFunction) {
    return visitFunction ? () => suppressedCallbacks === 0 && visitFunction(_scanner.getTokenOffset(), _scanner.getTokenLength(), _scanner.getTokenStartLine(), _scanner.getTokenStartCharacter()) : () => true;
  }
  function toOneArgVisit(visitFunction) {
    return visitFunction ? (arg) => suppressedCallbacks === 0 && visitFunction(arg, _scanner.getTokenOffset(), _scanner.getTokenLength(), _scanner.getTokenStartLine(), _scanner.getTokenStartCharacter()) : () => true;
  }
  function toOneArgVisitWithPath(visitFunction) {
    return visitFunction ? (arg) => suppressedCallbacks === 0 && visitFunction(arg, _scanner.getTokenOffset(), _scanner.getTokenLength(), _scanner.getTokenStartLine(), _scanner.getTokenStartCharacter(), () => _jsonPath.slice()) : () => true;
  }
  function toBeginVisit(visitFunction) {
    return visitFunction ? () => {
      if (suppressedCallbacks > 0) {
        suppressedCallbacks++;
      } else {
        let cbReturn = visitFunction(_scanner.getTokenOffset(), _scanner.getTokenLength(), _scanner.getTokenStartLine(), _scanner.getTokenStartCharacter(), () => _jsonPath.slice());
        if (cbReturn === false) {
          suppressedCallbacks = 1;
        }
      }
    } : () => true;
  }
  function toEndVisit(visitFunction) {
    return visitFunction ? () => {
      if (suppressedCallbacks > 0) {
        suppressedCallbacks--;
      }
      if (suppressedCallbacks === 0) {
        visitFunction(_scanner.getTokenOffset(), _scanner.getTokenLength(), _scanner.getTokenStartLine(), _scanner.getTokenStartCharacter());
      }
    } : () => true;
  }
  const onObjectBegin = toBeginVisit(visitor.onObjectBegin), onObjectProperty = toOneArgVisitWithPath(visitor.onObjectProperty), onObjectEnd = toEndVisit(visitor.onObjectEnd), onArrayBegin = toBeginVisit(visitor.onArrayBegin), onArrayEnd = toEndVisit(visitor.onArrayEnd), onLiteralValue = toOneArgVisitWithPath(visitor.onLiteralValue), onSeparator = toOneArgVisit(visitor.onSeparator), onComment = toNoArgVisit(visitor.onComment), onError = toOneArgVisit(visitor.onError);
  const disallowComments = options && options.disallowComments;
  const allowTrailingComma = options && options.allowTrailingComma;
  function scanNext() {
    while (true) {
      const token = _scanner.scan();
      switch (_scanner.getTokenError()) {
        case 4:
          handleError(14);
          break;
        case 5:
          handleError(15);
          break;
        case 3:
          handleError(13);
          break;
        case 1:
          if (!disallowComments) {
            handleError(11);
          }
          break;
        case 2:
          handleError(12);
          break;
        case 6:
          handleError(16);
          break;
      }
      switch (token) {
        case 12:
        case 13:
          if (disallowComments) {
            handleError(10);
          } else {
            onComment();
          }
          break;
        case 16:
          handleError(1);
          break;
        case 15:
        case 14:
          break;
        default:
          return token;
      }
    }
  }
  function handleError(error, skipUntilAfter = [], skipUntil = []) {
    onError(error);
    if (skipUntilAfter.length + skipUntil.length > 0) {
      let token = _scanner.getToken();
      while (token !== 17) {
        if (skipUntilAfter.indexOf(token) !== -1) {
          scanNext();
          break;
        } else if (skipUntil.indexOf(token) !== -1) {
          break;
        }
        token = scanNext();
      }
    }
  }
  function parseString(isValue) {
    const value = _scanner.getTokenValue();
    if (isValue) {
      onLiteralValue(value);
    } else {
      onObjectProperty(value);
      _jsonPath.push(value);
    }
    scanNext();
    return true;
  }
  function parseLiteral() {
    switch (_scanner.getToken()) {
      case 11:
        const tokenValue = _scanner.getTokenValue();
        let value = Number(tokenValue);
        if (isNaN(value)) {
          handleError(2);
          value = 0;
        }
        onLiteralValue(value);
        break;
      case 7:
        onLiteralValue(null);
        break;
      case 8:
        onLiteralValue(true);
        break;
      case 9:
        onLiteralValue(false);
        break;
      default:
        return false;
    }
    scanNext();
    return true;
  }
  function parseProperty() {
    if (_scanner.getToken() !== 10) {
      handleError(3, [], [2, 5]);
      return false;
    }
    parseString(false);
    if (_scanner.getToken() === 6) {
      onSeparator(":");
      scanNext();
      if (!parseValue()) {
        handleError(4, [], [2, 5]);
      }
    } else {
      handleError(5, [], [2, 5]);
    }
    _jsonPath.pop();
    return true;
  }
  function parseObject() {
    onObjectBegin();
    scanNext();
    let needsComma = false;
    while (_scanner.getToken() !== 2 && _scanner.getToken() !== 17) {
      if (_scanner.getToken() === 5) {
        if (!needsComma) {
          handleError(4, [], []);
        }
        onSeparator(",");
        scanNext();
        if (_scanner.getToken() === 2 && allowTrailingComma) {
          break;
        }
      } else if (needsComma) {
        handleError(6, [], []);
      }
      if (!parseProperty()) {
        handleError(4, [], [2, 5]);
      }
      needsComma = true;
    }
    onObjectEnd();
    if (_scanner.getToken() !== 2) {
      handleError(7, [2], []);
    } else {
      scanNext();
    }
    return true;
  }
  function parseArray() {
    onArrayBegin();
    scanNext();
    let isFirstElement = true;
    let needsComma = false;
    while (_scanner.getToken() !== 4 && _scanner.getToken() !== 17) {
      if (_scanner.getToken() === 5) {
        if (!needsComma) {
          handleError(4, [], []);
        }
        onSeparator(",");
        scanNext();
        if (_scanner.getToken() === 4 && allowTrailingComma) {
          break;
        }
      } else if (needsComma) {
        handleError(6, [], []);
      }
      if (isFirstElement) {
        _jsonPath.push(0);
        isFirstElement = false;
      } else {
        _jsonPath[_jsonPath.length - 1]++;
      }
      if (!parseValue()) {
        handleError(4, [], [4, 5]);
      }
      needsComma = true;
    }
    onArrayEnd();
    if (!isFirstElement) {
      _jsonPath.pop();
    }
    if (_scanner.getToken() !== 4) {
      handleError(8, [4], []);
    } else {
      scanNext();
    }
    return true;
  }
  function parseValue() {
    switch (_scanner.getToken()) {
      case 3:
        return parseArray();
      case 1:
        return parseObject();
      case 10:
        return parseString(true);
      default:
        return parseLiteral();
    }
  }
  scanNext();
  if (_scanner.getToken() === 17) {
    if (options.allowEmptyContent) {
      return true;
    }
    handleError(4, [], []);
    return false;
  }
  if (!parseValue()) {
    handleError(4, [], []);
    return false;
  }
  if (_scanner.getToken() !== 17) {
    handleError(9, [], []);
  }
  return true;
}
function getNodeType(value) {
  switch (typeof value) {
    case "boolean":
      return "boolean";
    case "number":
      return "number";
    case "string":
      return "string";
    case "object": {
      if (!value) {
        return "null";
      } else if (Array.isArray(value)) {
        return "array";
      }
      return "object";
    }
    default:
      return "null";
  }
}

// ../L-Lang/node_modules/jsonc-parser/lib/esm/main.js
var createScanner2 = createScanner;
var ScanError;
(function(ScanError2) {
  ScanError2[ScanError2["None"] = 0] = "None";
  ScanError2[ScanError2["UnexpectedEndOfComment"] = 1] = "UnexpectedEndOfComment";
  ScanError2[ScanError2["UnexpectedEndOfString"] = 2] = "UnexpectedEndOfString";
  ScanError2[ScanError2["UnexpectedEndOfNumber"] = 3] = "UnexpectedEndOfNumber";
  ScanError2[ScanError2["InvalidUnicode"] = 4] = "InvalidUnicode";
  ScanError2[ScanError2["InvalidEscapeCharacter"] = 5] = "InvalidEscapeCharacter";
  ScanError2[ScanError2["InvalidCharacter"] = 6] = "InvalidCharacter";
})(ScanError || (ScanError = {}));
var SyntaxKind;
(function(SyntaxKind2) {
  SyntaxKind2[SyntaxKind2["OpenBraceToken"] = 1] = "OpenBraceToken";
  SyntaxKind2[SyntaxKind2["CloseBraceToken"] = 2] = "CloseBraceToken";
  SyntaxKind2[SyntaxKind2["OpenBracketToken"] = 3] = "OpenBracketToken";
  SyntaxKind2[SyntaxKind2["CloseBracketToken"] = 4] = "CloseBracketToken";
  SyntaxKind2[SyntaxKind2["CommaToken"] = 5] = "CommaToken";
  SyntaxKind2[SyntaxKind2["ColonToken"] = 6] = "ColonToken";
  SyntaxKind2[SyntaxKind2["NullKeyword"] = 7] = "NullKeyword";
  SyntaxKind2[SyntaxKind2["TrueKeyword"] = 8] = "TrueKeyword";
  SyntaxKind2[SyntaxKind2["FalseKeyword"] = 9] = "FalseKeyword";
  SyntaxKind2[SyntaxKind2["StringLiteral"] = 10] = "StringLiteral";
  SyntaxKind2[SyntaxKind2["NumericLiteral"] = 11] = "NumericLiteral";
  SyntaxKind2[SyntaxKind2["LineCommentTrivia"] = 12] = "LineCommentTrivia";
  SyntaxKind2[SyntaxKind2["BlockCommentTrivia"] = 13] = "BlockCommentTrivia";
  SyntaxKind2[SyntaxKind2["LineBreakTrivia"] = 14] = "LineBreakTrivia";
  SyntaxKind2[SyntaxKind2["Trivia"] = 15] = "Trivia";
  SyntaxKind2[SyntaxKind2["Unknown"] = 16] = "Unknown";
  SyntaxKind2[SyntaxKind2["EOF"] = 17] = "EOF";
})(SyntaxKind || (SyntaxKind = {}));
var parseTree2 = parseTree;
var findNodeAtLocation2 = findNodeAtLocation;
var getNodeValue2 = getNodeValue;
var ParseErrorCode;
(function(ParseErrorCode2) {
  ParseErrorCode2[ParseErrorCode2["InvalidSymbol"] = 1] = "InvalidSymbol";
  ParseErrorCode2[ParseErrorCode2["InvalidNumberFormat"] = 2] = "InvalidNumberFormat";
  ParseErrorCode2[ParseErrorCode2["PropertyNameExpected"] = 3] = "PropertyNameExpected";
  ParseErrorCode2[ParseErrorCode2["ValueExpected"] = 4] = "ValueExpected";
  ParseErrorCode2[ParseErrorCode2["ColonExpected"] = 5] = "ColonExpected";
  ParseErrorCode2[ParseErrorCode2["CommaExpected"] = 6] = "CommaExpected";
  ParseErrorCode2[ParseErrorCode2["CloseBraceExpected"] = 7] = "CloseBraceExpected";
  ParseErrorCode2[ParseErrorCode2["CloseBracketExpected"] = 8] = "CloseBracketExpected";
  ParseErrorCode2[ParseErrorCode2["EndOfFileExpected"] = 9] = "EndOfFileExpected";
  ParseErrorCode2[ParseErrorCode2["InvalidCommentToken"] = 10] = "InvalidCommentToken";
  ParseErrorCode2[ParseErrorCode2["UnexpectedEndOfComment"] = 11] = "UnexpectedEndOfComment";
  ParseErrorCode2[ParseErrorCode2["UnexpectedEndOfString"] = 12] = "UnexpectedEndOfString";
  ParseErrorCode2[ParseErrorCode2["UnexpectedEndOfNumber"] = 13] = "UnexpectedEndOfNumber";
  ParseErrorCode2[ParseErrorCode2["InvalidUnicode"] = 14] = "InvalidUnicode";
  ParseErrorCode2[ParseErrorCode2["InvalidEscapeCharacter"] = 15] = "InvalidEscapeCharacter";
  ParseErrorCode2[ParseErrorCode2["InvalidCharacter"] = 16] = "InvalidCharacter";
})(ParseErrorCode || (ParseErrorCode = {}));
function printParseErrorCode(code) {
  switch (code) {
    case 1:
      return "InvalidSymbol";
    case 2:
      return "InvalidNumberFormat";
    case 3:
      return "PropertyNameExpected";
    case 4:
      return "ValueExpected";
    case 5:
      return "ColonExpected";
    case 6:
      return "CommaExpected";
    case 7:
      return "CloseBraceExpected";
    case 8:
      return "CloseBracketExpected";
    case 9:
      return "EndOfFileExpected";
    case 10:
      return "InvalidCommentToken";
    case 11:
      return "UnexpectedEndOfComment";
    case 12:
      return "UnexpectedEndOfString";
    case 13:
      return "UnexpectedEndOfNumber";
    case 14:
      return "InvalidUnicode";
    case 15:
      return "InvalidEscapeCharacter";
    case 16:
      return "InvalidCharacter";
  }
  return "<unknown ParseErrorCode>";
}

// ../L-Lang/src/llang-diagnostics.ts
var LLANG_DIAGNOSTIC_LIMIT = 32;
function positionAt(text, offset) {
  const bounded = Math.max(0, Math.min(offset, text.length));
  let line = 1;
  let lineStart = 0;
  for (let i = 0;i < bounded; i++) {
    if (text.charCodeAt(i) === 10) {
      line++;
      lineStart = i + 1;
    }
  }
  return { line, column: bounded - lineStart + 1, offset: bounded };
}
function sourceRange(text, offset, length) {
  return {
    start: positionAt(text, offset),
    end: positionAt(text, offset + Math.max(1, length))
  };
}
function pointer(segments) {
  if (!segments.length)
    return "";
  return segments.map((segment) => String(segment).replaceAll("~", "~0").replaceAll("/", "~1")).map((segment) => `/${segment}`).join("");
}
function reportFor(diagnostics) {
  const normalized = diagnostics.map((diagnostic) => ({
    ...diagnostic,
    message: diagnostic.message.slice(0, 2000),
    ...diagnostic.hint ? { hint: diagnostic.hint.slice(0, 2000) } : {}
  }));
  const sorted = normalized.sort((a, b) => a.range.start.offset - b.range.start.offset || a.code.localeCompare(b.code));
  const truncated = sorted.length > LLANG_DIAGNOSTIC_LIMIT;
  const selected = sorted.slice(0, LLANG_DIAGNOSTIC_LIMIT);
  return {
    version: 1,
    ok: !sorted.some((diagnostic) => diagnostic.severity === "error"),
    diagnostics: selected,
    truncated
  };
}

// ../L-Lang/src/llang-jsonc.ts
init_wasm_contract();
init_stable_hash();
var LLANG_SOURCE_BYTES = 1024 * 1024;
var LLANG_JSON_DEPTH = 64;
function diagnostic(text, file, code, message, offset = 0, length = 1, path = "") {
  return {
    code,
    severity: "error",
    message,
    file,
    range: sourceRange(text, offset, length),
    path,
    related: []
  };
}
function hasUnpairedSurrogate(value) {
  for (let index = 0;index < value.length; index++) {
    const unit = value.charCodeAt(index);
    if (unit >= 55296 && unit <= 56319) {
      const next = value.charCodeAt(index + 1);
      if (!(next >= 56320 && next <= 57343))
        return true;
      index++;
    } else if (unit >= 56320 && unit <= 57343)
      return true;
  }
  return false;
}
function checkTree(text, file, node, diagnostics, path = [], depth = 1) {
  if (diagnostics.length > LLANG_DIAGNOSTIC_LIMIT)
    return;
  if (depth > LLANG_JSON_DEPTH) {
    diagnostics.push(diagnostic(text, file, "LLJ003", `JSONC depth exceeds ${LLANG_JSON_DEPTH}`, node.offset, node.length, pointer(path)));
    return;
  }
  if (node.type === "object") {
    const seen = new Map;
    for (const property of node.children ?? []) {
      if (diagnostics.length > LLANG_DIAGNOSTIC_LIMIT)
        break;
      const [keyNode, valueNode] = property.children ?? [];
      if (!keyNode || !valueNode)
        continue;
      const key = String(getNodeValue2(keyNode));
      const propertyPath = [...path, key];
      if (hasUnpairedSurrogate(key))
        diagnostics.push(diagnostic(text, file, "LLJ003", "key contains an unpaired Unicode surrogate", keyNode.offset, keyNode.length, pointer(propertyPath)));
      const prior = seen.get(key);
      if (prior) {
        const item = diagnostic(text, file, "LLJ002", `duplicate key ${JSON.stringify(key)}`, keyNode.offset, keyNode.length, pointer(propertyPath));
        item.related.push({
          message: "first key is here",
          range: sourceRange(text, prior.node.offset, prior.node.length),
          path: prior.path
        });
        diagnostics.push(item);
      } else
        seen.set(key, { node: keyNode, path: pointer(propertyPath) });
      checkTree(text, file, valueNode, diagnostics, propertyPath, depth + 1);
    }
  } else if (node.type === "array") {
    for (const [index, child] of (node.children ?? []).entries()) {
      if (diagnostics.length > LLANG_DIAGNOSTIC_LIMIT)
        break;
      checkTree(text, file, child, diagnostics, [...path, index], depth + 1);
    }
  } else if (node.type === "number" && !Number.isFinite(getNodeValue2(node))) {
    diagnostics.push(diagnostic(text, file, "LLJ001", "number must be finite", node.offset, node.length, pointer(path)));
  } else if (node.type === "string") {
    const value = String(getNodeValue2(node));
    if (hasUnpairedSurrogate(value))
      diagnostics.push(diagnostic(text, file, "LLJ003", "string contains an unpaired Unicode surrogate", node.offset, node.length, pointer(path)));
  }
}
function checkDepth(text, file, diagnostics) {
  const scanner = createScanner2(text, false);
  let depth = 0;
  for (;; ) {
    const token = scanner.scan();
    if (token === SyntaxKind.EOF)
      return;
    if (token === SyntaxKind.OpenBraceToken || token === SyntaxKind.OpenBracketToken) {
      depth++;
      if (depth > LLANG_JSON_DEPTH) {
        diagnostics.push(diagnostic(text, file, "LLJ003", `JSONC depth exceeds ${LLANG_JSON_DEPTH}`, scanner.getTokenOffset(), scanner.getTokenLength()));
        return;
      }
    } else if (token === SyntaxKind.CloseBraceToken || token === SyntaxKind.CloseBracketToken)
      depth = Math.max(0, depth - 1);
  }
}
function parseLlangJsonc(text, file = "<input>") {
  const diagnostics = [];
  if (Buffer.byteLength(text) > LLANG_SOURCE_BYTES)
    diagnostics.push(diagnostic(text, file, "LLJ003", `source exceeds ${LLANG_SOURCE_BYTES} bytes`));
  if (text.charCodeAt(0) === 65279)
    diagnostics.push(diagnostic(text, file, "LLJ003", "UTF-8 BOM is not allowed"));
  checkDepth(text, file, diagnostics);
  if (diagnostics.some((item) => item.code === "LLJ003"))
    return { report: reportFor(diagnostics) };
  const errors = [];
  const root = parseTree2(text, errors, {
    allowTrailingComma: true,
    disallowComments: false,
    allowEmptyContent: false
  });
  for (const error of errors.slice(0, LLANG_DIAGNOSTIC_LIMIT + 1))
    diagnostics.push(diagnostic(text, file, "LLJ001", `invalid JSONC: ${printParseErrorCode(error.error)}`, error.offset, error.length));
  if (root && root.type !== "object")
    diagnostics.push(diagnostic(text, file, "LLJ001", "JSONC root must be an object", root.offset, root.length));
  if (root)
    checkTree(text, file, root, diagnostics);
  const report = reportFor(diagnostics);
  if (!root || !report.ok)
    return { report };
  return {
    document: {
      text,
      file,
      root,
      value: getNodeValue2(root),
      sourceHash: digest(new TextEncoder().encode(text))
    },
    report
  };
}
function nodeForPath(document, path) {
  return findNodeAtLocation2(document.root, [...path]) ?? document.root;
}
function rangeForPath(document, path) {
  const node = nodeForPath(document, path);
  return sourceRange(document.text, node.offset, node.length);
}
function decodeUtf8(bytes, file) {
  try {
    return new TextDecoder2("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes);
  } catch {
    throw new Error(`${file} must contain valid UTF-8`);
  }
}
function parseStrictJsonObject(text, file = "<input>") {
  if (Buffer.byteLength(text) > LLANG_SOURCE_BYTES)
    throw new Error(`${file} exceeds ${LLANG_SOURCE_BYTES} bytes`);
  try {
    JSON.parse(text);
  } catch (error) {
    throw new Error(`${file} must contain standard JSON: ${error instanceof Error ? error.message : String(error)}`);
  }
  const parsed = parseLlangJsonc(text, file);
  if (!parsed.document)
    throw new Error(`${file} contains ambiguous or invalid JSON: ${parsed.report.diagnostics.map((item) => `${item.code} ${item.path || "/"}: ${item.message}`).join("; ")}`);
  return parsed.document.value;
}

// ../L-Lang/src/llang-artifact.ts
init_wasm_runtime();
function hash(value, label) {
  if (typeof value !== "string" || !/^[a-f0-9]{64}$/.test(value))
    throw new WasmError("INVALID_ARTIFACT", `invalid ${label}`);
  return value;
}
function parseLlangBuildManifest(input) {
  const value = record(input, [
    "version",
    "language",
    "profile",
    "export",
    "contract",
    "compiler",
    "backend",
    "options",
    "irHash",
    "programHash",
    "sourceHash",
    "wasmHash",
    "file"
  ]);
  if (value.version !== 2 || value.language !== "l-lang" || value.profile !== "predicate-i32-v1" || value.export !== "evaluate" || value.options !== "mvp-no-optimization" || typeof value.compiler !== "string" || !value.compiler || typeof value.backend !== "string" || !value.backend)
    throw new WasmError("INVALID_ARTIFACT", "unsupported L-Lang manifest");
  const wasmHash = hash(value.wasmHash, "wasmHash");
  if (value.file !== `${wasmHash}.wasm`)
    throw new WasmError("INVALID_ARTIFACT", "invalid Wasm file name");
  return {
    version: 2,
    language: "l-lang",
    profile: "predicate-i32-v1",
    export: "evaluate",
    contract: parseContract(value.contract),
    compiler: value.compiler,
    backend: value.backend,
    options: "mvp-no-optimization",
    irHash: hash(value.irHash, "irHash"),
    programHash: hash(value.programHash, "programHash"),
    sourceHash: hash(value.sourceHash, "sourceHash"),
    wasmHash,
    file: String(value.file)
  };
}
function validateLlangArtifact(manifestInput, bytes) {
  const manifest = parseLlangBuildManifest(manifestInput);
  if (digest(bytes) !== manifest.wasmHash || !WebAssembly.validate(bytes))
    throw new WasmError("ARTIFACT_MISMATCH", "invalid L-Lang Wasm artifact");
  assertStatelessWasmBinary(bytes);
  const wasmBytes = new Uint8Array(bytes.byteLength);
  wasmBytes.set(bytes);
  const module = new WebAssembly.Module(wasmBytes.buffer);
  const exports = WebAssembly.Module.exports(module);
  if (WebAssembly.Module.imports(module).length || exports.length !== 1 || exports[0]?.name !== manifest.export || exports[0]?.kind !== "function")
    throw new WasmError("INVALID_ARTIFACT", "unexpected import/export contract");
  const contracts = WebAssembly.Module.customSections(module, "llang.contract");
  if (contracts.length !== 1 || new TextDecoder().decode(contracts[0]) !== digest(JSON.stringify(manifest.contract)))
    throw new WasmError("ARTIFACT_MISMATCH", "ABI contract does not match Wasm");
  return { manifest, bytes };
}

// ../L-Lang/src/llang-program.ts
init_ir();
init_stable_hash();
init_wasm_core();
init_wasm_contract();
var known = new Set([
  "language",
  "version",
  "id",
  "profile",
  "description",
  "contract",
  "body"
]);
function makeDiagnostic(document, code, message, path, hint, severity = "error") {
  return {
    code,
    severity,
    message,
    file: document.file,
    range: rangeForPath(document, path),
    path: pointer(path),
    related: [],
    ...hint ? { hint } : {}
  };
}
function objectValue(input) {
  return input !== null && typeof input === "object" && !Array.isArray(input) ? input : null;
}
function validateSemantics(document, body, contract) {
  const diagnostics = [];
  const fields = new Map(contract.fields.map((field) => [field.name, field]));
  function visit2(expression, path) {
    if ("conditions" in expression) {
      const seen = new Map;
      expression.conditions.forEach((condition, index) => {
        const conditionPath = [...path, "conditions", index];
        const fingerprint = fingerprintFor(condition);
        const prior = seen.get(fingerprint);
        if (prior) {
          const item = makeDiagnostic(document, "LLW001", "condition duplicates an earlier condition", conditionPath, undefined, "warning");
          item.related.push({
            message: "first identical condition is here",
            range: rangeForPath(document, prior.path),
            path: pointer(prior.path)
          });
          diagnostics.push(item);
        } else
          seen.set(fingerprint, { path: conditionPath });
        visit2(condition, conditionPath);
      });
      return;
    }
    if ("condition" in expression) {
      visit2(expression.condition, [...path, "condition"]);
      return;
    }
    if (expression.property.length !== 1) {
      diagnostics.push(makeDiagnostic(document, "LLP001", "predicate-i32-v1 supports a single property segment", [...path, "property"]));
      return;
    }
    const name = expression.property[0];
    const field = fields.get(name);
    if (!field) {
      diagnostics.push(makeDiagnostic(document, "LLT001", `unknown contract field ${JSON.stringify(name)}`, [...path, "property", 0], `available fields: ${[...fields.keys()].map((value) => JSON.stringify(value)).join(", ")}`));
      return;
    }
    if (expression.kind === "present") {
      if (!(field.nullable || field.undefinable || field.optional))
        diagnostics.push(makeDiagnostic(document, "LLT001", "present requires a nullable, undefinable or optional field", path));
      return;
    }
    const literal = expression.value;
    const valid = literal === null && field.nullable || typeof literal === "boolean" && field.kind === "boolean" || typeof literal === "string" && field.kind === "enum" && field.values.includes(literal);
    if (!valid)
      diagnostics.push(makeDiagnostic(document, "LLT001", `value ${JSON.stringify(literal)} is not valid for field ${JSON.stringify(name)}`, [...path, "value"], field.kind === "enum" ? `allowed values: ${field.values.map((value) => JSON.stringify(value)).join(", ")}` : undefined));
  }
  visit2(body, ["body"]);
  return diagnostics;
}
function checkLlangProgram(text, file = "<input>") {
  const syntax = parseLlangJsonc(text, file);
  if (!syntax.document)
    return { report: syntax.report };
  const document = syntax.document;
  const root = objectValue(document.value);
  const diagnostics = [];
  if (!root)
    diagnostics.push(makeDiagnostic(document, "LLS001", "program must be an object", []));
  if (!root)
    return { report: reportFor(diagnostics) };
  for (const key of Object.keys(root)) {
    if (diagnostics.length > LLANG_DIAGNOSTIC_LIMIT)
      break;
    if (!known.has(key))
      diagnostics.push(makeDiagnostic(document, "LLS001", `unknown program field ${JSON.stringify(key)}`, [key]));
  }
  for (const key of [
    "language",
    "version",
    "id",
    "profile",
    "contract",
    "body"
  ])
    if (!Object.hasOwn(root, key))
      diagnostics.push(makeDiagnostic(document, "LLS001", `missing required program field ${JSON.stringify(key)}`, []));
  if (root.language !== "l-lang")
    diagnostics.push(makeDiagnostic(document, "LLS001", 'language must be "l-lang"', [
      "language"
    ]));
  if (root.version !== 1)
    diagnostics.push(makeDiagnostic(document, "LLS001", "version must be 1", ["version"]));
  if (typeof root.id !== "string" || !/^[A-Za-z][A-Za-z0-9_-]{0,63}$/.test(root.id))
    diagnostics.push(makeDiagnostic(document, "LLS001", "id is invalid", ["id"]));
  if (root.profile !== "predicate-i32-v1")
    diagnostics.push(makeDiagnostic(document, "LLS001", 'profile must be "predicate-i32-v1"', [
      "profile"
    ]));
  if (Object.hasOwn(root, "description") && (typeof root.description !== "string" || !root.description.length || unicodeScalarLength(root.description) > 4096))
    diagnostics.push(makeDiagnostic(document, "LLS001", "description must be a non-empty string of at most 4096 characters", ["description"]));
  let contract;
  try {
    contract = parseContract(root.contract);
  } catch (error) {
    diagnostics.push(makeDiagnostic(document, "LLS001", error instanceof Error ? error.message : String(error), ["contract"]));
  }
  let body;
  try {
    body = parsePredicateExpression(root.body, "body");
  } catch (error) {
    diagnostics.push(makeDiagnostic(document, error instanceof PredicateProfileError ? "LLP001" : "LLS001", error instanceof Error ? error.message : String(error), error instanceof PredicateStructureError ? error.diagnosticPath : ["body"]));
  }
  if (contract && body) {
    diagnostics.push(...validateSemantics(document, body, contract));
    if (!diagnostics.some((item) => item.severity === "error")) {
      try {
        lowerPredicate(body, contract);
      } catch (error) {
        diagnostics.push(makeDiagnostic(document, "LLP001", error instanceof Error ? error.message : String(error), ["body"]));
      }
    }
  }
  const report = reportFor([...syntax.report.diagnostics, ...diagnostics]);
  if (!report.ok || !contract || !body)
    return { report };
  const program = {
    language: "l-lang",
    version: 1,
    id: root.id,
    profile: "predicate-i32-v1",
    ...typeof root.description === "string" ? { description: root.description } : {},
    contract,
    body
  };
  return {
    report,
    checked: {
      program,
      sourceHash: document.sourceHash,
      programHash: fingerprintFor({
        language: program.language,
        version: program.version,
        profile: program.profile,
        contract: program.contract,
        body: program.body
      }),
      document
    }
  };
}

// ../L-Lang/src/llang-capability-contracts.ts
init_prompt_source();
init_contained_path();
init_wasm_contract();
var LLANG_CAPABILITY_VERIFIER = "llang-capability-v2";
var roles2 = ["request", "source", "build", "wasm", "tests"];
function assertUnicodeScalars(value, label) {
  const pending = [value];
  while (pending.length) {
    const next = pending.pop();
    if (typeof next === "string") {
      for (let index = 0;index < next.length; index++) {
        const unit = next.charCodeAt(index);
        if (unit >= 55296 && unit <= 56319) {
          const following = next.charCodeAt(index + 1);
          if (!(following >= 56320 && following <= 57343))
            throw new WasmError("INVALID_CAPABILITY", `${label} contains an unpaired Unicode surrogate`);
          index++;
        } else if (unit >= 56320 && unit <= 57343)
          throw new WasmError("INVALID_CAPABILITY", `${label} contains an unpaired Unicode surrogate`);
      }
    } else if (Array.isArray(next))
      pending.push(...next);
    else if (next && typeof next === "object")
      pending.push(...Object.keys(next), ...Object.values(next));
  }
}
function parseLlangRequest(input) {
  const value = record(input, [
    "version",
    "id",
    "body",
    "profile",
    "contract",
    "requirements"
  ]);
  if (value.version !== 2 || value.profile !== "predicate-i32-v1")
    throw new WasmError("INVALID_CAPABILITY", "unsupported request version/profile");
  const requirements = list(value.requirements, "requirements").map(parseRequirement);
  unique(requirements.map((item) => item.id));
  const request = {
    version: 2,
    id: identifier(value.id),
    body: stringValue(value.body, "request body"),
    profile: "predicate-i32-v1",
    contract: parseContract(value.contract),
    requirements
  };
  assertUnicodeScalars(request, "request");
  if (Buffer.byteLength(JSON.stringify(request)) > 1024 * 1024)
    throw new WasmError("INVALID_CAPABILITY", "request exceeds size limit");
  return request;
}
function requestRevision(request) {
  return contentHash(request);
}
function requirementCoverage(request, suite) {
  const requirements = request.requirements.map((requirement) => ({
    id: requirement.id,
    level: requirement.level,
    caseIds: suite.cases.filter((item) => item.requirementIds.includes(requirement.id)).map((item) => item.id)
  }));
  return {
    coverage: request.requirements.length ? "evaluated" : "not-evaluated",
    requirements,
    uncoveredRequirements: requirements.filter((requirement) => requirement.level !== "should" && !requirement.caseIds.length).map((requirement) => requirement.id)
  };
}
function caseInput2(value) {
  const input = { ...value.input };
  for (const key of value.undefinedFields) {
    if (Object.hasOwn(input, key))
      throw new WasmError("INVALID_CAPABILITY", "undefined field also has a value");
    Object.defineProperty(input, key, { value: undefined, enumerable: true });
  }
  return input;
}
function parseCaseData(input, contract) {
  if (!input || typeof input !== "object" || Array.isArray(input) || Object.getPrototypeOf(input) !== Object.prototype && Object.getPrototypeOf(input) !== null || Object.getOwnPropertySymbols(input).length)
    throw new WasmError("INVALID_CAPABILITY", "test input must be JSON data");
  const known2 = new Set(contract.fields.map((field) => field.name));
  const entries = Object.entries(Object.getOwnPropertyDescriptors(input));
  if (entries.some(([key, descriptor]) => !known2.has(key) || !("value" in descriptor) || descriptor.value === undefined || descriptor.value !== null && typeof descriptor.value !== "boolean" && typeof descriptor.value !== "string"))
    throw new WasmError("INVALID_CAPABILITY", "test input must use known JSON scalar fields; use undefinedFields for undefined");
  return Object.fromEntries(entries.map(([key, descriptor]) => [
    key,
    descriptor.value
  ]));
}
function parseLlangSuite(input, request) {
  const value = record(input, [
    "version",
    "requestRevision",
    "contractHash",
    "cases"
  ]);
  if (value.version !== 2 || value.requestRevision !== requestRevision(request) || value.contractHash !== contentHash(request.contract))
    throw new WasmError("INVALID_CAPABILITY", "suite request or contract mismatch");
  const known2 = new Set(request.requirements.map((item) => item.id));
  const cases = list(value.cases, "cases").map((raw) => {
    const item = record(raw, [
      "id",
      "requirementIds",
      "input",
      "undefinedFields",
      "expected"
    ]);
    const requirementIds = list(item.requirementIds, "requirementIds").map(identifier);
    unique(requirementIds);
    if (requirementIds.some((id) => !known2.has(id)))
      throw new WasmError("INVALID_CAPABILITY", "unknown requirement id");
    const input2 = parseCaseData(item.input, request.contract);
    const undefinedFields = list(item.undefinedFields, "undefinedFields").map((field) => stringValue(field, "undefined field"));
    unique(undefinedFields);
    const contractFields = new Set(request.contract.fields.map((field) => field.name));
    if (undefinedFields.some((field) => !contractFields.has(field)))
      throw new WasmError("INVALID_CAPABILITY", "unknown undefined field");
    const expectation2 = record(item.expected, ["kind", "value", "code"]);
    let expected;
    if (expectation2.kind === "value" && typeof expectation2.value === "boolean" && !Object.hasOwn(expectation2, "code"))
      expected = { kind: "value", value: expectation2.value };
    else if (expectation2.kind === "error" && expectation2.code === "INVALID_INPUT" && !Object.hasOwn(expectation2, "value"))
      expected = { kind: "error", code: "INVALID_INPUT" };
    else
      throw new WasmError("INVALID_CAPABILITY", "unsupported test expectation");
    const result = {
      id: identifier(item.id),
      requirementIds,
      input: input2,
      undefinedFields,
      expected
    };
    const materialized = caseInput2(result);
    if (expected.kind === "value")
      encodeInput(request.contract, materialized);
    else {
      try {
        encodeInput(request.contract, materialized);
        throw new WasmError("INVALID_CAPABILITY", "error expectation requires invalid input");
      } catch (error) {
        if (!(error instanceof WasmError) || error.code !== "INVALID_INPUT")
          throw error;
      }
    }
    return result;
  });
  unique(cases.map((item) => item.id));
  if (!cases.some((item) => item.expected.kind === "value" && item.expected.value) || !cases.some((item) => item.expected.kind === "value" && !item.expected.value))
    throw new WasmError("INVALID_CAPABILITY", "suite needs positive and negative cases");
  const suite = {
    version: 2,
    requestRevision: String(value.requestRevision),
    contractHash: String(value.contractHash),
    cases
  };
  assertUnicodeScalars(suite, "suite");
  if (Buffer.byteLength(JSON.stringify(suite)) > 1024 * 1024)
    throw new WasmError("INVALID_CAPABILITY", "suite exceeds size limit");
  return suite;
}
function parseLlangCapabilityMetadata(input) {
  const value = record(input, [
    "id",
    "release",
    "purpose",
    "useWhen",
    "doNotUseWhen"
  ]);
  const metadata = {
    id: identifier(value.id),
    release: identifier(value.release),
    purpose: stringValue(value.purpose, "purpose"),
    useWhen: stringValue(value.useWhen, "useWhen"),
    doNotUseWhen: stringValue(value.doNotUseWhen, "doNotUseWhen")
  };
  assertUnicodeScalars(metadata, "metadata");
  return metadata;
}
function parseLlangCapabilityManifest(input) {
  const value = record(input, [
    "version",
    "metadata",
    "profile",
    "output",
    "permissions",
    "files"
  ]);
  if (value.version !== 2 || value.profile !== "predicate-i32-v1" || value.output !== "boolean" || !Array.isArray(value.permissions) || value.permissions.length)
    throw new WasmError("INVALID_CAPABILITY", "unsupported capability contract");
  const rawFiles = record(value.files, [...roles2]);
  const files = {};
  for (const role of roles2) {
    const ref = record(rawFiles[role], ["path", "hash"]);
    if (typeof ref.path !== "string" || !/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/.test(ref.path) || ref.path === "capability.json")
      throw new WasmError("INVALID_CAPABILITY", "invalid package path");
    if (typeof ref.hash !== "string" || !/^[a-f0-9]{64}$/.test(ref.hash))
      throw new WasmError("INVALID_CAPABILITY", "invalid file hash");
    files[role] = { path: ref.path, hash: ref.hash };
  }
  if (new Set(roles2.map((role) => files[role].path)).size !== roles2.length)
    throw new WasmError("INVALID_CAPABILITY", "duplicate package path");
  return {
    version: 2,
    metadata: parseLlangCapabilityMetadata(value.metadata),
    profile: "predicate-i32-v1",
    output: "boolean",
    permissions: [],
    files
  };
}
async function regularBytes(path) {
  const info = await lstat2(path);
  if (!info.isFile() || info.isSymbolicLink())
    throw new WasmError("INVALID_CAPABILITY", "expected regular non-symlink file");
  if (info.size > 1024 * 1024)
    throw new WasmError("INVALID_CAPABILITY", "capability file exceeds size limit");
  const bytes = await readFile2(path);
  if (bytes.byteLength > 1024 * 1024)
    throw new WasmError("INVALID_CAPABILITY", "capability file exceeds size limit");
  return bytes;
}
function json2(bytes, path) {
  return parseStrictJsonObject(decodeUtf8(bytes, path), path);
}
async function readLlangCapability(path) {
  const absolute = resolve3(path);
  const manifestBytes = await regularBytes(absolute);
  const manifest = parseLlangCapabilityManifest(json2(manifestBytes, absolute));
  const root = dirname2(absolute);
  const files = {};
  for (const role of roles2) {
    const file = await resolveContainedFile(root, manifest.files[role].path, role, { rejectSymbolicLinks: true });
    files[role] = await regularBytes(file);
    if (digest(files[role]) !== manifest.files[role].hash)
      throw new WasmError("INVALID_CAPABILITY", `${role} hash mismatch`);
  }
  const request = parseLlangRequest(json2(files.request, manifest.files.request.path));
  const sourceText = decodeUtf8(files.source, manifest.files.source.path);
  const checked = checkLlangProgram(sourceText, manifest.files.source.path).checked;
  if (!checked)
    throw new WasmError("INVALID_CAPABILITY", "invalid packaged L-Lang source");
  const build = parseLlangBuildManifest(json2(files.build, manifest.files.build.path));
  const suite = parseLlangSuite(json2(files.tests, manifest.files.tests.path), request);
  if (manifest.metadata.id !== request.id || checked.program.id !== request.id || contentHash(checked.program.contract) !== contentHash(request.contract) || build.sourceHash !== checked.sourceHash || build.programHash !== checked.programHash || contentHash(build.contract) !== contentHash(request.contract) || build.file !== manifest.files.wasm.path || build.wasmHash !== digest(files.wasm))
    throw new WasmError("INVALID_CAPABILITY", "request/source/build linkage mismatch");
  if (digest(await regularBytes(absolute)) !== digest(manifestBytes))
    throw new WasmError("INVALID_CAPABILITY", "manifest changed while reading");
  validateLlangArtifact(build, files.wasm);
  return {
    manifest,
    packageHash: contentHash(manifest),
    request,
    sourceText,
    checked,
    build,
    suite,
    bytes: files.wasm
  };
}

// ../L-Lang/src/llang-capability-verifier.ts
init_prompt_source();
init_wasm_contract();
async function runIsolated(snapshot) {
  return new Promise((resolveResult, reject) => {
    const worker = new Worker(new URL("./llang-capability-worker.ts", import.meta.url).href);
    const timer = setTimeout(() => {
      worker.terminate();
      reject(new WasmError("EXECUTION_TIMEOUT", "L-Lang capability verification exceeded 10 seconds"));
    }, 1e4);
    const finish = () => {
      clearTimeout(timer);
      worker.terminate();
    };
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
      build: snapshot.build,
      bytes: snapshot.bytes,
      suite: snapshot.suite
    });
  });
}
async function verifyLlangCapability(path) {
  const report = {
    version: 2,
    verifier: LLANG_CAPABILITY_VERIFIER,
    packageHash: null,
    status: "error",
    acceptance: "not-run",
    apiCalls: 0,
    requestRevision: null,
    suiteHash: null,
    sourceHash: null,
    programHash: null,
    artifactHash: null,
    results: [],
    requirements: [],
    uncoveredRequirements: [],
    coverage: "not-evaluated",
    diagnostics: []
  };
  try {
    const snapshot = await readLlangCapability(path);
    report.packageHash = snapshot.packageHash;
    report.requestRevision = requestRevision(snapshot.request);
    report.suiteHash = contentHash(snapshot.suite);
    report.sourceHash = snapshot.checked.sourceHash;
    report.programHash = snapshot.checked.programHash;
    report.artifactHash = snapshot.build.wasmHash;
    const coverage = requirementCoverage(snapshot.request, snapshot.suite);
    report.coverage = coverage.coverage;
    report.requirements = coverage.requirements.map(({ id, caseIds }) => ({
      id,
      caseIds
    }));
    report.uncoveredRequirements = coverage.uncoveredRequirements;
    report.results = await runIsolated(snapshot);
    if ((await readLlangCapability(path)).packageHash !== snapshot.packageHash)
      throw new WasmError("INVALID_CAPABILITY", "package changed during verification");
    report.status = report.results.some((item) => item.status === "error") ? "error" : report.results.some((item) => item.status === "fail") || report.uncoveredRequirements.length ? "fail" : "pass";
  } catch (error) {
    report.diagnostics.push((error instanceof Error ? error.message : String(error)).slice(0, 4096));
  }
  return report;
}

// ../L-Lang/src/capability-snapshot.ts
init_wasm_contract();
async function packageVersion(path) {
  const manifest = parseStrictJsonObject(decodeUtf8(await regularBytes(path), path), path);
  const version = manifest.version;
  if (version !== 1 && version !== 2)
    throw new WasmError("INVALID_CAPABILITY", "unsupported package version");
  return version;
}
async function readCapabilitySnapshot(path) {
  if (await packageVersion(path) === 2) {
    const snapshot = await readLlangCapability(path);
    return { ...snapshot, source: snapshot.request };
  }
  return readCapability(path);
}
async function verifyCapabilityPackage(path) {
  return await packageVersion(path) === 2 ? verifyLlangCapability(path) : verifyCapability(path);
}

// ../L-Lang/src/capability-host.ts
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
  return new Promise((resolve4, reject) => {
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
        resolve4(event.data.value);
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
    const snapshot = await readCapabilitySnapshot(manifestPath);
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
      const report = await verifyCapabilityPackage(manifestPath);
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
    if ((await readCapabilitySnapshot(manifestPath)).packageHash !== packageHash)
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

// ../L-Lang/src/capability-host-cli.ts
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
