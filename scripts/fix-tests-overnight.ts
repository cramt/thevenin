#!/usr/bin/env npx tsx
/**
 * Overnight test fixer — uses Claude Agent SDK to repeatedly pick ignored
 * harness tests, fix them, commit & push. Handles rate limits gracefully
 * by waiting and retrying.
 *
 * Usage:
 *   npx tsx scripts/fix-tests-overnight.ts [--max-iterations 50] [--cooldown 30]
 *
 * Options:
 *   --max-iterations N   Max agent sessions to run (default: 50)
 *   --cooldown N         Seconds to wait between iterations (default: 30)
 */

import { query } from "@anthropic-ai/claude-agent-sdk";
import { readFileSync } from "fs";
import { resolve } from "path";

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

const args = process.argv.slice(2);
function flag(name: string, fallback: number): number {
  const idx = args.indexOf(`--${name}`);
  if (idx !== -1 && args[idx + 1]) return parseInt(args[idx + 1], 10);
  return fallback;
}

const MAX_ITERATIONS = flag("max-iterations", 50);
const COOLDOWN_SEC = flag("cooldown", 30);
const PROJECT_DIR = resolve(import.meta.dirname ?? __dirname, "..");

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function timestamp(): string {
  return new Date().toLocaleTimeString("en-GB", { hour12: false });
}

function log(msg: string) {
  console.log(`[${timestamp()}] ${msg}`);
}

function logError(msg: string) {
  console.error(`[${timestamp()}] ERROR: ${msg}`);
}

async function sleep(seconds: number): Promise<void> {
  for (let remaining = seconds; remaining > 0; remaining--) {
    process.stdout.write(`\r[${timestamp()}] Waiting ${remaining}s...   `);
    await new Promise((r) => setTimeout(r, 1000));
  }
  process.stdout.write("\r" + " ".repeat(60) + "\r");
}

// ---------------------------------------------------------------------------
// Load the prompt
// ---------------------------------------------------------------------------

const fixingHarnessTests = readFileSync(
  resolve(PROJECT_DIR, "FIXING_HARNESS_TESTS.md"),
  "utf-8",
);

const PROMPT = `@FIXING_HARNESS_TESTS.md pick another set of ignored tests and work on them, when done just go ahead and commit and push it.

Here is the content of FIXING_HARNESS_TESTS.md for reference:

<fixing-harness-tests>
${fixingHarnessTests}
</fixing-harness-tests>

Instructions:
1. Look at thevenin/tests/harness.rs and find ignored tests
2. Pick a group of related ignored tests (prefer numerical accuracy bugs)
3. Diagnose the root cause using the methodology in FIXING_HARNESS_TESTS.md
4. Fix the issue in the Rust source code
5. Run the tests to verify they pass
6. Commit all changes with a descriptive message
7. Push to the current branch

Always run commands through \`nix develop --command ...\`.
`;

// ---------------------------------------------------------------------------
// Main loop
// ---------------------------------------------------------------------------

async function runIteration(iteration: number): Promise<"continue" | "rate_limited"> {
  log(`--- Iteration ${iteration}/${MAX_ITERATIONS} ---`);

  try {
    let resultText = "";
    let hitRateLimit = false;

    for await (const message of query({
      prompt: PROMPT,
      options: {
        cwd: PROJECT_DIR,
        allowedTools: ["Read", "Write", "Edit", "Bash", "Glob", "Grep", "Agent"],
        permissionMode: "bypassPermissions",
        allowDangerouslySkipPermissions: true,
        maxTurns: 200,
        includePartialMessages: true,
      },
    })) {
      const msg = message as any;

      if ("result" in msg) {
        // Final result
        resultText = msg.result ?? "";
        const cost = msg.total_cost_usd;
        const duration = msg.duration_ms;
        log(`Agent finished.${cost ? ` Cost: $${cost.toFixed(4)}` : ""}${duration ? ` Duration: ${(duration / 1000).toFixed(0)}s` : ""}`);
      } else if (msg.type === "system" && msg.subtype === "init") {
        log(`Session: ${msg.session_id}`);
      } else if (msg.type === "stream_event") {
        // Real-time streaming of assistant output
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
        // Periodic tool progress (shows it's alive)
        if (msg.elapsed_time_seconds && msg.elapsed_time_seconds % 10 === 0) {
          log(`  .. ${msg.tool_name} running (${msg.elapsed_time_seconds}s)`);
        }
      } else if (msg.type === "rate_limit_event") {
        const info = msg.rate_limit_info;
        if (info?.status === "rejected") {
          hitRateLimit = true;
          const resetsAt = info.resetsAt ? new Date(info.resetsAt).toLocaleTimeString("en-GB", { hour12: false }) : "unknown";
          log(`Rate limited! Resets at: ${resetsAt}`);
        } else if (info?.status === "allowed_warning") {
          log(`Rate limit warning: ${((info.utilization ?? 0) * 100).toFixed(0)}% utilized`);
        }
      }
    }

    if (hitRateLimit) return "rate_limited";
    return "continue";
  } catch (err: unknown) {
    const errMsg = err instanceof Error ? err.message : String(err);

    if (/rate.?limit|429|too many requests|overloaded|529/i.test(errMsg)) {
      log(`Rate limited: ${errMsg}`);
      return "rate_limited";
    }

    logError(errMsg);

    if (/5\d\d|ECONNRESET|ETIMEDOUT|fetch failed/i.test(errMsg)) {
      log("Transient error, will retry after cooldown.");
      return "rate_limited";
    }

    logError("Unexpected error, continuing after cooldown.");
    return "rate_limited";
  }
}

async function main() {
  log(`Starting overnight test fixer`);
  log(`Project: ${PROJECT_DIR}`);
  log(`Max iterations: ${MAX_ITERATIONS}`);
  log(`Cooldown: ${COOLDOWN_SEC}s`);
  console.log("=".repeat(60));

  let consecutiveRateLimits = 0;

  for (let i = 1; i <= MAX_ITERATIONS; i++) {
    const result = await runIteration(i);

    if (result === "rate_limited") {
      consecutiveRateLimits++;
      // Exponential backoff: 60s, 120s, 240s, 480s, max 600s
      const backoff = Math.min(60 * Math.pow(2, consecutiveRateLimits - 1), 600);
      log(`Rate limited (${consecutiveRateLimits}x consecutive). Backing off ${backoff}s...`);
      await sleep(backoff);
      i--; // Don't count rate-limited iterations
      continue;
    }

    consecutiveRateLimits = 0;

    if (i < MAX_ITERATIONS) {
      log(`Cooling down before next iteration...`);
      await sleep(COOLDOWN_SEC);
    }
  }

  log(`Completed ${MAX_ITERATIONS} iterations. Done.`);
}

main().catch((err) => {
  logError(`Fatal: ${err}`);
  process.exit(1);
});
