import { exchangeLoginCode, googleCallback, googleStart, logout, refresh } from "./authRoutes";
import { GOOGLE_TOKEN_URL } from "./google";
import { errorResponse, HttpError, json, MAX_BODY_BYTES } from "./http";
import * as v1 from "./routes";
import type { Deps, Env } from "./types";

type Handler = (req: Request, env: Env) => Promise<Response>;

/** Builds the request handler. `deps` lets tests replace outbound fetch and Google's token URL. */
export function createHandler(deps: Deps = {}): Handler {
  const full: Required<Deps> = {
    fetch: deps.fetch ?? ((input, init) => fetch(input, init)),
    googleTokenUrl: deps.googleTokenUrl ?? GOOGLE_TOKEN_URL,
  };

  return async (req, env) => {
    try {
      return await route(req, env, full);
    } catch (e) {
      if (e instanceof HttpError) return errorResponse(e);
      // Log only the kind of error: messages can contain request data.
      console.error("unhandled error", e instanceof Error ? e.name : typeof e);
      return json({ error: "server_error", message: "Something went wrong on our side. Please try again." }, 500);
    }
  };
}

async function route(req: Request, env: Env, deps: Required<Deps>): Promise<Response> {
  if (Number(req.headers.get("content-length") ?? "0") > MAX_BODY_BYTES) {
    throw new HttpError(413, "too_large", "That request is too big (the limit is 1 MB).");
  }
  const { pathname } = new URL(req.url);
  const path = pathname.length > 1 ? pathname.replace(/\/+$/, "") : pathname;
  const m = req.method;
  const methodNotAllowed = () => new HttpError(405, "method_not_allowed", "That method is not allowed here.");

  if (path === "/") return json({ ok: true, service: "wisper-api" });

  if (path.startsWith("/auth/")) {
    await enforceRateLimit(req, env);
    switch (path) {
      case "/auth/google/start": return m === "GET" ? googleStart(req, env) : Promise.reject(methodNotAllowed());
      case "/auth/google/callback": return m === "GET" ? googleCallback(req, env, deps) : Promise.reject(methodNotAllowed());
      case "/auth/token": return m === "POST" ? exchangeLoginCode(req, env) : Promise.reject(methodNotAllowed());
      case "/auth/refresh": return m === "POST" ? refresh(req, env) : Promise.reject(methodNotAllowed());
      case "/auth/logout": return m === "POST" ? logout(req, env) : Promise.reject(methodNotAllowed());
    }
    throw notFound();
  }

  if (path.startsWith("/v1/")) {
    const table: Record<string, Partial<Record<string, Handler>>> = {
      "/v1/me": { GET: v1.getMe },
      "/v1/devices": { GET: v1.listDevices },
      "/v1/account": { DELETE: v1.deleteAccount },
      "/v1/keys": { GET: v1.getKeys, DELETE: v1.deleteKeys },
      "/v1/settings": { GET: v1.getSettings, PUT: v1.putSettings },
      "/v1/items": { GET: v1.getItems, POST: v1.postItems },
      "/v1/stats": { GET: v1.getStats, POST: v1.postStats },
    };
    const entry = table[path];
    if (entry) {
      const h = entry[m];
      if (!h) throw methodNotAllowed();
      return h(req, env);
    }
    const key = /^\/v1\/keys\/([^/]+)$/.exec(path);
    if (key) {
      if (m !== "PUT") throw methodNotAllowed();
      return v1.putKey(req, env, safeDecode(key[1]!));
    }
    const dev = /^\/v1\/devices\/([^/]+)$/.exec(path);
    if (dev) {
      if (m !== "DELETE") throw methodNotAllowed();
      return v1.removeDevice(req, env, safeDecode(dev[1]!));
    }
  }
  throw notFound();
}

function safeDecode(s: string): string {
  try {
    return decodeURIComponent(s);
  } catch {
    throw notFound();
  }
}

function notFound(): HttpError {
  return new HttpError(404, "not_found", "That was not found.");
}

async function enforceRateLimit(req: Request, env: Env): Promise<void> {
  if (!env.AUTH_LIMITER) return;
  const ip = req.headers.get("cf-connecting-ip") ?? "unknown";
  const { success } = await env.AUTH_LIMITER.limit({ key: ip });
  if (!success) throw new HttpError(429, "rate_limited", "Too many attempts. Please wait a minute and try again.");
}

export default { fetch: createHandler() } satisfies ExportedHandler<Env>;
