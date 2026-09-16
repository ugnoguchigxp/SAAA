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

// src/wasm-runtime.ts
async function instantiateWasmPredicate(manifest, bytes) {
  if (digest(bytes) !== manifest.wasmHash)
    throw new WasmError("ARTIFACT_MISMATCH", "Wasm digest differs from manifest");
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
var init_wasm_runtime = __esm(() => {
  init_wasm_artifact();
  init_wasm_contract();
});

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

// src/capability-worker.ts
init_wasm_runtime();
self.onmessage = async (event) => {
  try {
    const { manifest, bytes, source, suite } = event.data;
    const runtime = await instantiateWasmPredicate(manifest, bytes);
    self.postMessage({
      results: runCapabilityCases(source, suite, runtime.evaluate)
    });
  } catch (error) {
    self.postMessage({
      error: error instanceof Error ? error.message : String(error)
    });
  }
};
