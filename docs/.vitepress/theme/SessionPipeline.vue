<script setup lang="ts">
import Icon from "./Icon.vue";

const logEntries = ["message", "tool", "tool", "tool", "file", "artifact", "checkpoint", "message"];

const freshness = [
  { name: "Current", tone: "go", text: "Reflects every event up to now" },
  { name: "Lagging", tone: "wait", text: "Still answers, and says how far behind it is" },
  { name: "Indeterminate", tone: "unknown", text: "Could not check, and says so" },
];
</script>

<template>
  <figure class="session-pipeline">
    <div class="session-pipeline__diagram">
      <section class="session-pipeline__side" style="grid-area: write">
        <span class="session-pipeline__eyebrow">Write</span>
        <div class="session-pipeline__node">
          <span class="session-pipeline__badge is-actor"><Icon name="user" :size="20" /></span>
          <span><strong>You or an agent</strong><span>Sends a command</span></span>
        </div>
        <span class="session-pipeline__down" aria-hidden="true" />
        <div class="session-pipeline__node">
          <span class="session-pipeline__badge is-decider"><Icon name="gauge" :size="20" /></span>
          <span><strong>Decider</strong><span>Checks the command against the history so far</span></span>
        </div>
      </section>

      <div class="session-pipeline__link" style="grid-area: a1">
        <span class="session-pipeline__label">append</span>
        <span class="session-pipeline__line" />
        <span class="session-pipeline__chip">guarded</span>
      </div>

      <section class="session-pipeline__log" style="grid-area: log">
        <span class="session-pipeline__badge is-log"><Icon name="history" :size="22" /></span>
        <strong>Session log</strong>
        <span>One append-only stream per session. The only source of truth.</span>
        <ol class="session-pipeline__stack" aria-hidden="true">
          <li v-for="(kind, index) in logEntries" :key="index" :class="`is-${kind}`" />
          <li class="is-new" />
        </ol>
        <em>Never rewritten, never purged</em>
      </section>

      <div class="session-pipeline__link" style="grid-area: a2">
        <span class="session-pipeline__label">fold</span>
        <span class="session-pipeline__line" />
        <span class="session-pipeline__chip">rebuildable</span>
      </div>

      <section class="session-pipeline__side" style="grid-area: read">
        <span class="session-pipeline__eyebrow">Read</span>
        <div class="session-pipeline__node">
          <span class="session-pipeline__badge is-view"><Icon name="layers" :size="20" /></span>
          <span><strong>Read models</strong><span>Views folded from the log, never the authority</span></span>
        </div>
        <span class="session-pipeline__down" aria-hidden="true" />
        <div class="session-pipeline__node">
          <span class="session-pipeline__badge is-view"><Icon name="eye" :size="20" /></span>
          <span><strong>Queries</strong><span>Sessions, history and the one to resume</span></span>
        </div>
      </section>

      <div class="session-pipeline__vlink" style="grid-area: cv" aria-hidden="true">
        <span class="session-pipeline__line" />
        <span class="session-pipeline__chip">by digest</span>
      </div>

      <section class="session-pipeline__art" style="grid-area: art">
        <span class="session-pipeline__badge is-artifact"><Icon name="database" :size="20" /></span>
        <span>
          <strong>Artifact store</strong>
          <span>Diffs, terminal output and files. The bytes are stored first, then the log points at them.</span>
        </span>
      </section>
    </div>

    <div class="session-pipeline__freshness">
      <span class="session-pipeline__freshness-title">
        <Icon name="clock" :size="16" />
        Every answer says how fresh it is
      </span>
      <ul>
        <li v-for="item in freshness" :key="item.name" :class="`is-${item.tone}`">
          <strong>{{ item.name }}</strong>
          <span>{{ item.text }}</span>
        </li>
      </ul>
      <p>
        Ask to see your own write and you get it, or a clear error when the view cannot catch up in time. Never a
        plausible wrong answer.
      </p>
    </div>
  </figure>
</template>

<style scoped>
.session-pipeline {
  --amber: #f59e0b;
  margin: 24px 0 0;
  padding: 28px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 28px;
  background: color-mix(in srgb, var(--vp-c-bg) 86%, transparent);
  box-shadow: 0 24px 60px -32px rgba(36, 28, 90, 0.28);
}

