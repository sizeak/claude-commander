import assert from "node:assert/strict";
import { describe, test } from "node:test";
import {
  Api,
  ApiError,
  Auth,
  act,
  errorMessage,
  type TokenSource,
  Unauthorized,
} from "../src/api.ts";

interface Call {
  url: string;
  init: RequestInit;
}

function fakeFetch(status: number, body = ""): { fetch: typeof fetch; calls: Call[] } {
  const calls: Call[] = [];
  const f = (async (url: string, init: RequestInit) => {
    calls.push({ url, init });
    return new Response(status === 204 ? null : body, { status });
  }) as unknown as typeof fetch;
  return { fetch: f, calls };
}

function headersOf(call: Call | undefined): Record<string, string> {
  assert.ok(call, "no request was made");
  return call.init.headers as Record<string, string>;
}

function apiWith(status: number, body = "", auth = new Auth()) {
  const { fetch, calls } = fakeFetch(status, body);
  const rejected: (string | undefined)[] = [];
  const api = new Api(auth, { onUnauthorized: (r) => rejected.push(r.message), fetch });
  return { api, calls, rejected, auth };
}

describe("errorMessage", () => {
  test("reads the ApiErrorBody envelope", () => {
    assert.equal(errorMessage(409, { error: { kind: "session", message: "busy" } }), "busy");
  });
  test("falls back to the status", () => {
    assert.equal(errorMessage(500, undefined), "HTTP 500");
    assert.equal(errorMessage(502, { error: "a bare string" }), "HTTP 502");
  });
});

describe("Api.request", () => {
  test("sends the bearer token and a JSON body under /api", async () => {
    const auth = new Auth();
    auth.set("t0k", "stored");
    const { api, calls } = apiWith(200, '{"added":1,"skipped":0}', auth);
    assert.deepEqual(await api.scanProjects({ path: "/x" }), { added: 1, skipped: 0 });
    const [call] = calls;
    assert.equal(call?.url, "/api/projects/scan");
    assert.equal(call?.init.method, "POST");
    const headers = headersOf(call);
    assert.equal(headers.Authorization, "Bearer t0k");
    assert.equal(headers["Content-Type"], "application/json");
    assert.equal(call?.init.body, '{"path":"/x"}');
  });

  test("sends no Authorization header without a token", async () => {
    const { api, calls } = apiWith(200, "{}");
    await api.workspace();
    assert.equal(headersOf(calls[0]).Authorization, undefined);
  });

  test("an empty body (204) resolves to undefined", async () => {
    const { api } = apiWith(204);
    assert.equal(await api.killSession("s1"), undefined);
  });

  test("a non-2xx rejects with the server's message", async () => {
    const { api } = apiWith(404, '{"error":{"kind":"session","message":"no such session"}}');
    await assert.rejects(api.review("s1"), (e) => {
      assert.ok(e instanceof ApiError);
      assert.equal(e.status, 404);
      assert.equal(e.message, "no such session");
      return true;
    });
  });

  test("a 401 forgets the token and reports it once", async () => {
    const auth = new Auth();
    auth.set("stale", "stored");
    const { api, rejected } = apiWith(401, "", auth);
    await assert.rejects(api.workspace(), Unauthorized);
    assert.equal(auth.token, null);
    assert.equal(rejected.length, 1);
  });

  test("a second 401 (a poll already in flight) does not re-report", async () => {
    // Re-reporting would re-open the connect screen with no message, wiping
    // the "rejected" the user was just shown.
    const auth = new Auth();
    auth.set("typo", "submitted");
    const { api, rejected } = apiWith(401, "", auth);
    await Promise.allSettled([api.workspace(), api.agentStates()]);
    assert.deepEqual(rejected, ["That token was rejected."]);
  });
});

describe("act", () => {
  test("wraps a success", async () => {
    assert.deepEqual(await act(Promise.resolve(3), assert.fail), { value: 3 });
  });
  test("reports a failure and yields null", async () => {
    const seen: string[] = [];
    const r = await act(Promise.reject(new ApiError(500, "boom")), (m) => seen.push(m));
    assert.equal(r, null);
    assert.deepEqual(seen, ["Action failed: boom"]);
  });
  test("stays quiet on a 401 (the connect screen handles it)", async () => {
    const r = await act(Promise.reject(new Unauthorized()), assert.fail);
    assert.equal(r, null);
  });
});

