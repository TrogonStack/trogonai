<script setup lang="ts">
import Icon from "./Icon.vue";

type Outcome = "withdrawn" | "rejected" | "approved";

const proposals: { id: string; outcome: Outcome; label: string; text: string; note?: string }[] = [
  {
    id: "prop-b804",
    outcome: "withdrawn",
    label: "Withdrawn",
    text: "Withdrawn by its author before anyone judged it.",
  },
  {
    id: "prop-9c21",
    outcome: "rejected",
    label: "Rejected",
    text: "Rejected by the verifier, with its reasons kept.",
  },
  {
    id: "prop-7f3a",
    outcome: "approved",
    label: "Approved",
    text: "Approved, and becomes revision 2.",
    note: "supersedes prop-b804",
  },
];
</script>

<template>
  <figure class="agent-rev">
    <header class="agent-rev__identity">
      <span class="agent-rev__identity-icon"><Icon name="bot" :size="18" /></span>
      <span class="agent-rev__identity-text">
        <code>agent-pr-reviewer</code>
        <span>A stable identity. It never changes in place.</span>
      </span>
      <span class="agent-rev__identity-tag"><Icon name="history" :size="13" /> Registry history</span>
    </header>

    <div class="agent-rev__diagram">
      <section class="agent-rev__revision is-base">
        <span class="agent-rev__revision-icon"><Icon name="git-branch" :size="16" /></span>
        <strong>Revision 1</strong>
        <span class="agent-rev__pill is-base">Base</span>
        <p>Every proposal starts from it.</p>
      </section>

      <svg class="agent-rev__fan" viewBox="0 0 60 300" preserveAspectRatio="none" aria-hidden="true">
        <path d="M0 150 C 30 150, 30 50, 60 50" class="is-muted" />
        <path d="M0 150 L 60 150" class="is-muted" />
        <path d="M0 150 C 30 150, 30 250, 60 250" class="is-go" />
      </svg>

      <ol class="agent-rev__proposals">
        <li v-for="item in proposals" :key="item.id" :class="`is-${item.outcome}`">
          <span class="agent-rev__proposal-head">
            <Icon name="git-pull-request" :size="14" />
            <code>{{ item.id }}</code>
            <span :class="['agent-rev__pill', `is-${item.outcome}`]">{{ item.label }}</span>
          </span>
          <span class="agent-rev__proposal-text">{{ item.text }}</span>
          <span v-if="item.note" class="agent-rev__proposal-note">{{ item.note }}</span>
        </li>
      </ol>

      <svg class="agent-rev__merge" viewBox="0 0 60 300" preserveAspectRatio="none" aria-hidden="true">
        <path d="M0 250 C 30 250, 30 150, 60 150" class="is-go is-flow" />
      </svg>

      <section class="agent-rev__revision is-active">
        <span class="agent-rev__revision-icon"><Icon name="badge-check" :size="16" /></span>
        <strong>Revision 2</strong>
        <span class="agent-rev__pill is-approved">Active</span>
        <p>Minted only when a proposal is activated.</p>
      </section>
    </div>

    <ul class="agent-rev__sessions">
      <li>
        <span class="agent-rev__session-icon"><Icon name="pin" :size="14" /></span>
        <span>A session started on <strong>revision 1</strong> finishes on revision 1.</span>
      </li>
      <li class="is-new">
        <span class="agent-rev__session-icon"><Icon name="pin" :size="14" /></span>
        <span>New sessions start on <strong>revision 2</strong>.</span>
      </li>
    </ul>

    <figcaption>
      Only the activated proposal reaches the agent's history. Proposals that never ship leave the registry
      untouched.
    </figcaption>
  </figure>
</template>

<style scoped>
.agent-rev {
  --teal: #14b8a6;
  --go: #10b981;
  --stop: #e11d48;
  margin: 24px 0 0;
  overflow: hidden;
  border: 1px solid var(--vp-c-divider);
  border-radius: 24px;
  background: color-mix(in srgb, var(--vp-c-bg) 90%, transparent);
  box-shadow: 0 30px 70px -40px rgba(36, 28, 90, 0.45);
}

