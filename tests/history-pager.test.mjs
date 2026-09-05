import test from "node:test";
import assert from "node:assert/strict";
import { HistoryPager } from "../src/history-pager.js";

const entry = (id, missing = false) => ({ id, missing });
const deferred = () => {
  let resolve;
  let reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};

test("loads bounded pages with the server cursor and appends unique rows", async () => {
  const calls = [];
  const cursor = { createdAt: "2026-01-01T00:00:00Z", id: "b" };
  const pages = [
    { entries: [entry("a"), entry("b")], nextCursor: cursor },
    { entries: [entry("b"), entry("c")], nextCursor: null },
  ];
  const pager = new HistoryPager(async (args) => { calls.push(args); return pages.shift(); });
  await pager.load({ reset: true });
  const appended = await pager.load();
  assert.deepEqual(appended.entries.map(({ id }) => id), ["c"]);
  assert.deepEqual(pager.items.map(({ id }) => id), ["a", "b", "c"]);
  assert.deepEqual(calls, [{ after: null, pageSize: 50 }, { after: cursor, pageSize: 50 }]);
  assert.equal(await pager.load(), null);
  assert.equal(calls.length, 2);
});

test("a refresh supersedes an in-flight older page without stale rows or loading state", async () => {
  const requests = [];
  const pager = new HistoryPager(() => {
    const request = deferred(); requests.push(request); return request.promise;
  });
  const first = pager.load({ reset: true });
  requests[0].resolve({ entries: [entry("a")], nextCursor: { id: "a" } });
  await first;
  const older = pager.load();
  assert.equal(await pager.load(), null, "duplicate Load more is suppressed");
  const refreshed = pager.load({ reset: true });
  requests[1].resolve({ entries: [entry("stale")], nextCursor: null });
  assert.equal(await older, null);
  assert.equal(pager.loading, true);
  requests[2].resolve({ entries: [entry("new")], nextCursor: null });
  await refreshed;
  assert.deepEqual(pager.items.map(({ id }) => id), ["new"]);
  assert.equal(pager.loading, false);
});

test("later-page failures retain rows and the retry cursor", async () => {
  let calls = 0;
  const cursor = { id: "a" };
  const pager = new HistoryPager(async () => {
    if (calls++ === 0) return { entries: [entry("a")], nextCursor: cursor };
    throw new Error("unavailable");
  });
  await pager.load({ reset: true });
  await assert.rejects(pager.load(), /unavailable/);
  assert.equal(pager.items.length, 1);
  assert.equal(pager.nextCursor, cursor);
  assert.equal(pager.loading, false);
});

test("delete and availability changes win over an already requested refresh", async () => {
  const pending = deferred();
  const pager = new HistoryPager(() => pending.promise);
  pager.items = [entry("deleted"), entry("restored", true)];
  const refresh = pager.load({ reset: true });
  pager.remove("deleted");
  pager.update(entry("restored", false));
  pending.resolve({ entries: [entry("deleted"), entry("restored", true)], nextCursor: { id: "deleted" } });
  await refresh;
  assert.deepEqual(pager.items, [entry("restored", false)]);
  assert.deepEqual(pager.nextCursor, { id: "deleted" }, "a deleted cursor row still defines the boundary");
});
