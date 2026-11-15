<script setup lang="ts">
import Icon from "./Icon.vue";

const reviewers = [
  { name: "Code review", agent: "agent-pr-reviewer", rev: 2 },
  { name: "Security review", agent: "agent-security", rev: 1 },
];

const nodes = [
  { icon: "arrow-right", name: "Sequence", text: "The next step starts when the last one is complete." },
  { icon: "split", name: "Parallel", text: "Independent work runs at once, then joins." },
  { icon: "repeat", name: "Repeat", text: "A review loop with an exit condition and a limit." },
  { icon: "pause", name: "Wait", text: "Pauses for a check, a person or a delay without using an agent turn." },
];
</script>

<template>
  <figure class="agent-flow">
    <div class="agent-flow__canvas">
      <div class="agent-flow__track">
        <section class="agent-flow__step is-agent">
          <span class="agent-flow__step-icon"><Icon name="code" :size="16" /></span>
          <strong>Implement</strong>
          <span class="agent-flow__step-text">Make the change in a fresh session.</span>
          <span class="agent-flow__pin"><Icon name="pin" :size="11" /> agent-coder rev 4</span>
        </section>

        <span class="agent-flow__link" aria-hidden="true" />

        <section class="agent-flow__repeat">
          <span class="agent-flow__frame-label is-repeat">
            <Icon name="repeat" :size="12" /> Repeat until approved, up to its limit
          </span>

          <div class="agent-flow__parallel">
            <span class="agent-flow__frame-label is-parallel"><Icon name="split" :size="12" /> Parallel</span>
            <div class="agent-flow__reviewers">
              <div v-for="item in reviewers" :key="item.name" class="agent-flow__step is-agent is-compact">
                <strong><Icon name="scan-eye" :size="14" /> {{ item.name }}</strong>
                <span class="agent-flow__pin"><Icon name="pin" :size="11" /> {{ item.agent }} rev {{ item.rev }}</span>
              </div>
            </div>
            <span class="agent-flow__join">join</span>
          </div>

          <div class="agent-flow__loop">
            <span class="agent-flow__loop-rail" aria-hidden="true" />
            <span class="agent-flow__rejected">Rejected</span>
            <div class="agent-flow__step is-agent is-compact">
              <strong><Icon name="wrench" :size="14" /> Revise, then review again</strong>
              <span class="agent-flow__pin"><Icon name="pin" :size="11" /> agent-coder rev 4</span>
            </div>
          </div>
        </section>

        <span class="agent-flow__link" aria-hidden="true"><span>approved</span></span>

        <section class="agent-flow__step is-wait">
          <span class="agent-flow__step-icon"><Icon name="pause" :size="16" /></span>
          <strong>Wait for CI</strong>
          <span class="agent-flow__step-text">An external check. No agent turn is spent while it runs.</span>
        </section>

        <span class="agent-flow__link" aria-hidden="true" />

        <section class="agent-flow__step is-done">
          <span class="agent-flow__step-icon"><Icon name="circle-check" :size="16" /></span>
          <strong>Done</strong>
          <span class="agent-flow__step-text">Evidence recorded for every step.</span>
        </section>
      </div>

      <p class="agent-flow__fresh">
        <Icon name="sparkles" :size="13" /> Every agent step runs in a fresh session, pinned to an exact revision.
      </p>
    </div>

    <ul class="agent-flow__legend">
      <li v-for="node in nodes" :key="node.name">
        <span class="agent-flow__legend-icon"><Icon :name="node.icon" :size="15" /></span>
        <span>
          <strong>{{ node.name }}</strong>
          <span>{{ node.text }}</span>
        </span>
      </li>
    </ul>

    <figcaption>
      <Icon name="history" :size="14" />
      A restart rebuilds the run from its record, so finished work is never paid for twice.
    </figcaption>
  </figure>
</template>

<style scoped>
.agent-flow {
  --teal: #14b8a6;
  --violet: #6f4cff;
  --amber: #f59e0b;
  --go: #10b981;
  --stop: #e11d48;
  margin: 24px 0 0;
}

