import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { TokenStore, takeHashToken } from "../src/token.ts";

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
  test("stores the token and strips it from the address bar", () => {
    const store = memoryStore();
    const p = page("#token=s3cret");
    const taken = takeHashToken(p.loc, p.hist, store);
    assert.deepEqual(taken, { token: "s3cret", replacedOther: false });
    assert.equal(store.get(), "s3cret");
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
    assert.equal(store.get(), "new");

    const same = page("#token=new");
    assert.deepEqual(takeHashToken(same.loc, same.hist, store), {
      token: "new",
      replacedOther: false,
    });
  });
});
