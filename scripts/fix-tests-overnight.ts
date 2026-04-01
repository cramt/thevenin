#!/usr/bin/env npx tsx
/**
 * Overnight test fixer — uses Claude Agent SDK to repeatedly pick ignored
 * harness tests, fix them, commit & push.
 *
 * Improvements over naive loop (informed by Agent SDK docs + best practices):
 *   - Doesn't inject 1,286-line FIXING_HARNESS_TESTS.md into prompt context;
 *     tells the agent to read it, keeping initial context lean for better quality
 *   - Uses settingSources: ["project"] so CLAUDE.md/skills/hooks load automatically
 *   - Tracks progress: counts #[ignore] before/after, detects no-commit iterations
 *   - Structured deduplication: tracks attempted .cir paths with outcomes in JSON
 *   - Git pull before each iteration to avoid push conflicts
 *   - Handles ResultMessage.subtype properly (success, error_max_turns, etc.)
 *   - Stops early after N consecutive no-progress iterations
 *   - Logs iteration summaries for post-mortem review
 *   - Post-commit regression check: runs full test suite after each commit, reverts if broken
 *   - Periodic triage refresh: re-runs triage every N successes so priorities stay current
 *   - Early exit when all remaining tests are intractable (skip-category match)
 *   - Session resumption: resumes agent on max_turns if progress was being made
 *   - Desktop notification on completion (notify-send)
 *
 * Designed for Claude Max subscription (flat-rate, rate-limited only).
 *
 * Usage:
 *   npx tsx scripts/fix-tests-overnight.ts [options]
 *
 * Options:
 *   --max-iterations N       Max successful agent sessions (default: 50)
 *   --cooldown N             Seconds between iterations (default: 30)
 *   --max-no-progress N      Stop after N consecutive no-commit iterations (default: 3)
 *   --triage-refresh-every N Refresh triage report every N successful iterations (default: 3)
 *
 * Graceful shutdown (finishes current iteration, then exits):
 *   touch scripts/.stop-fix-tests     # stop file — checked between iterations
 *   kill -USR1 $(pgrep -f fix-tests)  # signal — works mid-iteration too
 */

import { query } from "@anthropic-ai/claude-agent-sdk";
import { readFileSync, writeFileSync, appendFileSync, existsSync } from "fs";
import { resolve } from "path";
import { execSync } from "child_process";

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

const cliArgs = process.argv.slice(2);
function flag(name: string, fallback: number): number {
  const idx = cliArgs.indexOf(`--${name}`);
  if (idx !== -1 && cliArgs[idx + 1]) return parseInt(cliArgs[idx + 1], 10);
  return fallback;
}

const MAX_ITERATIONS = flag("max-iterations", 50);
const COOLDOWN_SEC = flag("cooldown", 30);
const MAX_NO_PROGRESS = flag("max-no-progress", 3);
const TRIAGE_REFRESH_EVERY = flag("triage-refresh-every", 3);
const PROJECT_DIR = resolve(import.meta.dirname ?? __dirname, "..");
const LOG_FILE = resolve(PROJECT_DIR, "scripts", "fix-tests.log");
const ATTEMPTS_FILE = resolve(PROJECT_DIR, "scripts", "attempted-tests.json");
const IGNORE_TOML = resolve(PROJECT_DIR, "thevenin", "tests", "ignore.toml");

const STOP_FILE = resolve(PROJECT_DIR, "scripts", ".stop-fix-tests");

// Test categories that are intractable — agent should not waste time on these
const SKIP_CATEGORIES = [
  ".control:",            // .control interpreter features (alter, vector indexing, resume, complex math)
  ".control scripting",   // legacy label
  ".elseif inline",       // pre-existing parser bug
  "XSPICE (US-056)",
  "BSIM1/BSIM2 (US-052/053)",
  "TEMPER keyword",       // legacy label (all TEMPER tests now pass)
  "imaginary unit",       // needs complex number math in vecexpr
  "resume command",       // needs paused simulation infrastructure
];

