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
 * Take a `#token=…` fragment (a pairing link): store it, then strip it from
 * the address bar so it isn't left in history or a shared screenshot. Other
 * fragment params and the query string are kept. The fragment never reaches
 * the server in a request.
 *
 * A link token always wins over a stored one: following a pairing link is the
 * user choosing that server's token (typically after it was rotated). It is not
 * silent, though: `replacedOther` tells the caller to say so on screen.
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
  store.set(token);
  return { token, replacedOther: previous !== null && previous !== token };
}
