<script setup lang="ts">
import { ref } from "vue";
import Icon from "./Icon.vue";

type Outcome = { label: string; mark: "continues" | "ends" | "changes" };
type Scenario = {
  label: string;
  scope: string;
  call: string;
  lease: Outcome;
  workspace: Outcome;
  processes: Outcome;
  note: string;
  source: string;
  sourceLabel: string;
};

const repo = "https://github.com/TrogonStack/trogonai/blob/main";
const sdk = `${repo}/tsworkspace/packages/sandbox-ws-client`;
const contract = `${repo}/rsworkspace/crates/sandbox/trogon-sandbox-contract/src`;

const scenarios = [
  {
    label: "Disconnect",
    scope: "WebSocket client",
    call: "client.close()",
    lease: { label: "Still active", mark: "continues" },
    workspace: { label: "Still on the worker", mark: "continues" },
    processes: { label: "Follow their own lifetime", mark: "continues" },
    note: "Closing the socket disconnects the application. The broker still governs lease expiry, and a client can reconnect while the lease is active.",
    source: `${sdk}/src/client.ts`,
    sourceLabel: "Client implementation",
  },
  {
    label: "Pause",
    scope: "Daytona worker",
    call: "sandbox.pause()",
    lease: { label: "Paused until resume or expiry", mark: "changes" },
    workspace: { label: "Files kept on disk", mark: "continues" },
    processes: { label: "Stopped", mark: "ends" },
    note: "Pausing a Daytona sandbox stops its guest. Resume makes its files available again. Start any commands or services you need again.",
    source: `${repo}/rsworkspace/crates/sandbox/trogon-sandbox-daytona-worker/README.md#behavior`,
    sourceLabel: "Daytona pause behavior",
  },
  {
    label: "Stop",
    scope: "Named sandbox",
    call: "sandbox.stop()",
    lease: { label: "Current run released", mark: "ends" },
    workspace: { label: "Requested snapshot retained on success", mark: "changes" },
    processes: { label: "Check native completion", mark: "changes" },
    note: "Stop keeps the sandbox name for another run. Restoring files depends on what the provider captured. The stop acknowledgement alone cannot confirm every guest process has exited.",
    source: `${contract}/stop_lease_request.rs`,
    sourceLabel: "Stop contract",
  },
  {
    label: "Restore",
    scope: "Vercel disk snapshot",
    call: "Sandbox.fromSnapshot(…)",
    lease: { label: "New lease", mark: "changes" },
    workspace: { label: "Files from the snapshot", mark: "continues" },
    processes: { label: "Start fresh", mark: "changes" },
    note: "A Vercel disk snapshot supplies the files for a new sandbox. With a saved restart plan, the SDK can launch registered servers again and wait for readiness.",
    source: `${sdk}/docs/how-to/restore-managed-servers.md`,
    sourceLabel: "Restoring managed servers",
  },
  {
    label: "Delete",
    scope: "Disposable sandbox",
    call: "sandbox.delete()",
    lease: { label: "Deletion requested", mark: "ends" },
    workspace: { label: "Lease-bound storage marked for purge", mark: "ends" },
    processes: { label: "Check native completion", mark: "changes" },
    note: "Delete also requests cleanup of resumable storage tied to the lease. Wait for the release outcome and check the provider when your next step depends on cleanup.",
    source: `${contract}/delete_lease_request.rs`,
    sourceLabel: "Delete contract",
  },
] as const satisfies readonly Scenario[];

const resources = [
  { key: "lease", label: "Lease", icon: "clock" },
  { key: "workspace", label: "Workspace", icon: "layers" },
  { key: "processes", label: "Processes", icon: "activity" },
] as const;

const current = ref<Scenario>(scenarios[1]);
</script>

<template>
  <figure class="lifecycle" aria-label="Sandbox lifecycle by action">
    <fieldset class="choices">
      <legend>Choose a lifecycle action</legend>
      <label v-for="scenario in scenarios" :key="scenario.label">
        <input v-model="current" type="radio" name="sandbox-lifecycle-action" :value="scenario" />
        <span>{{ scenario.label }}</span>
      </label>
    </fieldset>
    <div class="diagram" aria-live="polite" aria-atomic="true">
      <div class="operation">
        <span>{{ current.scope }}</span>
        <code>{{ current.call }}</code>
      </div>
      <dl class="lanes">
        <div v-for="resource in resources" :key="resource.key" class="lane" :class="current[resource.key].mark">
          <dt><Icon :name="resource.icon" :size="17" />{{ resource.label }}</dt>
          <dd>
            <svg class="trace" viewBox="0 0 100 20" preserveAspectRatio="none" aria-hidden="true">
              <path d="M 0 10 H 100" />
            </svg>
            <span class="outcome">
              <Icon :name="{ continues: 'check', ends: 'x', changes: 'arrow-right' }[current[resource.key].mark]" :size="14" />
              {{ current[resource.key].label }}
            </span>
          </dd>
        </div>
      </dl>
      <div class="note">
        <p>{{ current.note }}</p>
        <a :href="current.source">{{ current.sourceLabel }}</a>
      </div>
    </div>
  </figure>
