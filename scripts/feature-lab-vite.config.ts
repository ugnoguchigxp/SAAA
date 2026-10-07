import react from "@vitejs/plugin-react";
import type { IncomingMessage, ServerResponse } from "node:http";
import { defineConfig, type Plugin } from "vite";

export const LAB_HOST = "127.0.0.1:1422";
export const LAB_ORIGIN = "http://127.0.0.1:1422";
export const LAB_COOKIE = "saaa_lab_session";

export type LabRequestDecision = "document" | "proxy" | "forbid";

export function labRequestDecision(input: {
  method: string;
  path: string;
  host?: string;
  origin?: string;
  site?: string;
  dest?: string;
  mode?: string;
}): LabRequestDecision {
  if (
    input.dest === "iframe" ||
    input.dest === "object" ||
    input.dest === "embed" ||
    input.site === "cross-site"
  ) {
    return "forbid";
  }
  if (input.host !== LAB_HOST) return "forbid";
  if (input.path.startsWith("/api/")) {
    const changing = input.method !== "GET" && input.method !== "HEAD";
    if (changing && input.origin !== LAB_ORIGIN) return "forbid";
    if (!input.origin && input.site !== "same-origin") return "forbid";
    return "proxy";
  }
  const document = input.method === "GET" && (input.path === "/" || input.path.endsWith(".html"));
  const topLevel = !input.dest || input.dest === "document";
  const navigation = !input.mode || input.mode === "navigate";
  if (document && topLevel && navigation) return "document";
  return "proxy";
}

export function featureLabViteConfig(options: { token?: string; apiTarget?: string } = {}) {
  const plugins: Plugin[] = [react()];
  if (options.token && options.apiTarget) {
    plugins.push(labSessionPlugin(options.token));
  }
  return defineConfig({
    plugins,
    server: {
      port: 1422,
      strictPort: true,
      host: "127.0.0.1",
      hmr: false,
      proxy: options.apiTarget
        ? { "/api": { target: options.apiTarget, changeOrigin: false } }
        : undefined,
    },
  });
}

function labSessionPlugin(token: string): Plugin {
  return {
    name: "saaa-lab-session",
    configureServer(server) {
      server.middlewares.use((req, res, next) => {
        const decision = decisionFor(req);
        if (decision === "forbid") {
          res.statusCode = 403;
          res.end();
          return;
        }
        if (decision === "document") {
          applyDocumentCookie(res, token);
        }
        next();
      });
    },
  };
}

function decisionFor(req: IncomingMessage): LabRequestDecision {
  const url = req.url ?? "/";
  return labRequestDecision({
    method: req.method ?? "GET",
    path: url.split("?")[0] ?? "/",
    host: header(req, "host"),
    origin: header(req, "origin"),
    site: header(req, "sec-fetch-site"),
    dest: header(req, "sec-fetch-dest"),
    mode: header(req, "sec-fetch-mode"),
  });
}

function applyDocumentCookie(res: ServerResponse, token: string): void {
  res.setHeader("Set-Cookie", `${LAB_COOKIE}=${token}; HttpOnly; SameSite=Strict; Path=/api`);
  res.setHeader("X-Frame-Options", "DENY");
  res.setHeader("Content-Security-Policy", "frame-ancestors 'none'");
}

function header(req: IncomingMessage, name: string): string | undefined {
  const value = req.headers[name];
  return Array.isArray(value) ? value[0] : value;
}

export default featureLabViteConfig();
