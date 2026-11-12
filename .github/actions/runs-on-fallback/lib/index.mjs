import fs from "node:fs";

import { appendSummary, getInput, info, setFailed, setOutput, warning } from "./core.mjs";

const PER_PAGE = 100;

// A runaway Link chain must not hold a job open; 50 pages is 5000 runners.
const MAX_PAGES = 50;

// A hung API call must not hold the selection job open; the fallback is always
// available, so waiting longer than this buys nothing.
const REQUEST_TIMEOUT_MS = 15000;

// The whole search, across every page and group, stays well inside the five
// minutes a selection job gets, so it always reaches the fallback in time.
const SEARCH_TIMEOUT_MS = 60000;

const JOB_NAME = /^[a-z0-9][a-z0-9_-]*$/;

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

// Routing hygiene, not a security boundary. A pull request from a fork controls
// the workflow files and this action, so it can write the self-hosted labels
// into `runs-on` itself. This only keeps events that were never meant to reach
// the runner from being routed there by accident; the real controls are the
// repository and runner group settings described in the README.
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

function parseJobs(text) {
  const names = (text ?? "").split(/[\s,]+/).filter((name) => name.length > 0);
  if (names.length === 0) {
    names.push("default");
  }

  if (!names.every((name) => JOB_NAME.test(name))) {
    throw new InputError(
      "Every name in the `jobs` input must be lowercase letters, digits, dashes and underscores, starting with a letter or digit.",
    );
  }

  if (new Set(names).size !== names.length) {
    throw new InputError("The names in the `jobs` input must be unique.");
  }

  return names;
}

function readEvent(path) {
  try {
    const event = JSON.parse(fs.readFileSync(path, "utf8"));
    return typeof event === "object" ? event : null;
  } catch {
    return null;
  }
}

function listUrl({ apiUrl, path, query }) {
  return (page) => `${apiUrl}/${path}?${query}per_page=${PER_PAGE}&page=${page}`;
}

// Calls `visit` with each page's items until it returns true or the pages run
// out. One walker serves every listing, so none of them can forget to paginate.
async function eachPage({ fetch, token, url, key, visit, deadline }) {
  for (let page = 1; page <= MAX_PAGES; page += 1) {
    const response = await fetch(url(page), {
      headers: {
        Accept: "application/vnd.github+json",
        Authorization: `Bearer ${token}`,
        "X-GitHub-Api-Version": "2022-11-28",
      },
      signal: AbortSignal.any([deadline, AbortSignal.timeout(REQUEST_TIMEOUT_MS)]),
    });

    if (!response.ok) {
      throw new Error(`the ${key} API answered HTTP ${response.status}`);
    }

    const body = await response.json();
    if (!Array.isArray(body?.[key])) {
      throw new Error(`the ${key} API answered without a ${key} list`);
    }

    if (visit(body[key])) {
      return;
    }

    // A full page is the only sign of more; Link is the authority when sent.
    const link = response.headers?.get?.("link");
    const more = link == null ? true : /rel="next"/.test(link);
    if (body[key].length < PER_PAGE || !more) {
      return;
    }
  }
}

// Stops once every job is covered: the rest of a long list cannot change a
// decision already reached.
async function countIdleUpTo({ fetch, token, scope, repository, apiUrl, labels, limit, deadline }) {
  const [owner, name] = repository.split("/");
  let idle = 0;

  const countFrom = (path) =>
    eachPage({
      fetch,
      token,
      key: "runners",
      deadline,
      url: listUrl({ apiUrl, path, query: "" }),
      visit: (runners) => {
        idle += countIdle(runners, labels);
        return idle >= limit;
      },
    });

  if (scope === "repository") {
    await countFrom(`repos/${repository}/actions/runners`);
    return Math.min(idle, limit);
  }

  // The organization-wide list includes runners in groups this repository may
  // not use, and a job sent to one would queue forever. Only the groups GitHub
  // reports as visible to this repository are asked about.
  const groups = [];
  await eachPage({
    fetch,
    token,
    key: "runner_groups",
    deadline,
    url: listUrl({
      apiUrl,
      path: `orgs/${owner}/actions/runner-groups`,
      query: `visible_to_repository=${encodeURIComponent(name)}&`,
    }),
    visit: (page) => {
      groups.push(...page);
      return false;
    },
  });

  for (const group of groups) {
    // Which workflows such a group admits cannot be checked reliably from here,
    // so it counts as unavailable rather than risking a job that queues forever.
    if (group.restricted_to_workflows === true) {
      continue;
    }
    await countFrom(`orgs/${owner}/actions/runner-groups/${group.id}/runners`);
    if (idle >= limit) {
      break;
    }
  }

  return Math.min(idle, limit);
}

// Decides without touching the process, so the tests drive the code that ships
// rather than a copy of it. Only caller mistakes throw: a runner that cannot be
// asked about is a reason to use the hosted fallback, never to fail the run.
export async function select({
  githubToken,
  primaryLabels,
  fallbackLabels,
  scope,
  jobs,
  env = process.env,
  fetch = globalThis.fetch,
  searchTimeoutMs = SEARCH_TIMEOUT_MS,
}) {
  const chosenScope = (scope ?? "").trim() || "repository";
  if (!SCOPES.includes(chosenScope)) {
    throw new InputError(`The \`scope\` input must be one of: ${SCOPES.join(", ")}.`);
  }

  const names = parseJobs(jobs);

  const fallback = parseLabels(fallbackLabels);
  if (fallback.length === 0) {
    throw new InputError("The `fallback-labels` input must name at least one runner label.");
  }

  const primary = parseLabels(primaryLabels);
  const warnings = [];

  // Every name gets an entry, already holding the fallback when it is not
  // self-hosted, so a caller reads its job's labels without a fallback of its
  // own. The first names take the runners, so callers list the longest job first.
  const result = (taken, reason, message) => {
    const runners = Object.fromEntries(
      names.map((name, index) => [name, index < taken ? primary : fallback]),
    );
    return {
      runners,
      runsOn: runners[names[0]],
      selfHosted: taken > 0,
      selfHostedCount: taken,
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
      scope: chosenScope,
      repository: env.GITHUB_REPOSITORY,
      apiUrl,
      labels: primary,
      limit: names.length,
      deadline: AbortSignal.timeout(searchTimeoutMs),
    });

    if (idle > 0) {
      return result(idle, "idle-runner", `${idle} of ${names.length} job(s) go to idle runners carrying ${primary.join(", ")}.`);
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
      jobs: getInput("jobs"),
    });

    for (const line of result.warnings) {
      warning(line);
    }

    const target = result.selfHosted ? "self-hosted" : "fallback";
    const total = Object.keys(result.runners).length;
    const summary = `runs-on: ${JSON.stringify(result.runsOn)} (${target}, ${result.reason}), ${result.selfHostedCount} of ${total} job(s) self-hosted`;
    info(`${summary}. ${result.message}`);
    appendSummary(summary);

    setOutput("runs-on", JSON.stringify(result.runsOn));
    setOutput("runners", JSON.stringify(result.runners));
    setOutput("self-hosted", String(result.selfHosted));
    setOutput("self-hosted-count", String(result.selfHostedCount));
    setOutput("reason", result.reason);
  } catch (thrown) {
    if (thrown instanceof InputError) {
      setFailed(thrown.message);
      return;
    }
    throw thrown;
  }
}
