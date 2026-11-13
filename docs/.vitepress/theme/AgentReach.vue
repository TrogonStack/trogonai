<script setup lang="ts">
import Icon from "./Icon.vue";

type Status = "built" | "planned";

const sources = [
  { icon: "code", name: "Your editor", tag: "ACP", text: "A stdio bridge and an HTTP and WebSocket server" },
  { icon: "message-square", name: "Chat apps", tag: "Channels", text: "Telegram today" },
  { icon: "bot", name: "Other agents", tag: "A2A", text: "HTTP, REST and streaming, with a published agent card" },
  { icon: "wrench", name: "Tools", tag: "MCP", text: "Over stdio and HTTP" },
];

const modes: { shape: "one" | "many" | "any"; name: string; text: string; status: Status; note?: string }[] = [
  { shape: "one", name: "Unicast", text: "Ask one agent by name.", status: "built" },
  {
    shape: "many",
    name: "Multicast",
    text: "Follow a task as it happens. Every subscriber sees its events.",
    status: "built",
    note: "Task events only",
  },
  {
    shape: "any",
    name: "Anycast",
    text: "Ask any available reviewer, and one instance takes the work.",
    status: "planned",
    note: "No design yet",
  },
];

const statusLabel: Record<Status, string> = { built: "Built", planned: "Planned" };
</script>

<template>
  <figure class="agent-reach">
    <div class="agent-reach__hub">
      <ul class="agent-reach__sources">
        <li v-for="source in sources" :key="source.name">
          <span class="agent-reach__icon"><Icon :name="source.icon" :size="16" /></span>
          <span class="agent-reach__text">
            <span class="agent-reach__title">
              <strong>{{ source.name }}</strong>
              <code>{{ source.tag }}</code>
            </span>
            <span>{{ source.text }}</span>
          </span>
          <em class="agent-reach__chip is-built">Built</em>
        </li>
      </ul>

      <div class="agent-reach__bus">
        <span class="agent-reach__bus-icon"><Icon name="network" :size="22" /></span>
        <strong>One message bus</strong>
        <code>NATS</code>
        <span>Every protocol is carried over it</span>
      </div>

      <div class="agent-reach__agent">
        <span class="agent-reach__agent-icon"><Icon name="bot" :size="20" /></span>
        <strong>Your agent</strong>
        <span>ACP for editors and chat, A2A for other agents, MCP for the tools it calls.</span>
      </div>
    </div>

    <section class="agent-reach__modes">
      <h3><Icon name="send" :size="16" /> Ways to send work</h3>
      <ul>
        <li v-for="mode in modes" :key="mode.name" :class="`is-${mode.status}`">
          <svg class="agent-reach__glyph" viewBox="0 0 220 76" aria-hidden="true">
            <template v-if="mode.shape === 'one'">
              <line x1="30" y1="38" x2="176" y2="38" class="is-line" />
              <path d="M170 32 L178 38 L170 44" class="is-head" />
              <circle cx="30" cy="38" r="8" class="is-sender" />
              <circle cx="190" cy="38" r="8" class="is-hit" />
              <text x="30" y="64" text-anchor="middle">you</text>
              <text x="190" y="64" text-anchor="middle">pr-reviewer</text>
            </template>
            <template v-else-if="mode.shape === 'many'">
              <line x1="30" y1="38" x2="88" y2="38" class="is-line" />
              <line x1="88" y1="38" x2="184" y2="14" class="is-line" />
              <line x1="88" y1="38" x2="184" y2="38" class="is-line" />
              <line x1="88" y1="38" x2="184" y2="62" class="is-line" />
              <circle cx="30" cy="38" r="8" class="is-sender" />
              <rect x="70" y="28" width="36" height="20" rx="10" class="is-hub" />
              <circle cx="192" cy="14" r="7" class="is-hit" />
              <circle cx="192" cy="38" r="7" class="is-hit" />
              <circle cx="192" cy="62" r="7" class="is-hit" />
              <text x="30" y="66" text-anchor="middle">task</text>
              <text x="88" y="66" text-anchor="middle">events</text>
            </template>
            <template v-else>
              <line x1="30" y1="38" x2="164" y2="38" class="is-line" />
              <path d="M158 32 L166 38 L158 44" class="is-head" />
              <rect x="150" y="4" width="62" height="68" rx="16" class="is-group" />
              <circle cx="30" cy="38" r="8" class="is-sender" />
              <circle cx="190" cy="18" r="6" class="is-muted" />
              <circle cx="180" cy="38" r="8" class="is-hit" />
              <circle cx="190" cy="58" r="6" class="is-muted" />
              <text x="30" y="64" text-anchor="middle">you</text>
              <text x="104" y="28" text-anchor="middle">reviewer</text>
            </template>
          </svg>
          <span class="agent-reach__mode-head">
            <strong>{{ mode.name }}</strong>
            <em :class="['agent-reach__chip', `is-${mode.status}`]">{{ statusLabel[mode.status] }}</em>
          </span>
          <span class="agent-reach__mode-text">{{ mode.text }}</span>
          <span v-if="mode.note" class="agent-reach__note">{{ mode.note }}</span>
        </li>
      </ul>
    </section>
  </figure>
