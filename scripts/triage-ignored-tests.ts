#!/usr/bin/env npx tsx
/**
 * triage-ignored-tests.ts — Run all ignored harness tests and categorize failures.
 *
 * Uses nextest's `--message-format libtest-json-plus` to run all ignored tests
 * in a single invocation, then parses the structured TRIAGE_JSON lines that the
 * test harness emits on failure.
 *
 * Usage:
 *   pnpm run triage
 *   npx tsx scripts/triage-ignored-tests.ts --json    # JSON to stdout only
 *   npx tsx scripts/triage-ignored-tests.ts --out report.json
 */

import { execSync } from "child_process";
import { readFileSync, writeFileSync } from "fs";
import { resolve } from "path";
import { z } from "zod";

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

const PROJECT_DIR = resolve(import.meta.dirname ?? __dirname, "..");
const IGNORE_TOML = resolve(PROJECT_DIR, "thevenin/tests/ignore.toml");
const DEFAULT_OUT = resolve(PROJECT_DIR, "triage-report.json");

const args = process.argv.slice(2);
const jsonMode = args.includes("--json");
const outIdx = args.indexOf("--out");
const outPath = outIdx !== -1 ? resolve(args[outIdx + 1]) : DEFAULT_OUT;

// ---------------------------------------------------------------------------
// Schemas
// ---------------------------------------------------------------------------

const Category = z.enum([
  "PASSES_NOW",
  "NEAR_MISS",
  "CONVERGENCE",
  "VACUOUS",
  "MISSING_FEATURE",
  "TIMEOUT",
  "CRASH",
  "OTHER",
]);
type Category = z.infer<typeof Category>;

const TriageInfo = z.object({
  path: z.string(),
  phase: z.string(),
  category: Category,
  error: z.string(),
});

const NextestEvent = z.object({
  type: z.string(),
  event: z.string().optional(),
  name: z.string().optional(),
  exec_time: z.number().optional(),
  stdout: z.string().optional(),
});

const TestResult = z.object({
  path: z.string(),
  testName: z.string(),
  ignoreReason: z.string(),
  category: Category,
  phase: z.string(),
  errorMessage: z.string(),
  errorPct: z.number().nullable(),
});
type TestResult = z.infer<typeof TestResult>;

const TriageReport = z.object({
  generated: z.string(),
  summary: z.record(z.string(), z.number()),
  tests: z.array(TestResult),
});

// ---------------------------------------------------------------------------
// Parse ignore.toml
// ---------------------------------------------------------------------------

function parseIgnoreToml(): Map<string, string> {
  const content = readFileSync(IGNORE_TOML, "utf-8");
  const tests = new Map<string, string>();
  for (const line of content.split("\n")) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;
    const match = trimmed.match(/^"([^"]+)"\s*=\s*"(.+)"$/);
    if (match) tests.set(match[1], match[2]);
  }
  return tests;
}

// ---------------------------------------------------------------------------
// Derive test name (must match the proc macro's naming)
// ---------------------------------------------------------------------------

function deriveTestName(path: string): string {
  return "harness_" + path.replace(/\.cir$/, "").replace(/[/-]/g, "_");
}

// ---------------------------------------------------------------------------
// Extract error percentage from error message or ignore reason
// ---------------------------------------------------------------------------

function extractErrorPct(errorMsg: string, ignoreReason: string): number | null {
  // From interpolation mismatch: compute relative error
  const interpMatch = errorMsg.match(/expected ([\d.e+-]+), got ([\d.e+-]+)/);
  if (interpMatch) {
    const expected = parseFloat(interpMatch[1]);
    const got = parseFloat(interpMatch[2]);
    if (expected !== 0) return Math.abs((got - expected) / expected) * 100;
  }

  // From percentage in ignore reason: "~X.Y% error"
  const pctMatch = ignoreReason.match(/~([\d.]+)%/);
  if (pctMatch) return parseFloat(pctMatch[1]);

  return null;
}

// ---------------------------------------------------------------------------
// Categorize from ignore reason (fallback when TRIAGE_JSON isn't available)
// ---------------------------------------------------------------------------

function categorizeFromReason(reason: string): Category {
  if (/singular.?matrix|non-convergence|NR.*converge/i.test(reason)) return "CONVERGENCE";
  if (/~[\d.]+\s*% error|error at|mismatch/i.test(reason)) return "NEAR_MISS";
  if (/not implemented|not.*supported|requires/i.test(reason)) return "MISSING_FEATURE";
  if (/empty output|vacuous/i.test(reason)) return "VACUOUS";
  if (/times? out/i.test(reason)) return "TIMEOUT";
  return "OTHER";
}

// ---------------------------------------------------------------------------
// Priority for sorting (lower = fix first)
// ---------------------------------------------------------------------------

const CATEGORY_PRIORITY: Record<Category, number> = {
  PASSES_NOW: 0,
  VACUOUS: 1,
  NEAR_MISS: 2,
  CRASH: 3,
  CONVERGENCE: 4,
  MISSING_FEATURE: 5,
  TIMEOUT: 6,
  OTHER: 7,
};