.session-pipeline__diagram {
  display: grid;
  grid-template-areas:
    "write a1 log a2 read"
    ". . cv . ."
    ". . art . .";
  grid-template-columns: minmax(0, 1fr) 96px minmax(0, 1.05fr) 96px minmax(0, 1fr);
  grid-template-rows: auto 48px auto;
  align-items: center;
}

.session-pipeline__side {
  display: flex;
  flex-direction: column;
  align-items: stretch;
  padding: 14px;
  border-radius: 20px;
  background: var(--vp-c-bg-soft);
}

.session-pipeline__eyebrow {
  margin: 0 0 10px 4px;
  color: var(--vp-c-text-3);
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.08em;
  text-transform: uppercase;
}

.session-pipeline__node {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 10px;
  border-radius: 14px;
  background: var(--vp-c-bg);
}

.session-pipeline__node strong,
.session-pipeline__art strong {
  display: block;
  color: var(--vp-c-text-1);
  font-size: 14px;
  line-height: 1.3;
}

.session-pipeline__node span span,
.session-pipeline__art span span {
  display: block;
  color: var(--vp-c-text-2);
  font-size: 12.5px;
  line-height: 1.4;
}

.session-pipeline__down {
  align-self: center;
  width: 2px;
  height: 16px;
  margin: 2px 0;
  background: var(--vp-c-divider);
}

.session-pipeline__badge {
  display: grid;
  flex: none;
  place-items: center;
  width: 40px;
  height: 40px;
  border-radius: 12px;
  color: #fff;
  background:
    linear-gradient(160deg, rgba(255, 255, 255, 0.24), rgba(255, 255, 255, 0) 55%),
    var(--badge);
  box-shadow: 0 8px 18px -10px var(--badge);
}

.session-pipeline__badge.is-actor {
  --badge: #64748b;
}

.session-pipeline__badge.is-decider {
  --badge: #6f4cff;
}

.session-pipeline__badge.is-log {
  --badge: var(--amber);
  width: 52px;
  height: 52px;
  border-radius: 16px;
}

.session-pipeline__badge.is-view {
  --badge: #14b8a6;
}

.session-pipeline__badge.is-artifact {
  --badge: #2563eb;
}

.session-pipeline__link,
.session-pipeline__vlink {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 6px;
  padding: 0 6px;
}

.session-pipeline__label {
  color: var(--vp-c-text-3);
  font-size: 11px;
  font-weight: 500;
}

.session-pipeline__line {
  position: relative;
  width: 100%;
  height: 2px;
  border-radius: 2px;
  background: color-mix(in srgb, var(--amber) 55%, var(--vp-c-divider));
}

.session-pipeline__link .session-pipeline__line::after {
  content: "";
  position: absolute;
  top: 50%;
  right: -2px;
  width: 7px;
  height: 7px;
  border-top: 2px solid var(--amber);
  border-right: 2px solid var(--amber);
  transform: translateY(-50%) rotate(45deg);
}

.session-pipeline__chip {
  padding: 2px 8px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--amber) 14%, transparent);
  color: color-mix(in srgb, var(--amber) 75%, var(--vp-c-text-1));
  font-size: 11px;
  font-weight: 600;
  white-space: nowrap;
}

.session-pipeline__vlink {
  flex-direction: row;
  align-self: stretch;
  justify-content: center;
  gap: 10px;
}

.session-pipeline__vlink .session-pipeline__line {
  width: 2px;
  height: 100%;
}

.session-pipeline__vlink::before {
  content: "";
  width: 62px;
}

.session-pipeline__log {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 2px;
  padding: 20px 18px 16px;
  border: 1.5px solid color-mix(in srgb, var(--amber) 45%, var(--vp-c-divider));
  border-radius: 22px;
  background:
    linear-gradient(170deg, color-mix(in srgb, var(--amber) 10%, transparent), transparent 70%),
    var(--vp-c-bg);
  text-align: center;
}

.session-pipeline__log > strong {
  margin-top: 10px;
  color: var(--vp-c-text-1);
  font-size: 16px;
}

.session-pipeline__log > span:not(.session-pipeline__badge) {
  color: var(--vp-c-text-2);
  font-size: 12.5px;
  line-height: 1.4;
}

.session-pipeline__log em {
  margin-top: 10px;
  color: color-mix(in srgb, var(--amber) 75%, var(--vp-c-text-1));
  font-size: 11.5px;
  font-style: normal;
  font-weight: 600;
  letter-spacing: 0.04em;
  text-transform: uppercase;
}

