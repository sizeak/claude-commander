// The commander HTTP API, typed over the shapes generated from
// claude-commander-protocol (src/generated). This is the one place that knows
// about the bearer header and what a 401 means; nothing else calls fetch.
//
// Never hand-write a wire shape here: if a response has no generated type,
// the fix is a `ts` derive in protocol, not an interface in this file.

import type {
  AddProjectRequest,
  AgentStatesSnapshot,
  ApiErrorBody,
  ApplyOutcome,
  ConfigPatch,
  CreatedId,
  CreateOptions,
  CreateSessionOpts,
  NewComment,
  ProjectId,
  ReviewedToggle,
  ReviewSnapshot,
  ScanResponse,
  SessionId,
  Snapshot,
  ToggleReviewed,
} from "./generated/index.ts";

/** Where the current token came from — decides what a rejection says. */
export type TokenSource = "none" | "stored" | "hash" | "submitted";

/** The browser's copy of the server's bearer token. */
export class Auth {
  token: string | null = null;
  source: TokenSource = "none";

  set(token: string | null, source: TokenSource): void {
    this.token = token || null;
    this.source = this.token ? source : "none";
  }

  /** Forget the token after a 401; returns the message the connect screen shows. */
  reject(): string | undefined {
    const message = this.token ? "That token was rejected." : undefined;
    this.set(null, "none");
    return message;
  }

  headers(): Record<string, string> {
    return this.token ? { Authorization: `Bearer ${this.token}` } : {};
  }
}

/** The server answered 401: the token is missing or wrong. */
export class Unauthorized extends Error {
  constructor() {
    super("unauthorized");
    this.name = "Unauthorized";
  }
}

/** Any other non-2xx answer (or a network failure), with a user-facing message. */
export class ApiError extends Error {
  readonly status: number;
  constructor(status: number, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }
}

/** The user-facing message from a non-2xx body (`ApiErrorBody`), else `HTTP <status>`. */
export function errorMessage(status: number, body: unknown): string {
  const detail = (body as Partial<ApiErrorBody> | null)?.error;
  if (detail && typeof detail.message === "string" && detail.message) return detail.message;
  return `HTTP ${status}`;
}

type Method = "GET" | "POST" | "PATCH" | "PUT" | "DELETE";

export interface ApiOptions {
  /** Called on every 401, after the token has been forgotten. */
  onUnauthorized: (message: string | undefined) => void;
  fetch?: typeof fetch;
}

export class Api {
  readonly auth: Auth;
  private readonly onUnauthorized: (message: string | undefined) => void;
  private readonly fetchImpl: typeof fetch;

  constructor(auth: Auth, opts: ApiOptions) {
    this.auth = auth;
    this.onUnauthorized = opts.onUnauthorized;
    this.fetchImpl = opts.fetch ?? ((input, init) => fetch(input, init));
  }

  /**
   * One request. Resolves to the parsed JSON body (`undefined` for an empty
   * one, e.g. a 204); rejects with `Unauthorized` or `ApiError`.
   */
  async request<T>(method: Method, path: string, body?: unknown): Promise<T> {
    const headers: Record<string, string> = { Accept: "application/json", ...this.auth.headers() };
    const init: RequestInit = { method, headers };
    if (body !== undefined) {
      headers["Content-Type"] = "application/json";
      init.body = JSON.stringify(body);
    }
    const res = await this.fetchImpl(`/api${path}`, init);
    if (res.status === 401) {
      this.onUnauthorized(this.auth.reject());
      throw new Unauthorized();
    }
    const text = await res.text();
    let parsed: unknown;
    try {
      parsed = text ? JSON.parse(text) : undefined;
    } catch {
      parsed = undefined;
    }
    if (!res.ok) throw new ApiError(res.status, errorMessage(res.status, parsed));
    return parsed as T;
  }

  // ---- workspace -----------------------------------------------------------

  workspace = () => this.request<Snapshot>("GET", "/workspace");
  agentStates = () => this.request<AgentStatesSnapshot>("GET", "/agent-states");
  createOptions = () => this.request<CreateOptions>("GET", "/create-options");

  // ---- sessions --------------------------------------------------------------

  createSession = (opts: CreateSessionOpts) =>
    this.request<CreatedId<SessionId>>("POST", "/sessions", opts);
  restartSession = (id: SessionId) => this.request<unknown>("POST", `/sessions/${id}/restart`);
  killSession = (id: SessionId) => this.request<unknown>("POST", `/sessions/${id}/kill`);
  deleteSession = (id: SessionId) => this.request<unknown>("DELETE", `/sessions/${id}`);

  // ---- projects --------------------------------------------------------------

  addProject = (req: AddProjectRequest) =>
    this.request<CreatedId<ProjectId>>("POST", "/projects", req);
  scanProjects = (req: AddProjectRequest) =>
    this.request<ScanResponse>("POST", "/projects/scan", req);
  removeProject = (id: ProjectId) => this.request<unknown>("DELETE", `/projects/${id}`);

  // ---- config ----------------------------------------------------------------

  /**
   * `GET /config` serves the whole (redacted) config, which has no generated
   * type; the page only reads the patchable fields, which share `ConfigPatch`'s
   * names and types.
   */
  config = () => this.request<ConfigPatch>("GET", "/config");
  patchConfig = (patch: ConfigPatch) => this.request<unknown>("PATCH", "/config", patch);

  // ---- review ----------------------------------------------------------------

  review = (id: SessionId) => this.request<ReviewSnapshot>("GET", `/sessions/${id}/review`);
  addComment = (id: SessionId, c: NewComment) =>
    this.request<CreatedId<string>>("POST", `/sessions/${id}/comments`, c);
  deleteComment = (id: SessionId, cid: string) =>
    this.request<unknown>("DELETE", `/sessions/${id}/comments/${cid}`);
  toggleReviewed = (id: SessionId, req: ToggleReviewed) =>
    this.request<ReviewedToggle>("POST", `/sessions/${id}/files/reviewed`, req);
  applyComments = (id: SessionId) =>
    this.request<ApplyOutcome>("POST", `/sessions/${id}/comments/apply`);
}

/**
 * Run a user-initiated mutation: `{ value }` on success, `null` on failure. A
 * failure is reported through `report` (an alert by default) — except a 401,
 * which the connect screen already handles.
 */
export async function act<T>(
  p: Promise<T>,
  report: (message: string) => void = (m) => alert(m),
): Promise<{ value: T } | null> {
  try {
    return { value: await p };
  } catch (e) {
    if (!(e instanceof Unauthorized)) {
      report(`Action failed: ${e instanceof Error ? e.message : String(e)}`);
    }
    return null;
  }
}