// Priority order for test selection (highest ROI first)
const PRIORITY_ORDER = [
  "vacuous pass",     // Formatter bugs — often quick fixes
  "~0.",              // Sub-1% errors — closest to passing
  "~1.",              // 1-2% errors
  "~2.",              // 2-3% errors
  "~3.",              // etc.
  "~4.",
  "~5.",
  "DC OP:",           // Convergence failures — harder
  "NR non-convergence",
];

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function timestamp(): string {
  return new Date().toISOString().replace("T", " ").replace(/\.\d+Z$/, "");
}

function log(msg: string) {
  const line = `[${timestamp()}] ${msg}`;
  console.log(line);
  try { appendFileSync(LOG_FILE, line + "\n"); } catch (e) {
    process.stderr.write(`[log write failed: ${e}]\n`);
  }
}

function logError(msg: string) {
  const line = `[${timestamp()}] ERROR: ${msg}`;
  console.error(line);
  try { appendFileSync(LOG_FILE, line + "\n"); } catch (e) {
    process.stderr.write(`[log write failed: ${e}]\n`);
  }
}

async function sleep(seconds: number): Promise<void> {
  for (let remaining = seconds; remaining > 0; remaining--) {
    process.stdout.write(`\r[${timestamp()}] Waiting ${remaining}s...   `);
    await new Promise((r) => setTimeout(r, 1000));
  }
  process.stdout.write("\r" + " ".repeat(60) + "\r");
}

function shell(cmd: string): string {
  try {
    return execSync(cmd, { cwd: PROJECT_DIR, encoding: "utf-8", timeout: 60_000 }).trim();
  } catch (e) {
    logError(`shell command failed: ${cmd} — ${e instanceof Error ? e.message : e}`);
    return "";
  }
}

// ---------------------------------------------------------------------------
// ignore.toml parsing
// ---------------------------------------------------------------------------

