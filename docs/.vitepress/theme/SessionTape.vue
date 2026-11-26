<script setup lang="ts">
import Icon from "./Icon.vue";

type Kind = "message" | "tool" | "file" | "artifact" | "checkpoint";

type Entry = { kind: Kind; icon: string; label: string; detail: string; event: string };

const entries: Entry[] = [
  {
    kind: "message",
    icon: "user",
    label: "You",
    detail: "Make the login test stop flaking",
    event: "UserMessageRecorded",
  },
  {
    kind: "tool",
    icon: "wrench",
    label: "Tool call requested",
    detail: "Run the test suite",
    event: "ToolCallRequested",
  },
  { kind: "tool", icon: "circle-check", label: "Approved", detail: "You allowed it", event: "ToolCallApproved" },
  {
    kind: "tool",
    icon: "terminal",
    label: "Tool call completed",
    detail: "One failure, with the output kept for replay",
    event: "ToolCallCompleted",
  },
  { kind: "file", icon: "file-diff", label: "File changed", detail: "tests/login.test.ts", event: "FileChanged" },
  {
    kind: "artifact",
    icon: "layers",
    label: "Artifact recorded",
    detail: "A structured diff of the fix",
    event: "ArtifactRecorded",
  },
  {
    kind: "checkpoint",
    icon: "flag",
    label: "Checkpoint",
    detail: "A point a later run can restore from",
    event: "CheckpointProduced",
  },
  {
    kind: "message",
    icon: "sparkles",
    label: "Assistant reply",
    detail: "Fixed: the test raced a timer",
    event: "AssistantMessageCompleted",
  },
];

const legend: { kind: Kind; label: string }[] = [
  { kind: "message", label: "Messages" },
  { kind: "tool", label: "Tool calls" },
  { kind: "file", label: "Files" },
  { kind: "artifact", label: "Artifacts" },
  { kind: "checkpoint", label: "Checkpoints" },
];
</script>

<template>
  <figure class="session-tape" :style="{ '--rows': entries.length }">
    <div class="session-tape__window">
      <header class="session-tape__bar">
        <span class="session-tape__dots" aria-hidden="true"><i /><i /><i /></span>
        <span class="session-tape__title"><Icon name="history" :size="15" /> Fix the flaky login test</span>
        <span class="session-tape__tags">
          <span>Append-only</span>
          <span>Replayable</span>
        </span>
      </header>

      <ol class="session-tape__log">
        <li
          v-for="(entry, index) in entries"
          :key="entry.event"
          :class="['session-tape__row', `is-${entry.kind}`]"
          :style="{ '--i': index }"
        >
          <span class="session-tape__ordinal">{{ index + 1 }}</span>
          <span class="session-tape__icon"><Icon :name="entry.icon" :size="16" /></span>
          <span class="session-tape__text">
            <strong>{{ entry.label }}</strong>
            <span>{{ entry.detail }}</span>
          </span>
          <code class="session-tape__event">{{ entry.event }}</code>
        </li>
      </ol>

      <footer class="session-tape__foot">
        <span class="session-tape__replay"><Icon name="play" :size="13" /> Replaying from the first event</span>
        <span class="session-tape__legend">
          <span v-for="item in legend" :key="item.kind" :class="['session-tape__key', `is-${item.kind}`]">
            {{ item.label }}
          </span>
        </span>
      </footer>
    </div>
  </figure>
</template>

<style scoped>
.session-tape {
  --step: 1.1s;
  margin: 24px 0 0;
}

.session-tape__window {
  overflow: hidden;
  border: 1px solid var(--vp-c-divider);
  border-radius: 24px;
  background: color-mix(in srgb, var(--vp-c-bg) 90%, transparent);
  box-shadow: 0 30px 70px -40px rgba(36, 28, 90, 0.45);
}

.session-tape__bar {
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 12px 18px;
  border-bottom: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg-soft);
}

.session-tape__dots {
  display: inline-flex;
  gap: 6px;
}

.session-tape__dots i {
  width: 10px;
  height: 10px;
  border-radius: 50%;
  background: var(--vp-c-divider);
}

.session-tape__title {
  display: inline-flex;
  flex: 1;
  align-items: center;
  gap: 6px;
  min-width: 0;
  color: var(--vp-c-text-1);
  font-size: 13.5px;
  font-weight: 600;
  white-space: nowrap;
}

