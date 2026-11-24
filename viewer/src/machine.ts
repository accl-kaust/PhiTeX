// The machine a browser runs on, for Copy debug info and a report. The user
// agent string is frozen: Chromium says "Intel Mac OS X 10_15_7" on every
// Mac (Apple silicon, macOS 26 or 27) and "Windows NT 10.0" on Windows 11,
// so it names no machine. Chromium's client hints do
// (navigator.userAgentData.getHighEntropyValues: the system's version, the
// architecture); where there are none (Firefox, Safari) the line says the
// string is the browser's, not the machine's. Asked for when the debug info
// is, never at the start.

/** The client hints asked for. */
const HINTS = ["platformVersion", "architecture", "bitness", "model", "fullVersionList"];

/** What getHighEntropyValues answers (the hints above, and the low-entropy ones). */
export type Hints = {
  platform?: string;
  platformVersion?: string;
  architecture?: string;
  bitness?: string;
  model?: string;
  fullVersionList?: { brand: string; version: string }[];
};

type Nav = {
  hardwareConcurrency?: number;
  userAgent: string;
  userAgentData?: { platform?: string; getHighEntropyValues?(hints: string[]): Promise<Hints> };
};

/**
 * The system, from its client hints: macOS by its own version (macOS 11 and
 * later report it as it is, "27.2.1"); Windows 10 and 11 told apart as
 * Chromium documents (platformVersion's major 13 and up is 11, 1 to 10 is
 * 10, 0 is 7, 8 or 8.1); any other as it says.
 */
export function systemOf(platform: string, version: string): string {
  const v = version.replace(/(\.0)+$/, "");
  const major = Number(version.split(".")[0]);
  if (platform === "Windows" && version) return `${major >= 13 ? "Windows 11" : major >= 1 ? "Windows 10" : "Windows 7, 8 or 8.1"} (${version})`;
  return v ? `${platform} ${v}` : platform;
}

/** The architecture, from its hints: "arm64", "x86-64", "x86 (32-bit)". */
export function archOf(architecture: string, bitness: string): string {
  if (!architecture) return "";
  if (architecture === "x86") return bitness === "64" ? "x86-64" : `x86${bitness ? ` (${bitness}-bit)` : ""}`;
  return `${architecture}${bitness === "64" ? "64" : bitness ? ` (${bitness}-bit)` : ""}`;
}

/** The debug info's machine line from the hints (`hints`, none: the browser's own string, said to be that). */
export function machineText(cores: number | undefined, userAgent: string, hints?: Hints): string {
  const n = `${cores ?? "?"} cores`;
  if (!hints?.platform) return `machine: ${n}; the browser's reported user agent, which may not name this machine's system or chip: ${userAgent}`;
  const browser = (hints.fullVersionList ?? []).filter((b) => !/not.?a.?brand/i.test(b.brand)).map((b) => `${b.brand} ${b.version}`).join(", ");
  const parts = [n, systemOf(hints.platform, hints.platformVersion ?? ""), archOf(hints.architecture ?? "", hints.bitness ?? ""), hints.model ?? ""].filter(Boolean);
  return `machine: ${parts.join(", ")}${browser ? `; ${browser}` : ""} (the browser's client hints)`;
}

/** This browser's machine line: its client hints asked for now, where it has them. */
export async function machineLine(nav: Nav = navigator as unknown as Nav): Promise<string> {
  const hints = await nav.userAgentData?.getHighEntropyValues?.(HINTS).catch(() => undefined);
  return machineText(nav.hardwareConcurrency, nav.userAgent, hints);
}
