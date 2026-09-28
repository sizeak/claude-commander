// Where the browser keeps the server's bearer token, and how a pairing link
// hands one over. Pure (storage, location and history are injected), so
// `node --test` covers it.

const TOKEN_KEY = "cc_token";

/**
 * The token in `localStorage`. The accessor itself can throw (some private
 * modes, blocked site data); the token then lives only for the tab.
 */
export class TokenStore {
  private readonly storage: () => Storage | null;

  constructor(storage: () => Storage | null = () => localStorage) {
    this.storage = storage;
  }

  get(): string | null {
    try {
      return this.storage()?.getItem(TOKEN_KEY) ?? null;
    } catch {
      return null;
    }
  }

  set(token: string | null): void {
    try {
      const s = this.storage();
      if (token) s?.setItem(TOKEN_KEY, token);
      else s?.removeItem(TOKEN_KEY);
    } catch {
      // Not persisted.
    }
  }
}

export interface HashToken {
  token: string;
  /** A different token was stored before; the link replaced it. */
  replacedOther: boolean;
}

/**
 * Take a `#token=…` fragment (a pairing link) and strip it from the address
 * bar so it isn't left in history or a shared screenshot. Other fragment params
 * and the query string are kept. The fragment never reaches the server in a
 * request.
 *
 * A link token is tried ahead of a stored one: following a pairing link is the
 * user choosing that server's token (typically after it was rotated). It is
 * *not* stored here, though. The caller persists it once the server has
 * accepted it, so a stale link cannot cost the user a saved token that still
 * works (see `planRejection`). `replacedOther` tells the caller to say so on
 * screen when it does replace one.
 */
export function takeHashToken(
  loc: Pick<Location, "hash" | "pathname" | "search">,
  hist: Pick<History, "replaceState">,
  store: TokenStore,
): HashToken | null {
  const params = new URLSearchParams(loc.hash.replace(/^#/, ""));
  const token = (params.get("token") ?? "").trim();
  if (!token) return null;
  params.delete("token");
  const rest = params.toString();
  hist.replaceState(null, "", loc.pathname + loc.search + (rest ? `#${rest}` : ""));
  const previous = store.get();
  return { token, replacedOther: previous !== null && previous !== token };
}

/** What to do after the server refused a token. */
export type RejectionPlan =
  /** Carry on with the saved token (a stale link over a good saved one). */
  | { kind: "fallback"; token: string }
  /** Ask on the connect screen; forget the saved token if it was the one refused. */
  | { kind: "connect"; clearStored: boolean };

/** Decide what a refusal of `rejected` means, given the token saved in storage. */
export function planRejection(
  rejected: { token: string | null; source: string },
  stored: string | null,
): RejectionPlan {
  if (rejected.source === "hash" && stored !== null && stored !== rejected.token) {
    return { kind: "fallback", token: stored };
  }
  return { kind: "connect", clearStored: stored !== null && stored === rejected.token };
}
