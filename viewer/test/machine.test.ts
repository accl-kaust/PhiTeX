// The debug info's machine line (viewer/src/machine.ts): the system and the
// chip from Chromium's client hints, not its frozen user agent (#60: an
// Apple-silicon Mac on macOS 27 read "Intel Mac OS X 10_15_7"); without
// hints, the user agent said to be the browser's.
//
//   scripts/viewer-test.sh

import { test } from "node:test";
import assert from "node:assert/strict";
import { archOf, machineLine, machineText, systemOf } from "../src/machine.ts";

const FROZEN = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/158.0.0.0 Safari/537.36";

test("an Apple-silicon Mac on macOS 27: its version and chip, not the frozen string", async () => {
  const nav = {
    hardwareConcurrency: 12,
    userAgent: FROZEN,
    userAgentData: {
      platform: "macOS",
      getHighEntropyValues: async (hints: string[]) => {
        assert.deepEqual(hints, ["platformVersion", "architecture", "bitness", "model", "fullVersionList"]);
        return { platform: "macOS", platformVersion: "27.2.1", architecture: "arm", bitness: "64", model: "", fullVersionList: [{ brand: "Not.A/Brand", version: "24.0.0.0" }, { brand: "Google Chrome", version: "158.0.7600.12" }] };
      },
    },
  };
  const l = await machineLine(nav);
  assert.equal(l, "machine: 12 cores, macOS 27.2.1, arm64; Google Chrome 158.0.7600.12 (the browser's client hints)");
  assert.ok(!l.includes("Intel"));
});

test("no client hints (Firefox, Safari): the user agent, said to be the browser's", async () => {
  const ua = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:140.0) Gecko/20100101 Firefox/140.0";
  const l = await machineLine({ hardwareConcurrency: 8, userAgent: ua });
  assert.equal(l, `machine: 8 cores; the browser's reported user agent, which may not name this machine's system or chip: ${ua}`);
  // (hints that fail: the same)
  assert.equal(await machineLine({ hardwareConcurrency: 8, userAgent: ua, userAgentData: { getHighEntropyValues: () => Promise.reject(new Error("no")) } }), l);
});

test("systems and architectures", () => {
  assert.equal(systemOf("Windows", "15.0.0"), "Windows 11 (15.0.0)");
  assert.equal(systemOf("Windows", "10.0.0"), "Windows 10 (10.0.0)");
  assert.equal(systemOf("Windows", "0.3.0"), "Windows 7, 8 or 8.1 (0.3.0)");
  assert.equal(systemOf("macOS", "15.0.0"), "macOS 15");
  assert.equal(systemOf("Linux", ""), "Linux");
  assert.equal(archOf("x86", "64"), "x86-64");
  assert.equal(archOf("x86", "32"), "x86 (32-bit)");
  assert.equal(archOf("arm", "64"), "arm64");
  assert.equal(archOf("", ""), "");
  assert.equal(machineText(undefined, "UA", { platform: "Linux", platformVersion: "6.8.0", architecture: "x86", bitness: "64" }), "machine: ? cores, Linux 6.8, x86-64 (the browser's client hints)");
});