.session-tape__tags {
  display: inline-flex;
  gap: 6px;
}

.session-tape__tags span {
  padding: 2px 9px;
  border-radius: 999px;
  background: color-mix(in srgb, #f59e0b 14%, transparent);
  color: color-mix(in srgb, #f59e0b 75%, var(--vp-c-text-1));
  font-size: 11px;
  font-weight: 600;
  white-space: nowrap;
}

.session-tape__log {
  position: relative;
  margin: 0;
  padding: 10px 12px;
  list-style: none;
}

.session-tape__log::before {
  content: "";
  position: absolute;
  top: 26px;
  bottom: 26px;
  left: 78px;
  width: 2px;
  border-radius: 2px;
  background: var(--vp-c-divider);
}

.session-tape__row {
  --tone: #f59e0b;
  position: relative;
  display: grid;
  grid-template-columns: 28px 34px minmax(0, 1fr) auto;
  align-items: center;
  gap: 12px;
  margin: 0;
  padding: 8px 10px;
  border-radius: 14px;
  animation: session-tape-play calc(var(--step) * var(--rows)) ease-in-out infinite;
  animation-delay: calc(var(--step) * var(--i));
}

.session-tape__row.is-tool {
  --tone: #6f4cff;
}

.session-tape__row.is-file {
  --tone: #14b8a6;
}

.session-tape__row.is-artifact {
  --tone: #2563eb;
}

.session-tape__row.is-checkpoint {
  --tone: #10b981;
}

.session-tape__ordinal {
  color: var(--vp-c-text-3);
  font-family: var(--vp-font-family-mono);
  font-size: 11.5px;
  text-align: right;
}

.session-tape__icon {
  position: relative;
  z-index: 1;
  display: grid;
  place-items: center;
  width: 34px;
  height: 34px;
  border-radius: 11px;
  background: color-mix(in srgb, var(--tone) 16%, var(--vp-c-bg));
  color: var(--tone);
}

.session-tape__text {
  display: flex;
  flex-direction: column;
  min-width: 0;
}

.session-tape__text strong {
  color: var(--vp-c-text-1);
  font-size: 14px;
  line-height: 1.35;
}

.session-tape__text span {
  overflow: hidden;
  color: var(--vp-c-text-2);
  font-size: 13px;
  line-height: 1.4;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.session-tape__event {
  padding: 3px 8px;
  border-radius: 8px;
  background: var(--vp-c-bg-soft);
  color: var(--vp-c-text-2);
  font-size: 11.5px;
}

.session-tape__foot {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: space-between;
  gap: 10px 16px;
  padding: 12px 18px;
  border-top: 1px solid var(--vp-c-divider);
  font-size: 12px;
}

.session-tape__replay {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  color: color-mix(in srgb, #f59e0b 75%, var(--vp-c-text-1));
  font-weight: 600;
}

.session-tape__legend {
  display: inline-flex;
  flex-wrap: wrap;
  gap: 6px 14px;
  color: var(--vp-c-text-2);
}

.session-tape__key {
  --tone: #f59e0b;
  display: inline-flex;
  align-items: center;
  gap: 6px;
}

.session-tape__key::before {
  content: "";
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--tone);
}

.session-tape__key.is-tool {
  --tone: #6f4cff;
}

.session-tape__key.is-file {
  --tone: #14b8a6;
}

.session-tape__key.is-artifact {
  --tone: #2563eb;
}

.session-tape__key.is-checkpoint {
  --tone: #10b981;
}

@keyframes session-tape-play {
  0%,
  14%,
  100% {
    background: transparent;
    box-shadow: none;
  }
  3%,
  10% {
    background: color-mix(in srgb, var(--tone) 9%, transparent);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--tone) 40%, transparent);
  }
}

@media (prefers-reduced-motion: reduce) {
  .session-tape__row {
    animation: none;
  }
}

@media (max-width: 640px) {
  .session-tape__tags,
  .session-tape__event,
  .session-tape__ordinal {
    display: none;
  }

  .session-tape__row {
    grid-template-columns: 34px minmax(0, 1fr);
  }

  .session-tape__log::before {
    left: 38px;
  }
}
</style>
