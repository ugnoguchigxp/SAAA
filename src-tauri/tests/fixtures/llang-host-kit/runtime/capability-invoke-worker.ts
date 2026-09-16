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

// src/capability-invoke-worker.ts
init_wasm_runtime();
self.onmessage = async (event) => {
  try {
    const { build, bytes, input } = event.data;
    const runtime = await instantiateWasmPredicate(build, bytes);
    self.postMessage({ value: runtime.evaluate(input) });
  } catch {
    self.postMessage({ error: true });
  }
};
