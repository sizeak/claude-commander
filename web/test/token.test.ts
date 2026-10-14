import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { planRejection, TokenStore, takeHashToken } from "../src/token.ts";

/** A store over one fresh in-memory `Storage`. */
function memoryStore(): TokenStore {
  const s = memoryStorage();
  return new TokenStore(() => s);
}

/** An in-memory `Storage` stand-in. */
function memoryStorage(): Storage {
  const m = new Map<string, string>();
  return {
    getItem: (k) => m.get(k) ?? null,
    setItem: (k, v) => void m.set(k, v),
    removeItem: (k) => void m.delete(k),
    clear: () => m.clear(),
    key: (i) => [...m.keys()][i] ?? null,
    get length() {
      return m.size;
    },
  };
}

function page(hash: string, search = "") {
  const replaced: string[] = [];
  return {
    loc: { hash, pathname: "/", search },
    hist: { replaceState: (_d: unknown, _t: string, url: string) => void replaced.push(url) },
    replaced,
  };
}

describe("TokenStore", () => {
  test("round-trips, and clears on null", () => {
    const store = memoryStore();
    store.set("abc");
    assert.equal(store.get(), "abc");
    store.set(null);
    assert.equal(store.get(), null);
  });

  test("a storage that throws (private mode) degrades to no storage", () => {
    const store = new TokenStore(() => {
      throw new Error("SecurityError");
    });
    store.set("abc");
    assert.equal(store.get(), null);
  });
});

describe("takeHashToken", () => {
  test("takes the token and strips it from the address bar, storing nothing yet", () => {
    // Persisting waits for the server to accept it (see planRejection).
    const store = memoryStore();
    const p = page("#token=s3cret");
    const taken = takeHashToken(p.loc, p.hist, store);
    assert.deepEqual(taken, { token: "s3cret", replacedOther: false });
    assert.equal(store.get(), null);
    assert.deepEqual(p.replaced, ["/"]);
  });

  test("keeps the other fragment params and the query string", () => {
    const store = memoryStore();
    const p = page("#a=1&token=t&b=2", "?x=y");
    takeHashToken(p.loc, p.hist, store);
    assert.deepEqual(p.replaced, ["/?x=y#a=1&b=2"]);
  });

  test("no token (or a blank one) leaves the page and the store alone", () => {
    const store = memoryStore();
    store.set("kept");
    for (const hash of ["", "#", "#a=1", "#token=", "#token=%20"]) {
      const p = page(hash);
      assert.equal(takeHashToken(p.loc, p.hist, store), null, hash);
      assert.deepEqual(p.replaced, [], hash);
    }
    assert.equal(store.get(), "kept");
  });

  test("says when the link replaced a different stored token", () => {
    const store = memoryStore();
    store.set("old");
    const p = page("#token=new");
    assert.deepEqual(takeHashToken(p.loc, p.hist, store), {
      token: "new",
      replacedOther: true,
    });
    // The saved token survives until the link's token is accepted.
    assert.equal(store.get(), "old");

    store.set("new");
    const same = page("#token=new");
    assert.deepEqual(takeHashToken(same.loc, same.hist, store), {
      token: "new",
      replacedOther: false,
    });
  });
});

describe("planRejection: what a 401 does to the saved token", () => {
  test("a stale link falls back to the saved token instead of losing it", () => {
    assert.deepEqual(planRejection({ token: "link", source: "hash" }, "saved"), {
      kind: "fallback",
      token: "saved",
    });
  });

  test("a rejected link with nothing else saved goes to the connect screen", () => {
    assert.deepEqual(planRejection({ token: "link", source: "hash" }, null), {
      kind: "connect",
      clearStored: false,
    });
    // The link carried the saved token itself: that one is bad too.
    assert.deepEqual(planRejection({ token: "same", source: "hash" }, "same"), {
      kind: "connect",
      clearStored: true,
    });
  });

  test("a rejected saved or typed token is forgotten", () => {
    for (const source of ["stored", "submitted"] as const) {
      assert.deepEqual(planRejection({ token: "t", source }, "t"), {
        kind: "connect",
        clearStored: true,
      });
    }
  });

  test("a saved token other than the rejected one is left alone", () => {
    assert.deepEqual(planRejection({ token: "typo", source: "submitted" }, "good"), {
      kind: "connect",
      clearStored: false,
    });
  });
});