.agent-flow__canvas {
  padding: 22px 20px 16px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 24px;
  background: color-mix(in srgb, var(--vp-c-bg) 90%, transparent);
  box-shadow: 0 30px 70px -40px rgba(36, 28, 90, 0.45);
}

.agent-flow__track {
  display: grid;
  grid-template-columns:
    minmax(0, 1fr) 36px minmax(0, 1.75fr) 64px minmax(0, 0.95fr) 36px minmax(0, 0.8fr);
  align-items: center;
}

.agent-flow__step {
  --tone: var(--teal);
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 6px;
  min-width: 0;
  padding: 14px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 18px;
  background: var(--vp-c-bg);
}

.agent-flow__step.is-wait {
  --tone: var(--amber);
}

.agent-flow__step.is-done {
  --tone: var(--go);
}

.agent-flow__step.is-compact {
  gap: 6px;
  padding: 10px 12px;
  border-radius: 14px;
}

.agent-flow__step-icon {
  display: grid;
  place-items: center;
  width: 30px;
  height: 30px;
  border-radius: 10px;
  background: color-mix(in srgb, var(--tone) 16%, transparent);
  color: color-mix(in srgb, var(--tone) 85%, var(--vp-c-text-1));
}

.agent-flow__step strong {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  color: var(--vp-c-text-1);
  font-size: 14px;
  line-height: 1.3;
}

.agent-flow__step.is-compact strong {
  font-size: 13px;
}

.agent-flow__step.is-compact strong :deep(svg) {
  flex: none;
  color: color-mix(in srgb, var(--tone) 85%, var(--vp-c-text-1));
}

.agent-flow__step-text {
  color: var(--vp-c-text-2);
  font-size: 12.5px;
  line-height: 1.45;
}

.agent-flow__pin {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  max-width: 100%;
  padding: 1px 8px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--teal) 12%, transparent);
  color: color-mix(in srgb, var(--teal) 80%, var(--vp-c-text-1));
  font-family: var(--vp-font-family-mono);
  font-size: 10.5px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.agent-flow__link {
  position: relative;
  display: grid;
  place-items: center;
  align-self: center;
  height: 24px;
}

.agent-flow__link::before {
  content: "";
  position: absolute;
  top: 50%;
  left: 4px;
  right: 6px;
  height: 2px;
  background: var(--vp-c-divider);
}

.agent-flow__link::after {
  content: "";
  position: absolute;
  top: calc(50% - 4px);
  right: 2px;
  border: 5px solid transparent;
  border-left: 6px solid var(--vp-c-text-3);
  border-right: 0;
}

.agent-flow__link span {
  position: relative;
  z-index: 1;
  padding: 1px 6px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--go) 14%, var(--vp-c-bg));
  color: color-mix(in srgb, var(--go) 85%, var(--vp-c-text-1));
  font-size: 10.5px;
  font-weight: 600;
}

.agent-flow__repeat {
  position: relative;
  display: flex;
  flex-direction: column;
  gap: 12px;
  min-width: 0;
  padding: 28px 12px 12px;
  border: 1.5px dashed color-mix(in srgb, var(--violet) 45%, var(--vp-c-divider));
  border-radius: 20px;
  background: color-mix(in srgb, var(--violet) 4%, transparent);
}

.agent-flow__frame-label {
  --tone: var(--violet);
  position: absolute;
  top: -11px;
  left: 14px;
  display: inline-flex;
  align-items: center;
  gap: 5px;
  max-width: calc(100% - 28px);
  padding: 2px 10px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--tone) 14%, var(--vp-c-bg));
  color: color-mix(in srgb, var(--tone) 85%, var(--vp-c-text-1));
  font-size: 11px;
  font-weight: 600;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.agent-flow__frame-label.is-parallel {
  --tone: var(--teal);
}

