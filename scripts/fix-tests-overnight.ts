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
 *   - Deduplication: logs attempted tests so agent can skip previously-failed ones
 *   - Git pull before each iteration to avoid push conflicts
 *   - Handles ResultMessage.subtype properly (success, error_max_turns, etc.)
 *   - Stops early after N consecutive no-progress iterations
 *   - Logs iteration summaries for post-mortem review
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
 *
 * Graceful shutdown (finishes current iteration, then exits):
 *   touch scripts/.stop-fix-tests     # stop file — checked between iterations
 *   kill -USR1 $(pgrep -f fix-tests)  # signal — works mid-iteration too
 */

import { query } from "@anthropic-ai/claude-agent-sdk";
import { readFileSync, appendFileSync, existsSync } from "fs";
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
const PROJECT_DIR = resolve(import.meta.dirname ?? __dirname, "..");
const LOG_FILE = resolve(PROJECT_DIR, "scripts", "fix-tests.log");
const ATTEMPTS_FILE = resolve(PROJECT_DIR, "scripts", "attempted-tests.log");

const STOP_FILE = resolve(PROJECT_DIR, "scripts", ".stop-fix-tests");

// Test categories that are intractable — agent should not waste time on these
const SKIP_CATEGORIES = [
  ".control scripting",
  "XSPICE (US-056)",
  "BSIM1/BSIM2 (US-052/053)",
  "TEMPER keyword",
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
  try { appendFileSync(LOG_FILE, line + "\n"); } catch {}
}

function logError(msg: string) {
  const line = `[${timestamp()}] ERROR: ${msg}`;
  console.error(line);
  try { appendFileSync(LOG_FILE, line + "\n"); } catch {}
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
    return execSync(cmd, { cwd: PROJECT_DIR, encoding: "utf-8", timeout: 30_000 }).trim();
  } catch {
    return "";
  }
}

/** Count ignored tests in ignore.toml (each non-comment, non-empty line with `=` is one test) */
function countIgnoredTests(): number {
  const content = shell(`grep -c '=' thevenin/tests/ignore.toml`);
  return parseInt(content, 10) || 0;
}

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

/** Load previously attempted tests (for deduplication) */
function loadAttemptedTests(): string[] {
  if (!existsSync(ATTEMPTS_FILE)) return [];
  return readFileSync(ATTEMPTS_FILE, "utf-8")
    .split("\n")
    .filter((l) => l.trim().length > 0);
}