/** Parse ignore.toml into a Map of path → reason */
function parseIgnoreToml(): Map<string, string> {
  if (!existsSync(IGNORE_TOML)) return new Map();
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

/** Count ignored tests */
function countIgnoredTests(): number {
  return parseIgnoreToml().size;
}

/** Check if a test's ignore reason matches any skip category */
function isIntractable(reason: string): boolean {
  return SKIP_CATEGORIES.some((cat) => reason.includes(cat));
}

/** Returns the list of tractable (non-skipped) test paths from ignore.toml */
function getTractableTests(): Array<{ path: string; reason: string }> {
  const ignored = parseIgnoreToml();
  const tractable: Array<{ path: string; reason: string }> = [];
  for (const [path, reason] of ignored) {
    if (!isIntractable(reason)) {
      tractable.push({ path, reason });
    }
  }
  return tractable;
}

// ---------------------------------------------------------------------------
// Structured deduplication
// ---------------------------------------------------------------------------

interface AttemptRecord {
  path: string;
  outcome: "fixed" | "failed" | "reverted";
  date: string;
  summary: string;
}

function loadAttempts(): AttemptRecord[] {
  if (!existsSync(ATTEMPTS_FILE)) return [];
  try {
    return JSON.parse(readFileSync(ATTEMPTS_FILE, "utf-8"));
  } catch {
    return [];
  }
}

function saveAttempts(attempts: AttemptRecord[]): void {
  try {
    writeFileSync(ATTEMPTS_FILE, JSON.stringify(attempts, null, 2) + "\n");
  } catch (e) {
    logError(`Failed to write attempts file: ${e}`);
  }
}

/** Record which .cir paths were attempted this iteration by diffing ignore.toml before/after */
function recordAttempts(
  ignoredBefore: Map<string, string>,
  ignoredAfter: Map<string, string>,
  outcome: "fixed" | "failed" | "reverted",
  summary: string,
): void {
  const attempts = loadAttempts();
  const date = new Date().toISOString().slice(0, 10);

  if (outcome === "fixed") {
    // Tests that were removed from ignore.toml = fixed
    for (const path of ignoredBefore.keys()) {
      if (!ignoredAfter.has(path)) {
        attempts.push({ path, outcome: "fixed", date, summary });
      }
    }
  } else {
    // No test was fixed — figure out which tests the agent likely worked on
    // by checking which tractable tests haven't been attempted recently
    // For now, just record a generic "failed" entry with the summary
    attempts.push({ path: "_unknown_", outcome, date, summary });
  }

  saveAttempts(attempts);
}

/** Build a structured dedup section for the prompt */
function buildDedupSection(): string {
  const attempts = loadAttempts();
  if (attempts.length === 0) return "";

  const failed = attempts.filter((a) => a.outcome === "failed" || a.outcome === "reverted");
  const fixed = attempts.filter((a) => a.outcome === "fixed");

  let section = "\n\n## Previously attempted tests";

  if (fixed.length > 0) {
    section += `\n\nAlready fixed (${fixed.length}): ${fixed.map((a) => a.path).join(", ")}`;
  }

  if (failed.length > 0) {
    // Group by path, show most recent attempt
    const byPath = new Map<string, AttemptRecord>();
    for (const a of failed) byPath.set(a.path, a);

    const failedEntries = [...byPath.values()]
      .filter((a) => a.path !== "_unknown_")
      .map((a) => `- ${a.path} (${a.date}): ${a.summary.slice(0, 120)}`);

    const unknownCount = failed.filter((a) => a.path === "_unknown_").length;

    if (failedEntries.length > 0) {
      section += `\n\nPreviously failed — do NOT reattempt unless you have a new approach:\n${failedEntries.join("\n")}`;
    }
    if (unknownCount > 0) {
      section += `\n\n${unknownCount} previous iteration(s) made no progress.`;
    }
  }

  return section;
}

// ---------------------------------------------------------------------------
// Git helpers
// ---------------------------------------------------------------------------

/** Get the latest commit hash */
function latestCommit(): string {
  return shell("git rev-parse HEAD");
}

/** Pull latest changes (rebase to keep linear history) */
function gitPull(): boolean {
  const result = shell("git pull --rebase 2>&1");
  if (result.includes("CONFLICT") || result.includes("error:")) {
    logError(`git pull failed: ${result}`);
    shell("git rebase --abort");
    return false;
  }
  return true;
}

/** Run the full test suite and return true if it passes */
function regressionCheck(): boolean {
  log("Running post-commit regression check...");
  try {
    execSync(
      "nix develop --command cargo nextest run --workspace",
      { cwd: PROJECT_DIR, encoding: "utf-8", timeout: 300_000, stdio: "pipe" },
    );
    log("Regression check passed.");
    return true;
  } catch (err: unknown) {
    const stderr = (err as any).stderr?.toString() ?? "";
    const stdout = (err as any).stdout?.toString() ?? "";
    const summary = (stdout + stderr).split("\n").find((l: string) => /Summary|FAIL/.test(l)) ?? "tests failed";
    logError(`Regression check FAILED: ${summary}`);
    return false;
  }
}

/** Revert all commits back to a known-good commit, restoring a clean working tree */
function revertTo(commitHash: string): void {
  log(`Reverting to ${commitHash.slice(0, 8)}...`);
  shell(`git reset --hard ${commitHash}`);
  shell("git clean -fd");
  log("Reverted. Working tree restored to pre-iteration state.");
}

/** Refresh the triage report by running the triage script */
function refreshTriageReport(): void {
  log("Refreshing triage report...");
  try {
    execSync(
      "npx tsx scripts/triage-ignored-tests.ts --out triage-report.json",
      { cwd: PROJECT_DIR, encoding: "utf-8", timeout: 600_000, stdio: "pipe" },
    );
    log("Triage report refreshed.");
  } catch (err: unknown) {
    const msg = err instanceof Error ? err.message : String(err);
    logError(`Triage refresh failed (non-fatal): ${msg}`);
  }
}

// ---------------------------------------------------------------------------
// Notification
// ---------------------------------------------------------------------------

function notify(title: string, body: string): void {
  try {
    execSync(
      `notify-send --app-name="fix-tests" ${JSON.stringify(title)} ${JSON.stringify(body)}`,
      { timeout: 5_000, stdio: "pipe" },
    );
  } catch {
    // notify-send may not be available — that's fine
  }
}

// ---------------------------------------------------------------------------
// Graceful shutdown
//
// Two ways to request a graceful stop:
//   1. Touch the stop file:  touch scripts/.stop-fix-tests
//   2. Send SIGUSR1:         kill -USR1 <pid>
//
// The current iteration finishes, then the loop exits cleanly.
// ---------------------------------------------------------------------------

let stopRequested = false;

function checkStopRequested(): boolean {
  if (stopRequested) return true;
  if (existsSync(STOP_FILE)) {
    stopRequested = true;
    try { execSync(`rm -f "${STOP_FILE}"`); } catch {}
    log("Stop requested via stop file.");
    return true;
  }
  return false;
}

process.on("SIGUSR1", () => {
  stopRequested = true;
  log("Stop requested via SIGUSR1 — will finish current iteration then exit.");
});

// Also handle SIGINT/SIGTERM gracefully: if we get one while sleeping between
// iterations, exit cleanly instead of printing an ugly stack trace.
let inIteration = false;
for (const sig of ["SIGINT", "SIGTERM"] as const) {
  process.on(sig, () => {
    if (inIteration) {
      // During an iteration, set flag and let it finish
      stopRequested = true;
      log(`${sig} received — will finish current iteration then exit.`);
    } else {
      // Between iterations (cooldown/backoff), exit immediately
      log(`${sig} received — exiting.`);
      process.exit(0);
    }
  });
}

// ---------------------------------------------------------------------------
// Build the prompt (lean — no inline doc injection)
// ---------------------------------------------------------------------------

function buildPrompt(resumeSessionId?: string): string {
  const dedupSection = buildDedupSection();

  // Try to load triage report if available for better prioritization
  let triageSection = "";
  try {
    const triagePath = resolve(PROJECT_DIR, "triage-report.json");
    if (existsSync(triagePath)) {
      const triage = JSON.parse(readFileSync(triagePath, "utf-8"));
      const nearMisses = (triage.tests ?? [])
        .filter((t: any) => t.category === "NEAR_MISS")
        .sort((a: any, b: any) => {
          const pa = parseFloat(a.error_pct) || 999;
          const pb = parseFloat(b.error_pct) || 999;
          return pa - pb;
        })
        .slice(0, 10);
      const vacuous = (triage.tests ?? []).filter((t: any) => t.category === "VACUOUS");
      const passesNow = (triage.tests ?? []).filter((t: any) => t.category === "PASSES_NOW");

      if (passesNow.length > 0) {
        triageSection += `\n\nTests that ALREADY PASS (just remove from ignore.toml and commit!):\n${passesNow.map((t: any) => `- ${t.path}`).join("\n")}`;
      }
      if (vacuous.length > 0) {
        triageSection += `\n\nVacuous-pass tests (formatter bugs — often quick fixes):\n${vacuous.map((t: any) => `- ${t.path}: ${t.error_message}`).join("\n")}`;
      }
      if (nearMisses.length > 0) {
        triageSection += `\n\nNearest misses (sorted by error magnitude — best ROI):\n${nearMisses.map((t: any) => `- ${t.path} (${t.error_pct || "unknown %"}): ${t.error_message?.slice(0, 120)}`).join("\n")}`;
      }
      triageSection += `\n\nTriage summary: ${JSON.stringify(triage.summary)}`;
    }
  } catch {}

  if (resumeSessionId) {
    return `You hit the turn limit but were making progress (commits were made).
Continue where you left off — finish fixing the current test, run the full test suite
to verify no regressions, and commit.`;
  }

  return `Read FIXING_HARNESS_TESTS.md for the full methodology, then fix ignored harness tests.

Test architecture: Tests are auto-generated at compile time by the \`ngspice_tests!()\` proc macro
(in thevenin-test-macro/). It discovers all .cir/.out pairs from ngspice-upstream/tests/ and embeds
them as string literals. Ignore reasons live in thevenin/tests/ignore.toml (TOML: "path/to/file.cir" = "reason").

## Prioritization strategy

Pick tests in this order (highest ROI first):
1. **Tests that already pass** — just un-ignore them
2. **Vacuous passes** — the formatter produces no output; fix the .print resolution or
   output formatting bug. These are often missing expression evaluation (e.g. \`v(g)/10\`)
   or device parameter queries (\`@m1[Vbs]\`).
3. **Near-miss numerical errors (<1%)** — usually a single wrong coefficient, sign, or
   parameter in the device model. Compare term-by-term against ngspice C source.
4. **Numerical errors (1-5%)** — same as above but deeper bugs.
5. **Convergence failures** — hardest. May need solver changes or device initialization fixes.

## Debugging device model bugs efficiently

When you find a numerical mismatch, don't just stare at the full circuit. Instead:
1. Find which device model is involved (BSIM3SOI, VBIC, BJT, etc.)
2. In the ngspice C source (\`ngspice-upstream/src/spicelib/devices/<model>/\`), find the
   corresponding \`*load.c\` file (e.g. \`b3soipdld.c\` for BSIM3SOI-PD)
3. Compare the Rust companion/eval function against the C source **term by term**
4. Focus on the specific output variable that's wrong (e.g. Ids, gm, Vth)
5. Add temporary debug prints comparing intermediate values if needed
6. Common bug patterns:
   - Wrong sign (\`-\` vs \`+\`)
   - Wrong exponent (e.g. \`exp(1.0)\` vs \`exp(vt)\`)
   - Missing terms (accidentally dropped a line during C→Rust port)
   - Wrong variable name (off-by-one in similar parameters)
   - Integer vs float division
   - Missing \`abs()\` or \`max()\` clamping

## Steps
1. Read thevenin/tests/ignore.toml to see all ignored tests and their reasons
2. Pick a test or group of related tests (follow prioritization above)
3. **Before starting work**, read the relevant history file in \`scripts/fix-tests-history/\`
   for the device model or subsystem you're investigating. This tells you what was already
   tried and ruled out — do NOT repeat failed approaches.
4. Diagnose the root cause using the methodology in FIXING_HARNESS_TESTS.md
5. Fix the issue in the Rust source code
6. Run the fixed tests to verify they pass
7. Run the FULL test suite to check for regressions: nix develop --command cargo nextest run --workspace 2>&1 | grep -E "FAIL|Summary"
8. Run clippy: nix develop --command cargo clippy --workspace -- -D warnings
9. To un-ignore a fixed test, remove its line from thevenin/tests/ignore.toml
10. Commit all changes with a descriptive message and push to the current branch

## Recording findings
After investigating a test (whether you fixed it or not), append your findings to
the relevant file in \`scripts/fix-tests-history/\`. Use a \`## Session N findings (date)\`
heading. If no file exists for the device model, create one. Keep entries concise:
what was tried, what was found, whether it's worth retrying.

Files: \`applied-fixes.md\`, \`failed-investigations.md\`, \`vbic.md\`, \`bsim3soi.md\`,
\`transmission-line.md\`, \`general-circuits.md\`, \`missing-features.md\`.

## Important rules
- Always run commands through \`nix develop --command ...\`
- Do NOT attempt tests in these intractable categories: ${SKIP_CATEGORIES.join(", ")}
- Do NOT attempt tests that would require implementing entire missing subsystems
- If a test group requires an architectural change, document why and move on — do NOT commit
- When running the full test suite, use \`--workspace\` to catch regressions across all crates
- Do NOT bloat FIXING_HARNESS_TESTS.md — write findings to \`scripts/fix-tests-history/\` instead
${triageSection}${dedupSection}`;
}

// ---------------------------------------------------------------------------
// Run one iteration
// ---------------------------------------------------------------------------

type IterationResult =
  | { status: "success"; summary: string }
  | { status: "no_progress"; summary: string }
  | { status: "rate_limited" }
  | { status: "max_turns_with_progress"; sessionId: string; summary: string }
  | { status: "error"; message: string };

async function runIteration(
  iterationNum: number,
  resumeSessionId?: string,
): Promise<IterationResult> {
  inIteration = true;
  try {
  log(`\n${"=".repeat(60)}`);
  log(`--- Iteration ${iterationNum} ---`);

  const ignoredBefore = parseIgnoreToml();
  const commitBefore = latestCommit();
  log(`Ignored tests before: ${ignoredBefore.size}${resumeSessionId ? " (resuming session)" : ""}`);

  // Pull latest before starting
  if (!gitPull()) {
    return { status: "error", message: "git pull failed" };
  }

  try {
    let resultText = "";
    let hitRateLimit = false;
    let sessionId = resumeSessionId ?? "";
    let resultSubtype = "";

    const queryOptions: any = {
      cwd: PROJECT_DIR,
      allowedTools: ["Read", "Write", "Edit", "Bash", "Glob", "Grep", "Agent"],
      permissionMode: "bypassPermissions",
      allowDangerouslySkipPermissions: true,
      maxTurns: 200,
      settingSources: ["project"],
      effort: "high",
      includePartialMessages: true,
    };

    // Resume an existing session if provided
    if (resumeSessionId) {
      queryOptions.sessionId = resumeSessionId;
    }

    for await (const message of query({
      prompt: buildPrompt(resumeSessionId),
      options: queryOptions,
    })) {
      const msg = message as any;

      if (msg.type === "result") {
        resultText = msg.result ?? "";
        resultSubtype = msg.subtype ?? "";
        sessionId = msg.session_id ?? sessionId;
        const duration = msg.duration_ms;
        const turns = msg.num_turns;
        log(`Agent finished (${resultSubtype}).${turns ? ` Turns: ${turns}` : ""}${duration ? ` Duration: ${(duration / 1000).toFixed(0)}s` : ""}`);

        // Log a summary of what the agent said
        if (resultText) {
          const summaryLines = resultText.split("\n").slice(0, 10).join("\n");
          log(`Summary:\n${summaryLines}`);
        }

        // Break out of the async iterator — it may not signal done on its own
        break;
      } else if (msg.type === "system" && msg.subtype === "init") {
        sessionId = msg.session_id ?? "";
        log(`Session: ${sessionId}`);
      } else if (msg.type === "stream_event") {
        const event = msg.event;
        if (event?.type === "content_block_start") {
          if (event.content_block?.type === "tool_use") {
            log(`  -> ${event.content_block.name}`);
          }
        } else if (event?.type === "content_block_delta") {
          if (event.delta?.type === "text_delta") {
            process.stdout.write(event.delta.text);
          }
        }
      } else if (msg.type === "tool_progress") {
        if (msg.elapsed_time_seconds && Math.floor(msg.elapsed_time_seconds) % 10 === 0) {
          log(`  .. ${msg.tool_name} running (${msg.elapsed_time_seconds}s)`);
        }
      } else if (msg.type === "rate_limit_event") {
        const info = msg.rate_limit_info;
        if (info?.status === "rejected") {
          hitRateLimit = true;
          const resetsAt = info.resetsAt
            ? new Date(info.resetsAt).toLocaleTimeString("en-GB", { hour12: false })
            : "unknown";
          log(`Rate limited! Resets at: ${resetsAt}`);
        } else if (info?.status === "allowed_warning") {
          log(`Rate limit warning: ${((info.utilization ?? 0) * 100).toFixed(0)}% utilized`);
        }
      }
    }

    if (hitRateLimit) return { status: "rate_limited" };

    // Check if progress was made
    const commitAfter = latestCommit();
    const ignoredAfter = parseIgnoreToml();
    const madeCommit = commitAfter !== commitBefore;
    const fixedTests = ignoredBefore.size - ignoredAfter.size;

    log(`Ignored tests after: ${ignoredAfter.size} (${fixedTests >= 0 ? "-" : "+"}${Math.abs(fixedTests)})`);
    log(`New commit: ${madeCommit ? "yes" : "no"}`);

    const shortSummary = resultText.split("\n")[0]?.slice(0, 200) ?? "no summary";

    // Handle max_turns with progress — eligible for session resumption
    if (resultSubtype === "error_max_turns" && madeCommit) {
      log("Hit max turns but made progress — will resume this session.");
      recordAttempts(ignoredBefore, ignoredAfter, "failed", `max_turns (progress): ${shortSummary}`);
      return { status: "max_turns_with_progress", sessionId, summary: shortSummary };
    }

    if (resultSubtype === "error_max_turns") {
      log("Hit max turns — agent ran out of steps.");
    } else if (resultSubtype === "error_during_execution") {
      log("Error during agent execution.");
    }

    if (madeCommit) {
      // Verify the commit didn't break other tests
      if (!regressionCheck()) {
        logError("Agent's commit introduced regressions — reverting.");
        revertTo(commitBefore);
        recordAttempts(ignoredBefore, ignoredAfter, "reverted", shortSummary);
        return { status: "no_progress", summary: `REVERTED (regression): ${shortSummary}` };
      }
      recordAttempts(ignoredBefore, ignoredAfter, "fixed", shortSummary);
      return { status: "success", summary: shortSummary };
    } else {
      recordAttempts(ignoredBefore, ignoredAfter, "failed", shortSummary);
      return { status: "no_progress", summary: shortSummary };
    }
  } catch (err: unknown) {
    const errMsg = err instanceof Error ? err.message : String(err);

    if (/rate.?limit|429|too many requests|overloaded|529/i.test(errMsg)) {
      log(`Rate limited: ${errMsg}`);
      return { status: "rate_limited" };
    }

    logError(errMsg);

    if (/5\d\d|ECONNRESET|ETIMEDOUT|fetch failed/i.test(errMsg)) {
      log("Transient error, will retry after cooldown.");
      return { status: "rate_limited" };
    }

    return { status: "error", message: errMsg };
  }
  } finally {
    inIteration = false;
  }
}

// ---------------------------------------------------------------------------
// Peak-hours pause — Anthropic Max subscriptions get reduced capacity during
// US work hours: 05:00–11:00 PT (Pacific Time).
// ---------------------------------------------------------------------------

function getPacificHour(): number {
  return parseInt(
    new Intl.DateTimeFormat("en-US", {
      timeZone: "America/Los_Angeles",
      hour: "numeric",
      hour12: false,
    }).format(new Date()),
    10,
  );
}

function getPacificMinute(): number {
  return parseInt(
    new Intl.DateTimeFormat("en-US", {
      timeZone: "America/Los_Angeles",
      minute: "numeric",
    }).format(new Date()),
    10,
  );
}

function isPeakHours(): boolean {
  const hour = getPacificHour();
  return hour >= 5 && hour < 11; // 05:00–11:00 PT
}

function msUntilPeakEnds(): number {
  const hour = getPacificHour();
  const minute = getPacificMinute();
  // Minutes remaining: from current HH:MM to 11:00
  const minutesLeft = (11 - hour) * 60 - minute;
  return Math.max(0, minutesLeft * 60_000);
}

async function waitOutPeakHours(): Promise<void> {
  if (!isPeakHours()) return;

  const ms = msUntilPeakEnds();
  const minutes = Math.ceil(ms / 60_000);
  log(`Peak hours (05:00–11:00 PT) — pausing for ~${minutes} minutes until peak ends...`);

  // Sleep in 60s chunks so we can still respond to stop requests
  const end = Date.now() + ms;
  while (Date.now() < end) {
    if (checkStopRequested()) return;
    const remaining = Math.ceil((end - Date.now()) / 60_000);
    process.stdout.write(`\r[${timestamp()}] Peak hours — resuming in ~${remaining} min   `);
    await new Promise((r) => setTimeout(r, Math.min(60_000, end - Date.now())));
  }
  process.stdout.write("\r" + " ".repeat(70) + "\r");
  log("Peak hours ended — resuming.");
}

// ---------------------------------------------------------------------------
// Main loop
// ---------------------------------------------------------------------------

async function main() {
  // Clean up stale stop file from a previous run
  try { execSync(`rm -f "${STOP_FILE}"`); } catch {}

  log(`Starting overnight test fixer (PID: ${process.pid})`);
  log(`Project: ${PROJECT_DIR}`);
  log(`Max iterations: ${MAX_ITERATIONS}`);
  log(`Cooldown: ${COOLDOWN_SEC}s`);
  log(`Stop after ${MAX_NO_PROGRESS} consecutive no-progress iterations`);
  log(`Triage refresh every ${TRIAGE_REFRESH_EVERY} successful iterations`);
  log(`Log file: ${LOG_FILE}`);
  log(`Initial ignored tests: ${countIgnoredTests()}`);

  // Early exit: check if all remaining tests are intractable
  const tractable = getTractableTests();
  log(`Tractable tests: ${tractable.length}`);
  if (tractable.length === 0) {
    log("All remaining ignored tests match skip categories — nothing to do.");
    log("Remaining tests require architectural changes or missing subsystems.");
    notify("fix-tests: nothing to do", "All remaining tests are intractable.");
    return;
  }
  log(`Tractable: ${tractable.map((t) => t.path).join(", ")}`);
  log("=".repeat(60));

  let consecutiveRateLimits = 0;
  let consecutiveNoProgress = 0;
  let successCount = 0;
  let iterationCount = 0;
  let resumeSessionId: string | undefined;

  while (successCount < MAX_ITERATIONS) {
    if (checkStopRequested()) {
      log("Graceful stop — not starting new iteration.");
      break;
    }

    // Pause during Anthropic peak hours (05:00–11:00 PT) when Max gets reduced capacity
    await waitOutPeakHours();
    if (checkStopRequested()) break;

    iterationCount++;
    const result = await runIteration(iterationCount, resumeSessionId);
    resumeSessionId = undefined; // Clear after use

    // Check again after iteration completes
    if (checkStopRequested()) {
      log("Graceful stop — iteration completed, exiting loop.");
      break;
    }

    switch (result.status) {
      case "rate_limited": {
        consecutiveRateLimits++;
        const backoff = Math.min(60 * Math.pow(2, consecutiveRateLimits - 1), 600);
        log(`Rate limited (${consecutiveRateLimits}x consecutive). Backing off ${backoff}s...`);
        await sleep(backoff);
        // Don't increment iterationCount on retry — handled by while loop
        continue;
      }

      case "success":
        consecutiveRateLimits = 0;
        consecutiveNoProgress = 0;
        successCount++;
        log(`Iteration ${iterationCount} succeeded (${successCount} total successes).`);
        notify("fix-tests: test fixed", result.summary.slice(0, 100));

        // Periodically refresh the triage report so priorities stay current
        if (successCount % TRIAGE_REFRESH_EVERY === 0) {
          refreshTriageReport();
        }

        // Re-check tractability after a success — new fixes may have changed the landscape
        {
          const remaining = getTractableTests();
          if (remaining.length === 0) {
            log("All remaining tests are now intractable — stopping.");
            break;
          }
        }
        break;

      case "max_turns_with_progress":
        consecutiveRateLimits = 0;
        // Don't count as no_progress — the agent was making headway
        log(`Iteration ${iterationCount} hit max turns but made progress — resuming session.`);
        resumeSessionId = result.sessionId;
        break;

      case "no_progress":
        consecutiveRateLimits = 0;
        consecutiveNoProgress++;
        log(`Iteration ${iterationCount} made no progress (${consecutiveNoProgress}/${MAX_NO_PROGRESS} before stopping).`);

        if (consecutiveNoProgress >= MAX_NO_PROGRESS) {
          log(`\nStopping: ${MAX_NO_PROGRESS} consecutive iterations with no commits.`);
          log(`Remaining tests are likely intractable or require architectural changes.`);
        }
        break;

      case "error":
        consecutiveRateLimits = 0;
        consecutiveNoProgress++;
        logError(`Iteration ${iterationCount} failed: ${result.message}`);
        break;
    }

    if (consecutiveNoProgress >= MAX_NO_PROGRESS) break;

    // Check tractability after no_progress too — avoid re-checking the same intractable tests
    const remaining = getTractableTests();
    if (remaining.length === 0) {
      log("All remaining tests are intractable — stopping.");
      break;
    }

    log(`Cooling down before next iteration...`);
    await sleep(COOLDOWN_SEC);
  }

  // Final summary
  const finalIgnored = countIgnoredTests();
  log(`\n${"=".repeat(60)}`);
  log(`FINAL SUMMARY`);
  log(`  Total iterations: ${iterationCount}`);
  log(`  Successful iterations: ${successCount}`);
  log(`  Remaining ignored tests: ${finalIgnored}`);
  log(`  Tractable remaining: ${getTractableTests().length}`);
  log(`${"=".repeat(60)}`);

  notify(
    "fix-tests: done",
    `${successCount} fixes in ${iterationCount} iterations. ${finalIgnored} tests remaining.`,
  );
}

main().catch((err) => {
  logError(`Fatal: ${err}`);
  notify("fix-tests: CRASHED", String(err).slice(0, 100));
  process.exit(1);
});
