<script setup lang="ts">
import Icon from "./Icon.vue";

type Status = "platform" | "designed";

const envelope = [
  { key: "subject", value: "session.sessions.events.<id>", note: "Which session it belongs to" },
  { key: "type", value: "ToolCallCompleted", note: "What happened" },
  { key: "event id", value: "stable, from the command", note: "A retry is recognized" },
  { key: "correlation", value: "the request it serves", note: "Follow one request end to end" },
  { key: "causation", value: "what led to it", note: "Walk the chain of cause" },
  { key: "trace", value: "W3C trace context", note: "Joins your traces" },
];

const subscribers = [
  { icon: "list", name: "Session list", text: "Recent sessions, titles and previews" },
  { icon: "play", name: "Resume index", text: "The session to pick up next" },
  { icon: "search", name: "Search", text: "Find a session by what happened in it" },
  { icon: "bell", name: "Your service", text: "Notify, export or index. It replays from the first event.", yours: true },
];

const pillars: {
  icon: string;
  tone: string;
  title: string;
  text: string;
  items: { text: string; status: Status }[];
}[] = [
  {
    icon: "plug",
    tone: "#6f4cff",
    title: "Pluggable",
    text: "New features read the log. They never ask the writer to change.",
    items: [
      { text: "A view catches up from its last checkpoint, or replays from the start", status: "platform" },
      { text: "Follow one session or every session with a subject filter", status: "platform" },
      { text: "Add a view later and it still sees the whole history", status: "designed" },
    ],
  },
  {
    icon: "activity",
    tone: "#14b8a6",
    title: "Observable",
    text: "The log already says what happened. Telemetry says how it went.",
    items: [
      { text: "Every command is traced with its outcome and its write guard", status: "platform" },
      { text: "Metrics for replays, snapshots and write conflicts", status: "platform" },
      { text: "Correlation, causation and trace context ride on every event", status: "designed" },
    ],
  },
  {
    icon: "flask-conical",
    tone: "#f59e0b",
    title: "Safe to change",
    text: "Behavior is plain logic over events, so it is easy to test and replace.",
    items: [
      { text: "Decisions are pure functions, tested by replaying events", status: "platform" },
      { text: "Deciders run as sandboxed WebAssembly, upgraded while work is in flight", status: "platform" },
      { text: "Rebuild any view, or see a session as it was at any point", status: "designed" },
    ],
  },
];

const statusLabel: Record<Status, string> = {
  platform: "In the platform",
  designed: "Designed",
};
</script>

<template>
  <figure class="session-events">
    <div class="session-events__flow">
      <section class="session-events__envelope">
        <header>
          <Icon name="zap" :size="15" />
          <span>One event, as it travels</span>
        </header>
        <dl>
          <div v-for="field in envelope" :key="field.key">
            <dt>{{ field.key }}</dt>
            <dd>
              <code>{{ field.value }}</code>
              <span>{{ field.note }}</span>
            </dd>
          </div>
        </dl>
      </section>

      <div class="session-events__fan" aria-hidden="true">
        <span class="session-events__fan-line" />
        <span class="session-events__chip">fan out</span>
      </div>

      <ul class="session-events__subs">
        <li v-for="sub in subscribers" :key="sub.name" :class="{ 'is-yours': sub.yours }">
          <span class="session-events__sub-icon"><Icon :name="sub.icon" :size="16" /></span>
          <span>
            <strong>{{ sub.name }}</strong>
            <span>{{ sub.text }}</span>
          </span>
        </li>
      </ul>
    </div>

    <div class="session-events__pillars">
      <section v-for="pillar in pillars" :key="pillar.title" :style="{ '--tone': pillar.tone }">
        <h3>
          <span class="session-events__pillar-icon"><Icon :name="pillar.icon" :size="17" /></span>
          {{ pillar.title }}
        </h3>
        <p>{{ pillar.text }}</p>
        <ul>
          <li v-for="item in pillar.items" :key="item.text">
            <span>{{ item.text }}</span>
            <em :class="`is-${item.status}`">{{ statusLabel[item.status] }}</em>
          </li>
        </ul>
      </section>
    </div>

    <figcaption>
      <strong>In the platform</strong> means the shared event-sourcing runtime ships today: deciders, snapshots,
      projections and their telemetry. The session service built on top of it is still designed.
    </figcaption>
  </figure>
</template>

<style scoped>
.session-events {
  margin: 24px 0 0;
}

.session-events__flow {
  display: grid;
  grid-template-columns: minmax(0, 1.25fr) 88px minmax(0, 1fr);
  align-items: center;
}

.session-events__envelope {
  overflow: hidden;
  border: 1px solid var(--vp-c-divider);
  border-radius: 22px;
  background: color-mix(in srgb, var(--vp-c-bg) 90%, transparent);
  box-shadow: 0 30px 70px -40px rgba(36, 28, 90, 0.45);
}

.session-events__envelope header {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 12px 18px;
  border-bottom: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg-soft);
  color: var(--vp-c-text-1);
  font-size: 13.5px;
  font-weight: 600;
}

.session-events__envelope header :deep(svg) {
  color: #f59e0b;
}

.session-events__envelope dl {
  margin: 0;
  padding: 8px 18px 12px;
}

.session-events__envelope dl > div {
  display: grid;
  grid-template-columns: 92px minmax(0, 1fr);
  gap: 12px;
  padding: 8px 0;
}

.session-events__envelope dl > div + div {
  border-top: 1px dashed var(--vp-c-divider);
}

.session-events__envelope dt {
  padding-top: 2px;
  color: var(--vp-c-text-3);
  font-family: var(--vp-font-family-mono);
  font-size: 12px;
}