function sortKey(t: TestResult): number {
  const base = CATEGORY_PRIORITY[t.category] * 1000;
  if (t.category === "NEAR_MISS" && t.errorPct !== null) return base + t.errorPct;
  return base + 500;
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

function log(msg: string) {
  if (!jsonMode) process.stderr.write(msg + "\n");
}

function main() {
  const ignored = parseIgnoreToml();
  log(`Found ${ignored.size} ignored tests. Running all at once via nextest...`);

  // Build first
  log("Building...");
  try {
    execSync("cargo build --tests --package thevenin", {
      cwd: PROJECT_DIR,
      stdio: "pipe",
      timeout: 300_000,
    });
  } catch (err: any) {
    console.error("Build failed:", err.stderr?.toString() ?? err.message);
    process.exit(1);
  }

  // Run all ignored tests in one shot with JSON output
  log("Running tests...\n");
  let rawOutput: string;
  try {
    rawOutput = execSync(
      "cargo nextest run --package thevenin --test harness --run-ignored only --no-fail-fast --message-format libtest-json-plus",
      {
        cwd: PROJECT_DIR,
        encoding: "utf-8",
        timeout: 600_000,
        env: { ...process.env, NEXTEST_EXPERIMENTAL_LIBTEST_JSON: "1" },
        stdio: ["pipe", "pipe", "pipe"],
        maxBuffer: 100 * 1024 * 1024, // 100MB — stdout includes full test output diffs
      },
    );
  } catch (err: any) {
    // nextest exits non-zero when tests fail — that's expected
    rawOutput = err.stdout ?? "";
    if (!rawOutput) {
      console.error("nextest produced no output:", err.stderr?.toString() ?? err.message);
      process.exit(1);
    }
  }

  // Parse the JSON lines from nextest
  const events: z.infer<typeof NextestEvent>[] = [];
  for (const line of rawOutput.split("\n")) {
    if (!line.startsWith("{")) continue;
    const parsed = NextestEvent.safeParse(JSON.parse(line));
    if (parsed.success) events.push(parsed.data);
  }

  // Build a map: testName -> nextest event for completed tests
  const testEvents = new Map<string, z.infer<typeof NextestEvent>>();
  for (const ev of events) {
    if (ev.type === "test" && (ev.event === "failed" || ev.event === "ok")) {
      // nextest name format: "thevenin::harness$harness_foo_bar"
      const name = ev.name?.split("$")[1] ?? ev.name ?? "";
      testEvents.set(name, ev);
    }
  }

  // Match against ignore.toml entries
  const results: TestResult[] = [];

  for (const [path, reason] of ignored) {
    const testName = deriveTestName(path);
    const ev = testEvents.get(testName);

    if (!ev) {
      results.push(
        TestResult.parse({
          path,
          testName,
          ignoreReason: reason,
          category: categorizeFromReason(reason),
          phase: "unknown",
          errorMessage: "test did not run (name mismatch or filtered)",
          errorPct: null,
        }),
      );
      continue;
    }

    if (ev.event === "ok") {
      results.push(
        TestResult.parse({
          path,
          testName,
          ignoreReason: reason,
          category: "PASSES_NOW",
          phase: "complete",
          errorMessage: "",
          errorPct: null,
        }),
      );
      continue;
    }

    // Failed — parse the TRIAGE_JSON line from stdout
    const stdout = ev.stdout ?? "";
    const triageMatch = stdout.match(/^TRIAGE_JSON:(.+)$/m);

    let category: Category = "OTHER";
    let phase = "unknown";
    let errorMessage = "";

    if (triageMatch) {
      const parsed = TriageInfo.safeParse(JSON.parse(triageMatch[1]));
      if (parsed.success) {
        category = parsed.data.category;
        phase = parsed.data.phase;
        errorMessage = parsed.data.error;
      }
    }

    // Fallback: extract from panic message
    if (!errorMessage) {
      const failedMatch = stdout.match(/Test .+? failed: (.+)/);
      if (failedMatch) errorMessage = failedMatch[1].trim();
    }

    // Check for timeout
    if (!triageMatch && /timed? out|SIGTERM|time limit/i.test(stdout)) {
      category = "TIMEOUT";
    }

    // Last resort: categorize from ignore reason
    if (category === "OTHER" && !triageMatch) {
      category = categorizeFromReason(reason);
    }

    results.push(
      TestResult.parse({
        path,
        testName,
        ignoreReason: reason,
        category,
        phase,
        errorMessage: errorMessage.slice(0, 2000),
        errorPct: extractErrorPct(errorMessage, reason),
      }),
    );
  }

  // Sort by priority
  results.sort((a, b) => sortKey(a) - sortKey(b));

  // Build summary
  const summary: Record<string, number> = { total: ignored.size };
  for (const cat of Category.options) {
    summary[cat] = results.filter((t) => t.category === cat).length;
  }

  const report = TriageReport.parse({
    generated: new Date().toISOString(),
    summary,
    tests: results,
  });

  // Output
  const json = JSON.stringify(report, null, 2);
  if (jsonMode) {
    process.stdout.write(json + "\n");
  } else {
    writeFileSync(outPath, json + "\n");
    log(`Report written to ${outPath}`);
  }

  // Human-readable summary
  log("\n=== Triage Summary ===");
  log(`  Total ignored:     ${ignored.size}`);
  for (const [cat, count] of Object.entries(summary)) {
    if (cat === "total" || count === 0) continue;
    log(`  ${cat.padEnd(18)} ${count}`);
  }

  const passesNow = results.filter((t) => t.category === "PASSES_NOW");
  if (passesNow.length > 0) {
    log("\nTests that now PASS (just un-ignore!):");
    for (const t of passesNow) log(`  - ${t.path}`);
  }

  const nearMisses = results.filter((t) => t.category === "NEAR_MISS");
  if (nearMisses.length > 0) {
    log("\nNear-misses (best ROI, sorted by error):");
    for (const t of nearMisses) {
      const pct = t.errorPct !== null ? `${t.errorPct.toFixed(2)}%` : "?%";
      log(`  - ${t.path} (${pct}) [${t.phase}]`);
    }
  }

  const vacuous = results.filter((t) => t.category === "VACUOUS");
  if (vacuous.length > 0) {
    log("\nVacuous passes (formatter bugs):");
    for (const t of vacuous) log(`  - ${t.path}`);
  }
}

main();
