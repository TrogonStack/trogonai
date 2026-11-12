import fs from "node:fs";

import { appendSummary, getInput, info, setFailed, setOutput, warning } from "./core.mjs";

const PER_PAGE = 100;

// A runaway Link chain must not hold a job open; 50 pages is 5000 runners.
const MAX_PAGES = 50;

const SCOPES = ["repository", "organization"];

// Events whose code was reviewed and merged, or started by someone with write
// access. Anything not listed is untrusted, so a new event type fails closed.
const TRUSTED_EVENTS = new Set(["push", "workflow_dispatch", "schedule", "merge_group"]);

// Thrown for anything a caller can fix by editing their workflow, so main can
// report it as a GitHub error annotation rather than a stack trace.
export class InputError extends Error {}

export function parseLabels(text) {
  return (text ?? "")
    .split(/[,\n]/)
    .map((label) => label.trim())
    .filter((label) => label.length > 0);
}

// A self-hosted runner keeps its state between jobs and usually sits on a
// private network, so only code a maintainer has accepted may reach it. A fork
// pull request runs the fork author's workflow edits, which is the case this
// exists to keep out.
export function decideTrust({ eventName, event, repository }) {
  if (!event) {
    return { trusted: false, reason: "untrusted-event" };
  }

  if (eventName === "pull_request") {
    const head = event.pull_request?.head?.repo?.full_name;
    const same = typeof head === "string" && head.toLowerCase() === String(repository).toLowerCase();
    return same ? { trusted: true } : { trusted: false, reason: "fork-pull-request" };
  }

  return TRUSTED_EVENTS.has(eventName)
    ? { trusted: true }
    : { trusted: false, reason: "untrusted-event" };
}

function isIdle(runner, wanted) {
  if (runner?.status !== "online" || runner.busy !== false || !Array.isArray(runner.labels)) {
    return false;
  }
  const have = new Set(
    runner.labels.map((label) => String(label?.name).toLowerCase()),
  );
  return wanted.every((label) => have.has(label.toLowerCase()));
}

export function countIdle(runners, labels) {
  return runners.filter((runner) => isIdle(runner, labels)).length;
}

function parseSlots(text) {
  const value = (text ?? "").trim() || "1";
  if (!/^\d+$/.test(value) || Number(value) < 1) {
    throw new InputError("The `slots` input must be a whole number of at least 1.");
  }
  return Number(value);
}

function readEvent(path) {
  try {
    const event = JSON.parse(fs.readFileSync(path, "utf8"));
    return typeof event === "object" ? event : null;
  } catch {
    return null;
  }
}

function runnersUrl({ scope, repository, apiUrl, page }) {
  const [owner] = repository.split("/");
  const base = scope === "organization" ? `orgs/${owner}` : `repos/${repository}`;
  return `${apiUrl}/${base}/actions/runners?per_page=${PER_PAGE}&page=${page}`;
}

// Stops once every slot is covered: the rest of a long list cannot change a
// decision already reached.
async function countIdleUpTo({ fetch, token, url, labels, limit }) {
  let idle = 0;

  for (let page = 1; page <= MAX_PAGES; page += 1) {
    const response = await fetch(url(page), {
      headers: {
        Accept: "application/vnd.github+json",
        Authorization: `Bearer ${token}`,
        "X-GitHub-Api-Version": "2022-11-28",
      },
    });

    if (!response.ok) {
      throw new Error(`the runners API answered HTTP ${response.status}`);
    }

    const body = await response.json();
    if (!Array.isArray(body?.runners)) {
      throw new Error("the runners API answered without a runners list");
    }

    idle += countIdle(body.runners, labels);
    if (idle >= limit) {
      return limit;
    }

    // A full page is the only sign of more; Link is the authority when sent.
    const link = response.headers?.get?.("link");
    const more = link == null ? true : /rel="next"/.test(link);
    if (body.runners.length < PER_PAGE || !more) {
      return idle;
    }
  }

  return idle;
}