function logAttemptedTest(testInfo: string) {
  try { appendFileSync(ATTEMPTS_FILE, `${timestamp()} | ${testInfo}\n`); } catch {}
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

function buildPrompt(): string {
  const attempted = loadAttemptedTests();
  const attemptedSection = attempted.length > 0
    ? `\n\nTests previously attempted (may have failed — avoid re-attempting unless you have a new approach):\n${attempted.map((l) => `- ${l}`).join("\n")}`
    : "";

  return `Read FIXING_HARNESS_TESTS.md for the full methodology, then fix ignored harness tests.

Test architecture: Tests are auto-generated at compile time by the \`ngspice_tests!()\` proc macro
(in thevenin-test-macro/). It discovers all .cir/.out pairs from ngspice-upstream/tests/ and embeds
them as string literals. Ignore reasons live in thevenin/tests/ignore.toml (TOML: "path/to/file.cir" = "reason").

Steps:
1. Read thevenin/tests/ignore.toml to see all ignored tests and their reasons
2. Pick a group of related ignored tests (prefer numerical accuracy bugs)
3. Diagnose the root cause using the methodology in FIXING_HARNESS_TESTS.md
4. Fix the issue in the Rust source code
5. Run the fixed tests to verify they pass
6. Run the FULL test suite to check for regressions: nix develop --command cargo nextest run --package thevenin 2>&1 | grep -E "FAIL|test result"
7. Run clippy: nix develop --command cargo clippy --workspace -- -D warnings
8. To un-ignore a fixed test, remove its line from thevenin/tests/ignore.toml
9. Commit all changes with a descriptive message and push to the current branch

Important rules:
- Always run commands through \`nix develop --command ...\`
- Do NOT attempt tests in these intractable categories: ${SKIP_CATEGORIES.join(", ")}
- Do NOT attempt tests that would require implementing entire missing subsystems
- If a test group requires an architectural change (e.g., full voltage-dependent charge integration), document why and move on
- If after investigation you determine the test cannot be fixed without major changes, say so clearly and do NOT commit
- Update FIXING_HARNESS_TESTS.md with any new findings (triage updates, applied fixes, investigations)
${attemptedSection}`;
}

// ---------------------------------------------------------------------------
// Run one iteration
// ---------------------------------------------------------------------------

type IterationResult =
  | { status: "success"; summary: string }
  | { status: "no_progress"; summary: string }
  | { status: "rate_limited" }
  | { status: "error"; message: string };

async function runIteration(iteration: number): Promise<IterationResult> {
  inIteration = true;
  log(`\n${"=".repeat(60)}`);
  log(`--- Iteration ${iteration}/${MAX_ITERATIONS} ---`);

  const ignoredBefore = countIgnoredTests();
  const commitBefore = latestCommit();
  log(`Ignored tests before: ${ignoredBefore}`);

  // Pull latest before starting
  if (!gitPull()) {
    return { status: "error", message: "git pull failed" };
  }

  try {
    let resultText = "";
    let hitRateLimit = false;
    let sessionId = "";
    let resultSubtype = "";

    for await (const message of query({
      prompt: buildPrompt(),
      options: {
        cwd: PROJECT_DIR,
        allowedTools: ["Read", "Write", "Edit", "Bash", "Glob", "Grep", "Agent"],
        permissionMode: "bypassPermissions",
        allowDangerouslySkipPermissions: true,
        maxTurns: 200,
        settingSources: ["project"],
        effort: "high",
        includePartialMessages: true,
      },
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
        if (msg.elapsed_time_seconds && msg.elapsed_time_seconds % 10 === 0) {
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

    // Handle non-success results
    if (resultSubtype === "error_max_turns") {
      log("Hit max turns — agent ran out of steps.");
    } else if (resultSubtype === "error_during_execution") {
      log("Error during agent execution.");
    }

    // Check if progress was made
    const commitAfter = latestCommit();
    const ignoredAfter = countIgnoredTests();
    const madeCommit = commitAfter !== commitBefore;
    const fixedTests = ignoredBefore - ignoredAfter;

    log(`Ignored tests after: ${ignoredAfter} (${fixedTests >= 0 ? "-" : "+"}${Math.abs(fixedTests)})`);
    log(`New commit: ${madeCommit ? "yes" : "no"}`);

    // Log what was attempted for deduplication
    const shortSummary = resultText.split("\n")[0]?.slice(0, 200) ?? "no summary";
    logAttemptedTest(shortSummary);

    if (madeCommit) {
      return { status: "success", summary: shortSummary };
    } else {
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
  log(`Log file: ${LOG_FILE}`);
  log(`Initial ignored tests: ${countIgnoredTests()}`);
  console.log("=".repeat(60));

  let consecutiveRateLimits = 0;
  let consecutiveNoProgress = 0;
  let successCount = 0;

  for (let i = 1; i <= MAX_ITERATIONS; i++) {
    if (checkStopRequested()) {
      log("Graceful stop — not starting new iteration.");
      break;
    }

    const result = await runIteration(i);
    inIteration = false;

    // Check again after iteration completes
    if (checkStopRequested()) {
      log("Graceful stop — iteration completed, exiting loop.");
      break;
    }

    switch (result.status) {
      case "rate_limited":
        consecutiveRateLimits++;
        const backoff = Math.min(60 * Math.pow(2, consecutiveRateLimits - 1), 600);
        log(`Rate limited (${consecutiveRateLimits}x consecutive). Backing off ${backoff}s...`);
        await sleep(backoff);
        i--; // Don't count rate-limited iterations
        continue;

      case "success":
        consecutiveRateLimits = 0;
        consecutiveNoProgress = 0;
        successCount++;
        log(`✓ Iteration ${i} succeeded (${successCount} total).`);
        break;

      case "no_progress":
        consecutiveRateLimits = 0;
        consecutiveNoProgress++;
        log(`✗ Iteration ${i} made no progress (${consecutiveNoProgress}/${MAX_NO_PROGRESS} before stopping).`);

        if (consecutiveNoProgress >= MAX_NO_PROGRESS) {
          log(`\nStopping: ${MAX_NO_PROGRESS} consecutive iterations with no commits.`);
          log(`Remaining tests are likely intractable or require architectural changes.`);
          break;
        }
        break;

      case "error":
        consecutiveRateLimits = 0;
        consecutiveNoProgress++;
        logError(`Iteration ${i} failed: ${result.message}`);
        break;
    }

    if (consecutiveNoProgress >= MAX_NO_PROGRESS) break;

    if (i < MAX_ITERATIONS) {
      log(`Cooling down before next iteration...`);
      await sleep(COOLDOWN_SEC);
    }
  }

  // Final summary
  log(`\n${"=".repeat(60)}`);
  log(`FINAL SUMMARY`);
  log(`  Successful iterations: ${successCount}`);
  log(`  Remaining ignored tests: ${countIgnoredTests()}`);
  log(`${"=".repeat(60)}`);
}

main().catch((err) => {
  logError(`Fatal: ${err}`);
  process.exit(1);
});