describe("Auth: what a rejection says", () => {
  const rejectedFrom = (source: TokenSource) => {
    const auth = new Auth();
    auth.set("tok", source);
    return auth.reject()?.message;
  };

  test("a token the user just submitted is reported as rejected", () => {
    assert.equal(rejectedFrom("submitted"), "That token was rejected.");
  });

  test("a stored or linked token going stale says nothing: nothing was submitted", () => {
    assert.equal(rejectedFrom("stored"), undefined);
    assert.equal(rejectedFrom("hash"), undefined);
    assert.equal(new Auth().reject()?.message, undefined);
  });

  test("a rejection holds until a new token is set", () => {
    const auth = new Auth();
    assert.equal(auth.rejected, false);
    auth.set("tok", "submitted");
    auth.reject();
    assert.equal(auth.rejected, true);
    // A second 401 (e.g. a poll already in flight) must not clear it.
    auth.reject();
    assert.equal(auth.rejected, true);
    auth.set("next", "submitted");
    assert.equal(auth.rejected, false);
  });
});

describe("request timeout", () => {
  // A fetch that never answers: a half-open TCP connection after a laptop
  // sleeps. Without a bound, the poll awaiting it never finishes, so the
  // poller never schedules another and the header says "connected" forever.
  const hung = (() => new Promise<Response>(() => {})) as unknown as typeof fetch;

  test("a request that never answers fails after the timeout", async () => {
    const api = new Api(new Auth(), {
      onUnauthorized: () => assert.fail("no 401 here"),
      fetch: hung,
      timeoutMs: 20,
    });
    await assert.rejects(api.workspace(), (e) => {
      assert.ok(e instanceof ApiError, String(e));
      assert.equal(e.status, 0);
      assert.match(e.message, /timed out/);
      return true;
    });
  });

  test("the fetch is handed an abort signal that fires at the timeout", async () => {
    let signal: AbortSignal | null | undefined;
    const f = ((_u: string, init: RequestInit) => {
      signal = init.signal;
      return new Promise<Response>(() => {});
    }) as unknown as typeof fetch;
    const api = new Api(new Auth(), {
      onUnauthorized: () => assert.fail("no 401 here"),
      fetch: f,
      timeoutMs: 20,
    });
    await assert.rejects(api.workspace(), ApiError);
    assert.equal(signal?.aborted, true, "the hung connection is torn down, not leaked");
  });

  test("a body that never finishes is bounded too", async () => {
    const f = (async () =>
      new Response(new ReadableStream({ start() {} }), {
        status: 200,
      })) as unknown as typeof fetch;
    const api = new Api(new Auth(), {
      onUnauthorized: () => assert.fail("no 401 here"),
      fetch: f,
      timeoutMs: 20,
    });
    await assert.rejects(api.workspace(), /timed out/);
  });
});

describe("a 401 for a token that is no longer current", () => {
  test("is ignored: a new token set mid-request must not be forgotten", async () => {
    // A poll sent with the old token is still in flight when the user
    // submits a new one; its 401 is about the old token, not the new.
    const auth = new Auth();
    auth.set("old", "stored");
    let answer: (r: Response) => void = () => {};
    const fetch = (() =>
      new Promise<Response>((resolve) => {
        answer = resolve;
      })) as unknown as typeof globalThis.fetch;
    const rejected: (string | undefined)[] = [];
    const api = new Api(auth, { onUnauthorized: (r) => rejected.push(r.message), fetch });

    const inFlight = api.workspace();
    auth.set("new", "submitted");
    answer(new Response(null, { status: 401 }));
    await assert.rejects(inFlight, Unauthorized);

    assert.equal(auth.token, "new");
    assert.equal(auth.rejected, false);
    assert.deepEqual(rejected, []);
  });

  test("Auth.reject ignores a stale token and acts on the current one", () => {
    const auth = new Auth();
    auth.set("current", "submitted");
    assert.equal(auth.reject("stale"), null);
    assert.equal(auth.token, "current");
    assert.deepEqual(auth.reject("current"), {
      message: "That token was rejected.",
      token: "current",
      source: "submitted",
    });
    assert.equal(auth.token, null);
  });
});
