// A `settled` that comes while the previous version's PDF is loading: the
// open asked for then gets the new version's, loaded again, never the load
// in flight (the CLI viewer's Pages.open, viewer/src/current.ts).
//
//   scripts/viewer-test.sh

import { test } from "node:test";
import assert from "node:assert/strict";
import { Current } from "../src/current.ts";

/** A load of what the server has now, finished when `release` is called. */
function server() {
  let now = 1;
  const pending: Array<() => void> = [];
  const dropped: number[] = [];
  let loads = 0;
  const load = () => {
    loads++;
    const v = now;
    return new Promise<{ v: number } | null>((res) => pending.push(() => res({ v })));
  };
  return {
    load,
    dropped,
    get loads() {
      return loads;
    },
    publish(v: number) {
      now = v;
    },
    async release() {
      while (pending.length) pending.shift()!();
      await new Promise((r) => setTimeout(r, 0));
    },
  };
}

test("a version settled during a load is loaded before the value is given", async () => {
  const s = server();
  const c = new Current(s.load, (d) => s.dropped.push(d.v));
  c.moved();
  const first = c.get();
  // (the build settles version 2 while version 1 loads)
  s.publish(2);
  c.moved();
  const second = c.get();
  await s.release();
  await s.release();
  assert.deepEqual(await second, { v: 2 });
  assert.deepEqual(await first, { v: 2 }, "the first open too: never the stale load");
  assert.equal(s.loads, 2);
  assert.deepEqual(s.dropped, [1]);
  // (nothing new: the value as it is, no load)
  assert.deepEqual(await c.get(), { v: 2 });
  assert.equal(s.loads, 2);
});

test("a load that finds nothing gives nothing", async () => {
  const c = new Current<number>(async () => null);
  c.moved();
  assert.equal(await c.get(), null);
});
