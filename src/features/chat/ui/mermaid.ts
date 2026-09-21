const SOURCE_LIMIT_BYTES = 8 * 1_024;
const RENDER_TIMEOUT_MS = 3_000;
const SVG_NAMESPACE = "http://www.w3.org/2000/svg";
const ALLOWED_TAGS = new Set([
  "svg",
  "g",
  "path",
  "rect",
  "circle",
  "ellipse",
  "line",
  "polyline",
  "polygon",
  "text",
  "tspan",
  "marker",
  "defs",
  "style",
]);
const ALLOWED_ATTRIBUTES = new Set([
  "aria-describedby",
  "aria-label",
  "aria-labelledby",
  "class",
  "cx",
  "cy",
  "d",
  "dominant-baseline",
  "fill",
  "fill-opacity",
  "font-family",
  "font-size",
  "font-style",
  "font-weight",
  "height",
  "id",
  "marker-end",
  "marker-height",
  "marker-mid",
  "marker-start",
  "marker-units",
  "marker-width",
  "orient",
  "points",
  "preserveAspectRatio",
  "refX",
  "refY",
  "role",
  "rx",
  "ry",
  "stroke",
  "stroke-dasharray",
  "stroke-linecap",
  "stroke-linejoin",
  "stroke-opacity",
  "stroke-width",
  "text-anchor",
  "transform",
  "viewBox",
  "width",
  "x",
  "x1",
  "x2",
  "y",
  "y1",
  "y2",
]);

let mermaidModule: Promise<typeof import("mermaid")> | null = null;
let nextDiagramId = 1;

async function loadMermaid() {
  mermaidModule ??= import("mermaid").then((module) => {
    module.default.initialize({
      startOnLoad: false,
      securityLevel: "strict",
      htmlLabels: false,
      theme: "neutral",
    });
    return module;
  });
  return mermaidModule;
}

function safeAttribute(name: string, value: string): boolean {
  if (!ALLOWED_ATTRIBUTES.has(name) || /^on/i.test(name)) return false;
  if (/url\s*\(/i.test(value)) return /^url\(#[A-Za-z0-9_-]+\)$/.test(value.trim());
  return !/javascript:/i.test(value);
}

function safeStyle(css: string): boolean {
  return !/(?:url\s*\(|@|:host\b|::slotted\b|expression\s*\(|javascript:|behavior\s*:)/i.test(css);
}

/** Rebuild Mermaid output from a strict SVG allowlist instead of trusting generated markup. */
export function sanitizeMermaidSvg(svg: string): SVGSVGElement | null {
  const parsed = new DOMParser().parseFromString(svg, "image/svg+xml");
  const sourceRoot = parsed.documentElement;
  if (sourceRoot.localName !== "svg" || parsed.querySelector("parsererror")) return null;

  function copy(source: Element): Element | null {
    if (!ALLOWED_TAGS.has(source.localName)) return null;
    const target = document.createElementNS(SVG_NAMESPACE, source.localName);
    for (const attribute of Array.from(source.attributes)) {
      if (safeAttribute(attribute.name, attribute.value)) {
        target.setAttribute(attribute.name, attribute.value);
      }
    }
    if (source.localName === "style") {
      const css = source.textContent ?? "";
      if (safeStyle(css)) target.textContent = css;
      return target;
    }
    for (const child of Array.from(source.childNodes)) {
      if (child.nodeType === Node.TEXT_NODE) {
        target.append(document.createTextNode(child.textContent ?? ""));
      } else if (child.nodeType === Node.ELEMENT_NODE) {
        const clean = copy(child as Element);
        if (clean) target.append(clean);
      }
    }
    return target;
  }

  return copy(sourceRoot) as SVGSVGElement | null;
}

function timeout<T>(promise: Promise<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    const timer = globalThis.setTimeout(
      () => reject(new Error("Mermaid render timed out")),
      RENDER_TIMEOUT_MS,
    );
    promise.then(
      (value) => {
        globalThis.clearTimeout(timer);
        resolve(value);
      },
      (error) => {
        globalThis.clearTimeout(timer);
        reject(error);
      },
    );
  });
}

export async function renderMermaidDiagrams(root: HTMLElement, failureMessage: string) {
  const sources = Array.from(root.querySelectorAll<HTMLElement>(".mermaid-source"));
  await Promise.all(
    sources.map(async (sourceElement) => {
      const block = sourceElement.closest<HTMLElement>(".mermaid-block");
      const fallback = block?.querySelector("pre");
      if (!block) return;
      if (!fallback) {
        if (block.querySelector(".mermaid-diagram")) {
          block.dataset.mermaidState = "rendered";
          sourceElement.remove();
        }
        return;
      }
      if (["rendering", "rendered"].includes(block.dataset.mermaidState ?? "")) return;
      block.dataset.mermaidState = "rendering";
      block.querySelector(".mermaid-error")?.remove();
      const source = sourceElement.textContent ?? "";
      try {
        if (new TextEncoder().encode(source).byteLength > SOURCE_LIMIT_BYTES) {
          throw new Error("Mermaid source exceeds limit");
        }
        const mermaid = await loadMermaid();
        const result = await timeout(
          mermaid.default.render(`saaa-mermaid-${nextDiagramId++}`, source),
        );
        const svg = sanitizeMermaidSvg(result.svg);
        if (!svg) throw new Error("Unsafe Mermaid SVG");
        svg.setAttribute("role", "img");
        const host = document.createElement("div");
        host.className = "mermaid-diagram";
        svg.style.display = "block";
        svg.style.width = "100%";
        svg.style.height = "auto";
        host.attachShadow({ mode: "open" }).append(svg);
        fallback.replaceWith(host);
        block.dataset.mermaidState = "rendered";
        sourceElement.remove();
      } catch {
        block.dataset.mermaidState = "error";
        const message = document.createElement("span");
        message.className = "mermaid-error";
        message.setAttribute("role", "status");
        message.textContent = failureMessage;
        block.prepend(message);
      }
    }),
  );
}