.agent-rev__identity {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 14px 20px;
  border-bottom: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg-soft);
}

.agent-rev__identity-icon {
  display: grid;
  flex: none;
  place-items: center;
  width: 34px;
  height: 34px;
  border-radius: 11px;
  background: color-mix(in srgb, var(--teal) 16%, transparent);
  color: color-mix(in srgb, var(--teal) 85%, var(--vp-c-text-1));
}

.agent-rev__identity-text {
  display: flex;
  flex: 1;
  flex-direction: column;
  min-width: 0;
}

.agent-rev__identity-text code {
  width: fit-content;
  padding: 0;
  background: none;
  color: var(--vp-c-text-1);
  font-size: 14px;
  font-weight: 600;
}

.agent-rev__identity-text span {
  color: var(--vp-c-text-2);
  font-size: 13px;
}

.agent-rev__identity-tag {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 3px 10px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--teal) 14%, transparent);
  color: color-mix(in srgb, var(--teal) 80%, var(--vp-c-text-1));
  font-size: 11.5px;
  font-weight: 600;
  white-space: nowrap;
}

.agent-rev__diagram {
  display: grid;
  grid-template-columns: minmax(0, 170px) 56px minmax(0, 1fr) 56px minmax(0, 170px);
  align-items: center;
  padding: 24px 20px 8px;
}

.agent-rev__revision {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 6px;
  padding: 16px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 20px;
  background: var(--vp-c-bg);
}

.agent-rev__revision.is-active {
  border-color: color-mix(in srgb, var(--go) 45%, var(--vp-c-divider));
  box-shadow: 0 0 0 4px color-mix(in srgb, var(--go) 10%, transparent);
}

.agent-rev__revision-icon {
  display: grid;
  place-items: center;
  width: 30px;
  height: 30px;
  border-radius: 10px;
  background: color-mix(in srgb, var(--teal) 16%, transparent);
  color: color-mix(in srgb, var(--teal) 85%, var(--vp-c-text-1));
}

.is-active .agent-rev__revision-icon {
  background: color-mix(in srgb, var(--go) 16%, transparent);
  color: var(--go);
}

.agent-rev__revision strong {
  color: var(--vp-c-text-1);
  font-size: 15px;
}

.agent-rev__revision p {
  margin: 0;
  color: var(--vp-c-text-2);
  font-size: 12.5px;
  line-height: 1.45;
}

.agent-rev__pill {
  --tone: var(--vp-c-text-3);
  display: inline-flex;
  align-items: center;
  gap: 6px;
  width: fit-content;
  padding: 1px 9px 1px 7px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--tone) 14%, transparent);
  color: color-mix(in srgb, var(--tone) 85%, var(--vp-c-text-1));
  font-size: 11.5px;
  font-weight: 600;
  white-space: nowrap;
}

.agent-rev__pill::before {
  content: "";
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--tone);
}

.agent-rev__pill.is-base {
  --tone: var(--teal);
}

.agent-rev__pill.is-approved {
  --tone: var(--go);
}

.agent-rev__pill.is-rejected {
  --tone: var(--stop);
}

.agent-rev__fan,
.agent-rev__merge {
  align-self: stretch;
  width: 100%;
  height: 100%;
  overflow: visible;
}

.agent-rev__fan path,
.agent-rev__merge path {
  fill: none;
  stroke-width: 2.5;
  stroke-linecap: round;
  vector-effect: non-scaling-stroke;
}

.agent-rev__fan .is-muted {
  stroke: var(--vp-c-text-3);
  stroke-dasharray: 4 5;
  opacity: 0.7;
}

.agent-rev .is-go {
  stroke: var(--go);
}

.agent-rev .is-flow {
  stroke-dasharray: 8 6;
  animation: agent-rev-flow 1.4s linear infinite;
}