.agent-flow__parallel {
  position: relative;
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  align-items: center;
  gap: 10px;
  padding: 20px 10px 10px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 16px;
  background: var(--vp-c-bg-soft);
}

.agent-flow__reviewers {
  display: flex;
  flex-direction: column;
  gap: 8px;
  min-width: 0;
}

.agent-flow__join {
  align-self: stretch;
  display: grid;
  place-items: center;
  width: 22px;
  border-radius: 8px;
  background: color-mix(in srgb, var(--teal) 16%, transparent);
  color: color-mix(in srgb, var(--teal) 85%, var(--vp-c-text-1));
  font-size: 10.5px;
  font-weight: 600;
  writing-mode: vertical-rl;
}

.agent-flow__loop {
  position: relative;
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 6px;
  padding-left: 30px;
}

.agent-flow__loop .agent-flow__step {
  align-self: stretch;
  border-style: dashed;
}

.agent-flow__loop-rail {
  position: absolute;
  top: -6px;
  bottom: 22px;
  left: 8px;
  width: 16px;
  border: 2px solid color-mix(in srgb, var(--violet) 60%, var(--vp-c-divider));
  border-top: 0;
  border-right: 0;
  border-radius: 0 0 0 12px;
}

.agent-flow__loop-rail::after {
  content: "";
  position: absolute;
  top: -6px;
  left: -4px;
  border: 5px solid transparent;
  border-bottom: 7px solid color-mix(in srgb, var(--violet) 60%, var(--vp-c-divider));
  border-top: 0;
}

.agent-flow__rejected {
  padding: 1px 9px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--stop) 14%, transparent);
  color: color-mix(in srgb, var(--stop) 85%, var(--vp-c-text-1));
  font-size: 11px;
  font-weight: 600;
}

.agent-flow__fresh {
  display: flex;
  align-items: flex-start;
  gap: 6px;
  margin: 16px 0 0;
  color: var(--vp-c-text-2);
  font-size: 12.5px;
}

.agent-flow__fresh :deep(svg) {
  flex: none;
  margin-top: 3px;
  color: var(--teal);
}

.agent-flow__legend {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(min(100%, 210px), 1fr));
  gap: 10px;
  margin: 16px 0 0;
  padding: 0;
  list-style: none;
}

.agent-flow__legend li {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  margin: 0;
  padding: 12px 14px;
  border-radius: 16px;
  background: var(--vp-c-bg-soft);
}

.agent-flow__legend-icon {
  display: grid;
  flex: none;
  place-items: center;
  width: 30px;
  height: 30px;
  border-radius: 10px;
  background: color-mix(in srgb, var(--teal) 16%, transparent);
  color: color-mix(in srgb, var(--teal) 85%, var(--vp-c-text-1));
}

.agent-flow__legend strong {
  display: block;
  color: var(--vp-c-text-1);
  font-size: 13.5px;
}

.agent-flow__legend li span span {
  display: block;
  color: var(--vp-c-text-2);
  font-size: 12.5px;
  line-height: 1.45;
}

.agent-flow figcaption {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  margin-top: 14px;
  color: var(--vp-c-text-2);
  font-size: 13px;
  line-height: 1.5;
}

.agent-flow figcaption :deep(svg) {
  flex: none;
  margin-top: 3px;
  color: var(--teal);
}

@media (max-width: 860px) {
  .agent-flow__track {
    grid-template-columns: minmax(0, 1fr);
  }

  .agent-flow__link {
    height: 40px;
  }

  .agent-flow__link::before {
    top: 4px;
    bottom: 8px;
    left: calc(50% - 1px);
    right: auto;
    width: 2px;
    height: auto;
  }

  .agent-flow__link::after {
    top: auto;
    right: auto;
    bottom: 2px;
    left: calc(50% - 5px);
    border: 5px solid transparent;
    border-top: 6px solid var(--vp-c-text-3);
    border-bottom: 0;
  }

  .agent-flow__repeat {
    margin-top: 8px;
  }
}
</style>