.session-pipeline__stack {
  display: flex;
  flex-direction: column;
  gap: 4px;
  width: 100%;
  max-width: 220px;
  margin: 14px 0 0;
  padding: 0;
  list-style: none;
}

.session-pipeline__stack li {
  --tone: var(--amber);
  height: 7px;
  margin: 0;
  border-radius: 999px;
  background: color-mix(in srgb, var(--tone) 70%, transparent);
}

.session-pipeline__stack li:nth-child(odd) {
  width: 88%;
}

.session-pipeline__stack li:nth-child(3n) {
  width: 72%;
}

.session-pipeline__stack .is-tool {
  --tone: #6f4cff;
}

.session-pipeline__stack .is-file {
  --tone: #14b8a6;
}

.session-pipeline__stack .is-artifact {
  --tone: #2563eb;
}

.session-pipeline__stack .is-checkpoint {
  --tone: #10b981;
}

.session-pipeline__stack .is-new {
  width: 60%;
  background: var(--amber);
  box-shadow: 0 0 12px var(--amber);
  animation: session-pipeline-append 2.4s ease-out infinite;
}

.session-pipeline__art {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 14px;
  border-radius: 18px;
  background: var(--vp-c-bg-soft);
}

.session-pipeline__freshness {
  margin-top: 28px;
  padding: 18px 20px;
  border-radius: 20px;
  background: var(--vp-c-bg-soft);
}

.session-pipeline__freshness-title {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  color: var(--vp-c-text-1);
  font-size: 14px;
  font-weight: 600;
}

.session-pipeline__freshness ul {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(min(100%, 200px), 1fr));
  gap: 10px;
  margin: 12px 0 0;
  padding: 0;
  list-style: none;
}

.session-pipeline__freshness li {
  --tone: #10b981;
  display: flex;
  flex-direction: column;
  gap: 6px;
  margin: 0;
  padding: 10px 12px;
  border-radius: 10px;
  background: var(--vp-c-bg);
}

.session-pipeline__freshness li.is-wait {
  --tone: var(--amber);
}

.session-pipeline__freshness li.is-unknown {
  --tone: #94a3b8;
}

.session-pipeline__freshness li strong {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  width: fit-content;
  padding: 2px 10px 2px 8px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--tone) 14%, transparent);
  color: color-mix(in srgb, var(--tone) 85%, var(--vp-c-text-1));
  font-size: 12.5px;
  font-weight: 600;
  white-space: nowrap;
}

.session-pipeline__freshness li strong::before {
  content: "";
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--tone);
}

.session-pipeline__freshness li span {
  color: var(--vp-c-text-2);
  font-size: 12.5px;
  line-height: 1.4;
}

.session-pipeline__freshness p {
  margin: 12px 0 0;
  color: var(--vp-c-text-2);
  font-size: 13.5px;
  line-height: 1.5;
}

@keyframes session-pipeline-append {
  0% {
    opacity: 0;
    transform: translateY(8px);
  }
  30%,
  100% {
    opacity: 1;
    transform: translateY(0);
  }
}

@media (prefers-reduced-motion: reduce) {
  .session-pipeline__stack .is-new {
    animation: none;
  }
}

@media (max-width: 860px) {
  .session-pipeline {
    padding: 20px 14px;
    border-radius: 20px;
  }

  .session-pipeline__diagram {
    grid-template-areas: "write" "a1" "log" "a2" "read" "art";
    grid-template-columns: minmax(0, 1fr);
    grid-template-rows: auto 64px auto 64px auto auto;
  }

  .session-pipeline__link {
    flex-direction: row;
    align-self: stretch;
    justify-content: center;
    gap: 10px;
  }

  .session-pipeline__link .session-pipeline__label {
    width: 40%;
    text-align: right;
  }

  .session-pipeline__link .session-pipeline__chip {
    width: 40%;
    background: none;
    padding: 0;
  }

  .session-pipeline__link .session-pipeline__line {
    flex: none;
    width: 2px;
    height: 100%;
  }

  .session-pipeline__link .session-pipeline__line::after {
    display: none;
  }

  .session-pipeline__vlink {
    display: none;
  }

  .session-pipeline__art {
    margin-top: 16px;
  }
}
</style>