// Decides without touching the process, so the tests drive the code that ships
// rather than a copy of it. Only caller mistakes throw: a runner that cannot be
// asked about is a reason to use the hosted fallback, never to fail the run.
export async function select({
  githubToken,
  primaryLabels,
  fallbackLabels,
  scope,
  slots,
  env = process.env,
  fetch = globalThis.fetch,
}) {
  const chosenScope = (scope ?? "").trim() || "repository";
  if (!SCOPES.includes(chosenScope)) {
    throw new InputError(`The \`scope\` input must be one of: ${SCOPES.join(", ")}.`);
  }

  const wanted = parseSlots(slots);

  const fallback = parseLabels(fallbackLabels);
  if (fallback.length === 0) {
    throw new InputError("The `fallback-labels` input must name at least one runner label.");
  }

  const primary = parseLabels(primaryLabels);
  const warnings = [];

  // The first `taken` slots get the primary labels, so a caller orders its
  // slots by how long each job runs and the longest ones land on the runner.
  const result = (taken, reason, message) => {
    const runners = Array.from({ length: wanted }, (_, slot) => (slot < taken ? primary : fallback));
    return {
      runners,
      runsOn: runners[0],
      selfHosted: taken > 0,
      selfHostedSlots: taken,
      reason,
      warnings,
      message,
    };
  };
  const fell = (reason, message) => result(0, reason, message);

  const trust = decideTrust({
    eventName: env.GITHUB_EVENT_NAME,
    event: env.GITHUB_EVENT_PATH ? readEvent(env.GITHUB_EVENT_PATH) : null,
    repository: env.GITHUB_REPOSITORY ?? "",
  });
  if (!trust.trusted) {
    return fell(trust.reason, `Event '${env.GITHUB_EVENT_NAME}' is not trusted with the self-hosted runner.`);
  }

  if (primary.length === 0) {
    return fell("not-configured", "No primary labels are configured.");
  }

  if (!githubToken) {
    return fell("no-token", "No token was given to list self-hosted runners.");
  }

  const apiUrl = (env.GITHUB_API_URL || "https://api.github.com").replace(/\/+$/, "");

  try {
    const idle = await countIdleUpTo({
      fetch,
      token: githubToken,
      labels: primary,
      limit: wanted,
      url: (page) =>
        runnersUrl({ scope: chosenScope, repository: env.GITHUB_REPOSITORY, apiUrl, page }),
    });

    if (idle > 0) {
      return result(idle, "idle-runner", `${idle} of ${wanted} slot(s) go to idle runners carrying ${primary.join(", ")}.`);
    }
    return fell("no-idle-runner", `No idle runner carries ${primary.join(", ")}.`);
  } catch (thrown) {
    warnings.push(`Could not list self-hosted runners, using the fallback: ${thrown.message}`);
    return fell("api-error", thrown.message);
  }
}

export async function main() {
  try {
    const result = await select({
      githubToken: getInput("github-token"),
      primaryLabels: getInput("primary-labels"),
      fallbackLabels: getInput("fallback-labels"),
      scope: getInput("scope"),
      slots: getInput("slots"),
    });

    for (const line of result.warnings) {
      warning(line);
    }

    const target = result.selfHosted ? "self-hosted" : "fallback";
    const summary = `runs-on: ${JSON.stringify(result.runsOn)} (${target}, ${result.reason}), ${result.selfHostedSlots} of ${result.runners.length} slot(s) self-hosted`;
    info(`${summary}. ${result.message}`);
    appendSummary(summary);

    setOutput("runs-on", JSON.stringify(result.runsOn));
    setOutput("runners", JSON.stringify(result.runners));
    setOutput("self-hosted", String(result.selfHosted));
    setOutput("self-hosted-slots", String(result.selfHostedSlots));
    setOutput("reason", result.reason);
  } catch (thrown) {
    if (thrown instanceof InputError) {
      setFailed(thrown.message);
      return;
    }
    throw thrown;
  }
}
