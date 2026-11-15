<script setup lang="ts">
import Icon from "./Icon.vue";

const before = ["Requested", "Approved", "Started"];

const resolutions = [
  { icon: "circle-check", tone: "go", when: "It is confirmed to have run", then: "Completed" },
  { icon: "x", tone: "stop", when: "It is confirmed not to have run", then: "Failed" },
  { icon: "circle-help", tone: "wait", when: "Nobody can tell yet", then: "Unknown, until the truth is learned" },
];

const outcomes = [
  { name: "Committed", tone: "go", text: "It is in the log." },
  { name: "Deduplicated", tone: "go", text: "A retry found it already there. Nothing is doubled." },
  { name: "Not appended", tone: "stop", text: "Refused before it was sent. Safe to retry." },
  { name: "Conflict", tone: "stop", text: "Someone else wrote first. Re-read and decide again." },
  { name: "Unknown", tone: "wait", text: "Sent but not confirmed. A retry commits it or finds the copy." },
];
</script>

<template>
  <figure class="session-recovery">
    <section class="session-recovery__panel">
      <h3><Icon name="zap" :size="16" /> A runner dies mid tool call</h3>

      <ol class="session-recovery__track">
        <li v-for="step in before" :key="step" class="is-done"><Icon name="check" :size="12" /> {{ step }}</li>
        <li class="is-crash"><Icon name="zap" :size="12" /> Crash</li>
      </ol>

      <div class="session-recovery__verdict">
        <span class="session-recovery__wrong">Failed</span>
        <span class="session-recovery__right"><Icon name="circle-help" :size="16" /> Interrupted, not failed</span>
      </div>
      <p class="session-recovery__why">
        The call started and nothing settled it, so the session says exactly that. Calling it failed would claim
        something nobody knows.
      </p>

      <ul class="session-recovery__resolve">
        <li v-for="item in resolutions" :key="item.then" :class="`is-${item.tone}`">
          <span class="session-recovery__resolve-icon"><Icon :name="item.icon" :size="15" /></span>
          <span>
            <span>{{ item.when }}</span>
            <strong>{{ item.then }}</strong>
          </span>
        </li>
      </ul>
    </section>

    <section class="session-recovery__panel">
      <h3><Icon name="copy" :size="16" /> Every write gets a straight answer</h3>
      <ul class="session-recovery__outcomes">
        <li v-for="item in outcomes" :key="item.name" :class="`is-${item.tone}`">
          <strong>{{ item.name }}</strong>
          <span>{{ item.text }}</span>
        </li>
      </ul>
      <p class="session-recovery__why">
        No write ends as "best effort". Every event has a stable identity, so a retried write is recognized instead of
        recorded twice.
      </p>
    </section>
  </figure>
</template>

<style scoped>
.session-recovery {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 16px;
  margin: 24px 0 0;
}

.session-recovery__panel {
  padding: 20px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 24px;
  background: color-mix(in srgb, var(--vp-c-bg) 86%, transparent);
}

.session-recovery__panel h3 {
  display: flex;
  align-items: center;
  gap: 8px;
  margin: 0 0 16px;
  padding: 0;
  border: none;
  color: var(--vp-c-text-1);
  font-size: 15px;
  font-weight: 600;
  letter-spacing: normal;
}

.session-recovery__track {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px;
  margin: 0;
  padding: 0;
  list-style: none;
}

.session-recovery__track li {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  margin: 0;
  padding: 4px 10px;
  border-radius: 999px;
  font-size: 12px;
  font-weight: 600;
}

.session-recovery__track li + li::before {
  content: "";
  width: 10px;
  height: 2px;
  margin-left: -14px;
  margin-right: 4px;
  background: var(--vp-c-divider);
}

.session-recovery__track .is-done {
  background: color-mix(in srgb, #6f4cff 14%, transparent);
  color: color-mix(in srgb, #6f4cff 80%, var(--vp-c-text-1));
}

.session-recovery__track .is-crash {
  background: color-mix(in srgb, #e11d48 14%, transparent);
  color: #e11d48;
}

.session-recovery__verdict {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 10px;
  margin-top: 18px;
}

.session-recovery__wrong {
  padding: 4px 12px;
  border-radius: 10px;
  background: var(--vp-c-bg-soft);
  color: var(--vp-c-text-3);
  font-size: 13px;
  font-weight: 600;
  text-decoration: line-through;
  text-decoration-thickness: 2px;
}

.session-recovery__right {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 6px 14px;
  border-radius: 12px;
  background: color-mix(in srgb, #f59e0b 16%, transparent);
  color: color-mix(in srgb, #f59e0b 80%, var(--vp-c-text-1));
  font-size: 14px;
  font-weight: 700;
}

.session-recovery__why {
  margin: 12px 0 0;
  color: var(--vp-c-text-2);
  font-size: 13.5px;
  line-height: 1.5;
}

.session-recovery__resolve,
.session-recovery__outcomes {
  display: flex;
  flex-direction: column;
  gap: 8px;
  margin: 16px 0 0;
  padding: 0;
  list-style: none;
}

.session-recovery__resolve li,
.session-recovery__outcomes li {
  --tone: #10b981;
  margin: 0;
  border-radius: 12px;
  background: var(--vp-c-bg-soft);
}

.session-recovery__resolve li.is-stop,
.session-recovery__outcomes li.is-stop {
  --tone: #e11d48;
}

.session-recovery__resolve li.is-wait,
.session-recovery__outcomes li.is-wait {
  --tone: #f59e0b;
}

.session-recovery__resolve li {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 12px;
}

.session-recovery__resolve-icon {
  display: grid;
  flex: none;
  place-items: center;
  width: 28px;
  height: 28px;
  border-radius: 9px;
  background: color-mix(in srgb, var(--tone) 16%, transparent);
  color: var(--tone);
}

.session-recovery__resolve li span span {
  display: block;
  color: var(--vp-c-text-2);
  font-size: 12px;
  line-height: 1.3;
}

.session-recovery__resolve strong {
  color: var(--vp-c-text-1);
  font-size: 13.5px;
}

.session-recovery__outcomes li {
  display: grid;
  grid-template-columns: 130px minmax(0, 1fr);
  align-items: center;
  gap: 10px;
  padding: 9px 12px;
}

.session-recovery__outcomes strong {
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

.session-recovery__outcomes strong::before {
  content: "";
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--tone);
}

.session-recovery__outcomes span {
  color: var(--vp-c-text-2);
  font-size: 13px;
  line-height: 1.45;
}

@media (max-width: 860px) {
  .session-recovery {
    grid-template-columns: minmax(0, 1fr);
  }
}

@media (max-width: 560px) {
  .session-recovery__outcomes li {
    grid-template-columns: minmax(0, 1fr);
    gap: 6px;
  }
}
</style>