.session-events__envelope dd {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  gap: 4px 10px;
  margin: 0;
  min-width: 0;
}

.session-events__envelope code {
  padding: 2px 7px;
  border-radius: 7px;
  background: var(--vp-c-bg-soft);
  color: var(--vp-c-text-1);
  font-size: 12px;
  overflow-wrap: anywhere;
}

.session-events__envelope dd span {
  color: var(--vp-c-text-2);
  font-size: 12.5px;
}

.session-events__fan {
  position: relative;
  display: grid;
  place-items: center;
  height: 100%;
  min-height: 60px;
}

.session-events__fan-line {
  position: absolute;
  top: 50%;
  left: 6px;
  right: 6px;
  height: 2px;
  background: linear-gradient(90deg, var(--vp-c-divider), #f59e0b);
}

.session-events__fan-line::after {
  content: "";
  position: absolute;
  top: -4px;
  right: -1px;
  border: 5px solid transparent;
  border-left: 7px solid #f59e0b;
  border-right: 0;
}

.session-events__chip {
  position: relative;
  padding: 2px 9px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 999px;
  background: var(--vp-c-bg);
  color: var(--vp-c-text-2);
  font-size: 11px;
  font-weight: 600;
  white-space: nowrap;
}

.session-events__subs {
  display: flex;
  flex-direction: column;
  gap: 10px;
  margin: 0;
  padding: 0;
  list-style: none;
}

.session-events__subs li {
  display: flex;
  align-items: center;
  gap: 12px;
  margin: 0;
  padding: 12px 14px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 16px;
  background: color-mix(in srgb, var(--vp-c-bg) 86%, transparent);
}

.session-events__subs li.is-yours {
  border-style: dashed;
  border-color: color-mix(in srgb, #6f4cff 50%, var(--vp-c-divider));
  background: color-mix(in srgb, #6f4cff 6%, transparent);
}

.session-events__sub-icon {
  display: grid;
  flex: none;
  place-items: center;
  width: 32px;
  height: 32px;
  border-radius: 10px;
  background: color-mix(in srgb, #f59e0b 16%, transparent);
  color: color-mix(in srgb, #f59e0b 85%, var(--vp-c-text-1));
}

.is-yours .session-events__sub-icon {
  background: color-mix(in srgb, #6f4cff 16%, transparent);
  color: #6f4cff;
}

.session-events__subs strong {
  display: block;
  color: var(--vp-c-text-1);
  font-size: 13.5px;
  line-height: 1.35;
}

.session-events__subs li span span {
  display: block;
  color: var(--vp-c-text-2);
  font-size: 12.5px;
  line-height: 1.4;
}

.session-events__pillars {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(min(100%, 240px), 1fr));
  gap: 14px;
  margin-top: 20px;
}

.session-events__pillars section {
  padding: 18px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 22px;
  background: color-mix(in srgb, var(--vp-c-bg) 86%, transparent);
}

.session-events__pillars h3 {
  display: flex;
  align-items: center;
  gap: 10px;
  margin: 0;
  padding: 0;
  border: none;
  color: var(--vp-c-text-1);
  font-size: 15px;
  font-weight: 600;
  letter-spacing: normal;
}

.session-events__pillar-icon {
  display: grid;
  place-items: center;
  width: 32px;
  height: 32px;
  border-radius: 10px;
  background: color-mix(in srgb, var(--tone) 16%, transparent);
  color: color-mix(in srgb, var(--tone) 85%, var(--vp-c-text-1));
}

.session-events__pillars p {
  margin: 10px 0 0;
  color: var(--vp-c-text-2);
  font-size: 13.5px;
  line-height: 1.5;
}

.session-events__pillars ul {
  display: flex;
  flex-direction: column;
  gap: 8px;
  margin: 14px 0 0;
  padding: 0;
  list-style: none;
}

.session-events__pillars li {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 6px;
  margin: 0;
  padding: 10px 12px;
  border-radius: 12px;
  background: var(--vp-c-bg-soft);
  color: var(--vp-c-text-1);
  font-size: 13px;
  line-height: 1.45;
}

.session-events__pillars em {
  --chip: #10b981;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 1px 9px 1px 7px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--chip) 14%, transparent);
  color: color-mix(in srgb, var(--chip) 85%, var(--vp-c-text-1));
  font-size: 11.5px;
  font-style: normal;
  font-weight: 600;
  white-space: nowrap;
}

.session-events__pillars em::before {
  content: "";
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--chip);
}

.session-events__pillars em.is-designed {
  --chip: #f59e0b;
}

.session-events figcaption {
  margin-top: 14px;
  color: var(--vp-c-text-2);
  font-size: 13px;
  line-height: 1.5;
}

.session-events figcaption strong {
  color: var(--vp-c-text-1);
}

@media (max-width: 860px) {
  .session-events__flow {
    grid-template-columns: minmax(0, 1fr);
  }

  .session-events__fan {
    height: 56px;
  }

  .session-events__fan-line {
    top: 6px;
    bottom: 6px;
    left: 50%;
    right: auto;
    width: 2px;
    height: auto;
    background: linear-gradient(180deg, var(--vp-c-divider), #f59e0b);
  }

  .session-events__fan-line::after {
    top: auto;
    right: auto;
    bottom: -1px;
    left: -4px;
    border: 5px solid transparent;
    border-top: 7px solid #f59e0b;
    border-bottom: 0;
  }
}

@media (max-width: 560px) {
  .session-events__envelope dl > div {
    grid-template-columns: minmax(0, 1fr);
    gap: 4px;
  }
}
</style>