</template>

<style scoped>
.lifecycle {
  --ink: color-mix(in srgb, #0e7490 78%, var(--vp-c-text-1));
  --paper: color-mix(in srgb, #38bdf8 4%, var(--vp-c-bg));
  margin: 20px 0;
  overflow: hidden;
  border: 1px solid var(--vp-c-divider);
  border-radius: 20px;
  background: var(--vp-c-bg);
  line-height: 1.45;
}
:global(.dark) .lifecycle {
  --ink: #7dd3fc;
}
.choices {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
  min-width: 0;
  margin: 0;
  padding: 9px;
  border: 0;
  border-bottom: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg-soft);
}
.choices legend {
  position: absolute;
  width: 1px;
  height: 1px;
  overflow: hidden;
  clip-path: inset(50%);
  white-space: nowrap;
}
.choices label {
  position: relative;
  flex: 1 1 auto;
  text-align: center;
  cursor: pointer;
}
.choices input {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  margin: 0;
  opacity: 0;
  cursor: pointer;
}
.choices label span {
  display: block;
  padding: 7px 10px;
  border: 1px solid transparent;
  border-radius: 9px;
  color: var(--vp-c-text-2);
  font-size: 13px;
  font-weight: 600;
}
.choices label:hover span, .choices input:checked + span {
  color: var(--ink);
  background: var(--vp-c-bg);
}
.choices input:checked + span {
  border-color: var(--vp-c-divider);
  box-shadow: 0 1px 3px rgb(15 23 42 / 8%);
}
.choices input:focus-visible + span {
  outline: 2px solid var(--ink);
  outline-offset: 1px;
}
.operation {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  justify-content: space-between;
  gap: 5px 12px;
  padding: 20px 24px 12px;
}
.operation span {
  color: var(--vp-c-text-1);
  font-weight: 650;
}
.operation code {
  padding: 0;
  background: transparent;
  color: var(--ink);
  font: 12px var(--vp-font-family-mono);
}
.lanes {
  display: grid;
  gap: 10px;
  margin: 0;
  padding: 5px 24px 18px;
}
.lane {
  display: grid;
  grid-template-columns: minmax(92px, 20%) minmax(0, 1fr);
  align-items: center;
  color: var(--vp-c-text-1);
  font-size: 13px;
}
.lane dt, .outcome {
  display: flex;
  align-items: center;
  gap: 8px;
}
.lane dt {
  font-weight: 600;
}
.lane dt :deep(svg) {
  color: var(--vp-c-text-2);
}
.lane dd {
  display: grid;
  grid-template-columns: minmax(25px, 1fr) minmax(155px, 55%);
  align-items: center;
  margin: 0;
}
.trace {
  width: 100%;
  height: 20px;
  overflow: visible;
  color: var(--ink);
}
.trace path {
  fill: none;
  stroke: currentColor;
  stroke-width: 1.4;
  vector-effect: non-scaling-stroke;
}
.changes .trace path { stroke-dasharray: 5 3; }
.ends .trace path { stroke-dasharray: 1 3; }
.ends .trace { color: var(--vp-c-text-3); }
.outcome {
  width: fit-content;
  min-height: 30px;
  padding: 4px 10px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 8px;
  background: var(--paper);
}
.outcome :deep(svg) {
  color: var(--ink);
}
.ends .outcome { background: var(--vp-c-bg-soft); }
.ends .outcome :deep(svg) { color: var(--vp-c-text-2); }
.note {
  display: grid;
  justify-items: start;
  gap: 5px;
  padding: 12px 24px 15px;
  border-top: 1px solid var(--vp-c-divider);
  background: var(--paper);
}
.note p, .note a {
  margin: 0;
  font-size: 13px;
  line-height: 1.6;
}
.note p { color: var(--vp-c-text-2); }
@media (max-width: 540px) {
  .operation, .lanes, .note { padding-left: 14px; padding-right: 14px; }
  .lane dd { grid-template-columns: minmax(8px, 1fr) minmax(110px, 82%); }
  .lane dt, .outcome { gap: 5px; }
  .operation { align-items: flex-start; flex-direction: column; }
  .choices label span { padding: 6px; }
}
</style>