</template>

<style scoped>
.agent-reach {
  --teal: #14b8a6;
  --gap: 48px;
  margin: 24px 0 0;
}

.agent-reach__hub {
  display: grid;
  grid-template-columns: minmax(0, 1.35fr) minmax(0, 0.75fr) minmax(0, 0.9fr);
  column-gap: var(--gap);
  align-items: stretch;
}

.agent-reach__sources {
  display: flex;
  flex-direction: column;
  gap: 10px;
  margin: 0;
  padding: 0;
  list-style: none;
}

.agent-reach__sources li {
  position: relative;
  display: flex;
  align-items: center;
  gap: 12px;
  margin: 0;
  padding: 12px 14px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 18px;
  background: color-mix(in srgb, var(--vp-c-bg) 86%, transparent);
}

.agent-reach__sources li::after {
  content: "";
  position: absolute;
  top: 50%;
  right: calc(var(--gap) * -1 - 1px);
  width: var(--gap);
  height: 2px;
  background: linear-gradient(90deg, var(--vp-c-divider), color-mix(in srgb, var(--teal) 70%, transparent));
}

.agent-reach__icon {
  display: grid;
  flex: none;
  place-items: center;
  width: 34px;
  height: 34px;
  border-radius: 11px;
  background: color-mix(in srgb, var(--teal) 16%, transparent);
  color: color-mix(in srgb, var(--teal) 85%, var(--vp-c-text-1));
}

.agent-reach__text {
  display: flex;
  flex: 1;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.agent-reach__title {
  display: inline-flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
}

.agent-reach__title strong {
  color: var(--vp-c-text-1);
  font-size: 14px;
  line-height: 1.35;
}

.agent-reach__title code {
  padding: 1px 7px;
  border-radius: 7px;
  background: var(--vp-c-bg-soft);
  color: var(--vp-c-text-2);
  font-size: 11.5px;
}

.agent-reach__text > span:last-child {
  color: var(--vp-c-text-2);
  font-size: 12.5px;
  line-height: 1.4;
}

.agent-reach__chip {
  --chip: #10b981;
  display: inline-flex;
  flex: none;
  align-items: center;
  gap: 6px;
  width: fit-content;
  padding: 1px 9px 1px 7px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--chip) 14%, transparent);
  color: color-mix(in srgb, var(--chip) 85%, var(--vp-c-text-1));
  font-size: 11.5px;
  font-style: normal;
  font-weight: 600;
  white-space: nowrap;
}

.agent-reach__chip::before {
  content: "";
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--chip);
}

.agent-reach__chip.is-planned {
  --chip: var(--vp-c-text-3);
  color: var(--vp-c-text-2);
  background: var(--vp-c-default-soft);
}

.agent-reach__bus {
  position: relative;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 6px;
  padding: 20px 16px;
  border: 1px solid color-mix(in srgb, var(--teal) 45%, var(--vp-c-divider));
  border-radius: 24px;
  background:
    repeating-linear-gradient(
      180deg,
      transparent 0 14px,
      color-mix(in srgb, var(--teal) 7%, transparent) 14px 15px
    ),
    color-mix(in srgb, var(--teal) 8%, var(--vp-c-bg));
  text-align: center;
}

.agent-reach__bus::after {
  content: "";
  position: absolute;
  top: 50%;
  right: calc(var(--gap) * -1 - 1px);
  width: calc(var(--gap) - 6px);
  height: 2px;
  background: color-mix(in srgb, var(--teal) 70%, transparent);
}

.agent-reach__bus::before {
  content: "";
  position: absolute;
  top: calc(50% - 5px);
  right: calc(var(--gap) * -1 + 1px);
  border: 6px solid transparent;
  border-left: 8px solid color-mix(in srgb, var(--teal) 80%, transparent);
  border-right: 0;
}

.agent-reach__bus-icon,
.agent-reach__agent-icon {
  display: grid;
  place-items: center;
  width: 46px;
  height: 46px;
  border-radius: 15px;
  background: color-mix(in srgb, var(--teal) 18%, var(--vp-c-bg));
  color: color-mix(in srgb, var(--teal) 85%, var(--vp-c-text-1));
}

.agent-reach__bus strong,
.agent-reach__agent strong {
  color: var(--vp-c-text-1);
  font-size: 15px;
}

.agent-reach__bus code {
  padding: 1px 9px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--teal) 16%, transparent);
  color: color-mix(in srgb, var(--teal) 80%, var(--vp-c-text-1));
  font-size: 11.5px;
  font-weight: 600;
}

.agent-reach__bus > span:last-child,
.agent-reach__agent > span:last-child {
  color: var(--vp-c-text-2);
  font-size: 12.5px;
  line-height: 1.45;
}