.agent-rev__proposals {
  display: grid;
  grid-auto-rows: 1fr;
  gap: 12px;
  margin: 0;
  padding: 0;
  list-style: none;
}

.agent-rev__proposals li {
  display: flex;
  flex-direction: column;
  justify-content: center;
  gap: 4px;
  margin: 0;
  padding: 12px 14px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 16px;
  background: var(--vp-c-bg-soft);
}

.agent-rev__proposals li.is-withdrawn,
.agent-rev__proposals li.is-rejected {
  border-style: dashed;
}

.agent-rev__proposals li.is-approved {
  border-color: color-mix(in srgb, var(--go) 45%, var(--vp-c-divider));
  background: color-mix(in srgb, var(--go) 6%, var(--vp-c-bg-soft));
}

.agent-rev__proposal-head {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
  color: var(--vp-c-text-2);
}

.agent-rev__proposal-head code {
  padding: 0;
  background: none;
  color: var(--vp-c-text-1);
  font-size: 13px;
  font-weight: 600;
}

.agent-rev__proposal-head .agent-rev__pill {
  margin-left: auto;
}

.agent-rev__proposal-text {
  color: var(--vp-c-text-2);
  font-size: 12.5px;
  line-height: 1.45;
}

.agent-rev__proposal-note {
  width: fit-content;
  padding: 1px 8px;
  border-radius: 7px;
  background: var(--vp-c-bg);
  color: var(--vp-c-text-3);
  font-family: var(--vp-font-family-mono);
  font-size: 11px;
}

.agent-rev__sessions {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 10px;
  margin: 16px 20px 0;
  padding: 0;
  list-style: none;
}

.agent-rev__sessions li {
  display: flex;
  align-items: center;
  gap: 10px;
  margin: 0;
  padding: 10px 14px;
  border-radius: 14px;
  background: var(--vp-c-bg-soft);
  color: var(--vp-c-text-2);
  font-size: 13px;
  line-height: 1.45;
}

.agent-rev__sessions strong {
  color: var(--vp-c-text-1);
}

.agent-rev__session-icon {
  display: grid;
  flex: none;
  place-items: center;
  width: 28px;
  height: 28px;
  border-radius: 9px;
  background: color-mix(in srgb, var(--teal) 16%, transparent);
  color: color-mix(in srgb, var(--teal) 85%, var(--vp-c-text-1));
}

.agent-rev__sessions li.is-new .agent-rev__session-icon {
  background: color-mix(in srgb, var(--go) 16%, transparent);
  color: var(--go);
}

.agent-rev figcaption {
  padding: 14px 20px 18px;
  color: var(--vp-c-text-2);
  font-size: 13px;
  line-height: 1.5;
}

@keyframes agent-rev-flow {
  to {
    stroke-dashoffset: -28;
  }
}

@media (prefers-reduced-motion: reduce) {
  .agent-rev .is-flow {
    animation: none;
  }
}

@media (max-width: 860px) {
  .agent-rev__diagram {
    grid-template-columns: minmax(0, 1fr);
    gap: 0;
  }

  .agent-rev__fan,
  .agent-rev__merge {
    display: none;
  }

  .agent-rev__proposals {
    position: relative;
    margin: 0;
    padding: 28px 0;
  }

  .agent-rev__proposals::before,
  .agent-rev__proposals::after {
    content: "";
    position: absolute;
    left: 50%;
    width: 2px;
    height: 20px;
    background: var(--vp-c-divider);
  }

  .agent-rev__proposals::before {
    top: 4px;
  }

  .agent-rev__proposals::after {
    bottom: 4px;
    background: var(--go);
  }

  .agent-rev__sessions {
    grid-template-columns: minmax(0, 1fr);
  }
}

@media (max-width: 560px) {
  .agent-rev__identity {
    flex-wrap: wrap;
  }

  .agent-rev__identity-tag {
    display: none;
  }
}
</style>
