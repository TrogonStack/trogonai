// A stand-in for the few functions this action uses from `@actions/core`,
// carrying their names and their behaviour. The package itself is ESM-only and
// reaches `undici` through `@actions/http-client`, so depending on it would mean
// a bundler and a committed `dist/`, and the tests would then exercise something
// other than the file the runner executes.

import crypto from "node:crypto";
import fs from "node:fs";
import { EOL } from "node:os";

// The runner uppercases an input name and replaces spaces, and nothing else, so
// `github-token` arrives as INPUT_GITHUB-TOKEN and not INPUT_GITHUB_TOKEN.
export function getInput(name, options = {}) {
  const value = process.env[`INPUT_${name.replace(/ /g, "_").toUpperCase()}`] ?? "";

  if (options.required && !value) {
    throw new Error(`Input required and not supplied: ${name}`);
  }

  return options.trimWhitespace === false ? value : value.trim();
}

// Workflow commands end at a newline, so a message carrying one would close the
// annotation and log the remainder as its own line.
function escapeData(value) {
  return String(value).replace(/%/g, "%25").replace(/\r/g, "%0D").replace(/\n/g, "%0A");
}

export function info(message) {
  process.stdout.write(`${message}${EOL}`);
}

export function warning(message) {
  process.stdout.write(`::warning::${escapeData(message)}${EOL}`);
}

export function error(message) {
  process.stdout.write(`::error::${escapeData(message)}${EOL}`);
}

export function setFailed(message) {
  // Not `process.exit`, which can truncate output still buffered on stdout.
  process.exitCode = 1;
  error(message);
}

export function setOutput(name, value) {
  const file = process.env.GITHUB_OUTPUT;

  if (!file) {
    throw new Error("Unable to find environment variable for file command OUTPUT");
  }

  // The delimited form, because a value holding a newline would otherwise be
  // read as the start of the next output.
  const delimiter = `ghadelimiter_${crypto.randomUUID()}`;

  if (name.includes(delimiter) || String(value).includes(delimiter)) {
    throw new Error(`Unexpected input: name and value should not contain the delimiter`);
  }

  fs.appendFileSync(file, `${name}<<${delimiter}${EOL}${value}${EOL}${delimiter}${EOL}`, {
    encoding: "utf8",
  });
}

// Optional, unlike outputs: a missing summary file only means the run has no
// summary page to write to, which is no reason to lose the decision itself.
export function appendSummary(line) {
  const file = process.env.GITHUB_STEP_SUMMARY;

  if (file) {
    fs.appendFileSync(file, `${line}${EOL}`, { encoding: "utf8" });
  }
}