.agent-reach__agent {
  display: flex;
  flex-direction: column;
  align-self: center;
  gap: 8px;
  padding: 20px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 24px;
  background: color-mix(in srgb, var(--vp-c-bg) 90%, transparent);
  box-shadow: 0 30px 70px -40px rgba(36, 28, 90, 0.45);
}

.agent-reach__modes {
  margin-top: 20px;
  padding: 18px 20px 20px;
  border-radius: 24px;
  background: var(--vp-c-bg-soft);
}

.agent-reach__modes h3 {
  display: flex;
  align-items: center;
  gap: 8px;
  margin: 0;
  padding: 0;
  border: none;
  color: var(--vp-c-text-1);
  font-size: 15px;
  font-weight: 600;
  letter-spacing: normal;
}

.agent-reach__modes ul {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(min(100%, 230px), 1fr));
  gap: 12px;
  margin: 14px 0 0;
  padding: 0;
  list-style: none;
}

.agent-reach__modes li {
  display: flex;
  flex-direction: column;
  gap: 6px;
  margin: 0;
  padding: 14px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 20px;
  background: var(--vp-c-bg);
}

.agent-reach__modes li.is-planned {
  border-style: dashed;
}

.agent-reach__glyph {
  width: 100%;
  height: 76px;
  margin-bottom: 4px;
  border-radius: 14px;
  background: var(--vp-c-bg-soft);
}

.agent-reach__glyph text {
  fill: var(--vp-c-text-2);
  font-family: var(--vp-font-family-mono);
  font-size: 10.5px;
}

.agent-reach__glyph .is-line {
  stroke: color-mix(in srgb, var(--teal) 70%, transparent);
  stroke-width: 2;
  stroke-linecap: round;
}

.agent-reach__glyph .is-head {
  fill: none;
  stroke: color-mix(in srgb, var(--teal) 80%, transparent);
  stroke-width: 2;
  stroke-linecap: round;
  stroke-linejoin: round;
}

.agent-reach__glyph .is-sender {
  fill: var(--vp-c-bg);
  stroke: var(--teal);
  stroke-width: 3;
}

.agent-reach__glyph .is-hit {
  fill: var(--teal);
}

.agent-reach__glyph .is-muted {
  fill: var(--vp-c-text-3);
}

.agent-reach__glyph .is-hub {
  fill: color-mix(in srgb, var(--teal) 22%, var(--vp-c-bg));
  stroke: var(--teal);
  stroke-width: 1.5;
}

.agent-reach__glyph .is-group {
  fill: none;
  stroke: var(--vp-c-text-3);
  stroke-width: 1.5;
  stroke-dasharray: 4 4;
}

.is-planned .agent-reach__glyph .is-line,
.is-planned .agent-reach__glyph .is-head {
  stroke: var(--vp-c-text-3);
  stroke-dasharray: 5 5;
}

.is-planned .agent-reach__glyph .is-sender {
  stroke: var(--vp-c-text-3);
}

.is-planned .agent-reach__glyph .is-hit {
  fill: var(--vp-c-text-2);
}

.agent-reach__mode-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}

.agent-reach__mode-head strong {
  color: var(--vp-c-text-1);
  font-size: 14.5px;
}

.agent-reach__mode-text {
  color: var(--vp-c-text-2);
  font-size: 13px;
  line-height: 1.5;
}

.agent-reach__note {
  color: var(--vp-c-text-3);
  font-size: 12px;
}

@media (max-width: 860px) {
  .agent-reach {
    --gap: 32px;
  }

  .agent-reach__hub {
    grid-template-columns: minmax(0, 1fr);
    row-gap: var(--gap);
  }

  .agent-reach__sources li::after {
    display: none;
  }

  .agent-reach__bus::before,
  .agent-reach__bus::after {
    right: auto;
    left: 50%;
  }

  .agent-reach__bus::after {
    top: auto;
    bottom: calc(var(--gap) * -1 + 6px);
    width: 2px;
    height: calc(var(--gap) - 6px);
    transform: translateX(-1px);
  }

  .agent-reach__bus::before {
    top: auto;
    bottom: calc(var(--gap) * -1 - 1px);
    border: 6px solid transparent;
    border-top: 8px solid color-mix(in srgb, var(--teal) 80%, transparent);
    border-bottom: 0;
    transform: translateX(-6px);
  }

  .agent-reach__sources {
    position: relative;
  }

  .agent-reach__sources::after {
    content: "";
    position: absolute;
    top: 100%;
    left: 50%;
    width: 2px;
    height: calc(var(--gap) - 6px);
    background: linear-gradient(180deg, var(--vp-c-divider), color-mix(in srgb, var(--teal) 70%, transparent));
  }

  .agent-reach__agent {
    align-self: stretch;
  }
}

@media (max-width: 560px) {
  .agent-reach__sources li {
    flex-wrap: wrap;
  }

  .agent-reach__sources .agent-reach__chip {
    margin-left: 46px;
  }
}
</style>
