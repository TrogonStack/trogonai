<script setup lang="ts">
import { withBase } from "vitepress";
import Icon from "./Icon.vue";

type Status = "built" | "partial" | "designed";

type Rail = { icon: string; name: string; text: string; status: Status; side: "left" | "right"; link?: boolean };

const rails: Rail[] = [
  {
    icon: "globe",
    name: "Network",
    text: "Reaches only the exact destinations its policy permits, through an mTLS proxy.",
    status: "built",
    side: "left",
  },
  {
    icon: "fingerprint",
    name: "Identity",
    text: "Proves which workload it is with attested certificates, not shared secrets.",
    status: "built",
    side: "left",
  },
  {
    icon: "scroll-text",
    name: "Audit",
    text: "Every decision is recorded. A permit that cannot be recorded is refused.",
    status: "built",
    side: "left",
  },
  {
    icon: "shield-check",
    name: "Policy",
    text: "Permits, ceilings and holds on what an agent may do. Policies can only narrow, never widen.",
    status: "partial",
    side: "right",
  },
  {
    icon: "key-round",
    name: "Credentials",
    text: "Uses connected accounts through a proxy that adds the credential. The agent never sees the key.",
    status: "designed",
    side: "right",
    link: true,
  },
  {
    icon: "cpu",
    name: "Model access",
    text: "Which models it may call, and how much it may spend.",
    status: "designed",
    side: "right",
  },
];

const statusLabel: Record<Status, string> = { built: "Built", partial: "Partial", designed: "Designed" };
</script>

<template>
  <figure class="agent-guardrails">
    <div class="agent-guardrails__core">
      <span class="agent-guardrails__rings" aria-hidden="true"><i /><i /><i /></span>
      <span class="agent-guardrails__core-icon"><Icon name="bot" :size="24" /></span>
      <strong>A running agent</strong>
      <span>Gets only what its policy grants</span>
    </div>

    <ul class="agent-guardrails__rails">
      <li
        v-for="(rail, index) in rails"
        :key="rail.name"
        :class="[`is-${rail.side}`, `is-${rail.status}`]"
        :style="{ '--row': (index % 3) + 1 }"
      >
        <span class="agent-guardrails__head">
          <span class="agent-guardrails__icon"><Icon :name="rail.icon" :size="16" /></span>
          <strong>{{ rail.name }}</strong>
          <em :class="['agent-guardrails__chip', `is-${rail.status}`]">{{ statusLabel[rail.status] }}</em>
        </span>
        <span class="agent-guardrails__text">
          {{ rail.text }}
          <template v-if="rail.link">
            See <a :href="withBase('/product/keys-and-secrets')">Keys &amp; Secrets</a>.
          </template>
        </span>
      </li>
    </ul>
  </figure>
</template>

<style scoped>
.agent-guardrails {
  --teal: #14b8a6;
  --gap: 40px;
  position: relative;
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(0, 0.8fr) minmax(0, 1fr);
  grid-template-rows: repeat(3, auto);
  column-gap: var(--gap);
  margin: 24px 0 0;
}

.agent-guardrails__rails {
  display: contents;
  margin: 0;
  padding: 0;
  list-style: none;
}

.agent-guardrails__rails li {
  --tone: #10b981;
  position: relative;
  grid-row: var(--row);
  display: flex;
  flex-direction: column;
  gap: 8px;
  margin: 6px 0;
  padding: 14px 16px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 20px;
  background: color-mix(in srgb, var(--vp-c-bg) 86%, transparent);
}

.agent-guardrails__rails li.is-left {
  grid-column: 1;
}

.agent-guardrails__rails li.is-right {
  grid-column: 3;
}

.agent-guardrails__rails li.is-partial {
  --tone: #2563eb;
}

.agent-guardrails__rails li.is-designed {
  --tone: #f59e0b;
  border-style: dashed;
}

.agent-guardrails__rails li::after {
  content: "";
  position: absolute;
  top: 50%;
  width: var(--gap);
  height: 0;
  border-top: 2px dashed color-mix(in srgb, var(--tone) 45%, var(--vp-c-divider));
}

.agent-guardrails__rails li.is-left::after {
  right: calc(var(--gap) * -1 - 1px);
}

.agent-guardrails__rails li.is-right::after {
  left: calc(var(--gap) * -1 - 1px);
}

.agent-guardrails__rails li.is-built::after {
  border-top-style: solid;
}

.agent-guardrails__head {
  display: flex;
  align-items: center;
  gap: 10px;
}

.agent-guardrails__icon {
  display: grid;
  flex: none;
  place-items: center;
  width: 32px;
  height: 32px;
  border-radius: 10px;
  background: color-mix(in srgb, var(--tone) 16%, transparent);
  color: color-mix(in srgb, var(--tone) 85%, var(--vp-c-text-1));
}

.agent-guardrails__head strong {
  flex: 1;
  color: var(--vp-c-text-1);
  font-size: 14.5px;
}

.agent-guardrails__chip {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 1px 9px 1px 7px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--tone) 14%, transparent);
  color: color-mix(in srgb, var(--tone) 85%, var(--vp-c-text-1));
  font-size: 11.5px;
  font-style: normal;
  font-weight: 600;
  white-space: nowrap;
}

.agent-guardrails__chip::before {
  content: "";
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--tone);
}

.agent-guardrails__text {
  color: var(--vp-c-text-2);
  font-size: 13px;
  line-height: 1.5;
}

.agent-guardrails__text a {
  color: var(--vp-c-brand-1);
  font-weight: 500;
}

.agent-guardrails__core {
  position: relative;
  grid-column: 2;
  grid-row: 1 / span 3;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 6px;
  overflow: hidden;
  margin: 6px 0;
  padding: 24px 16px;
  border: 1px solid color-mix(in srgb, var(--teal) 45%, var(--vp-c-divider));
  border-radius: 28px;
  background: color-mix(in srgb, var(--teal) 7%, var(--vp-c-bg));
  text-align: center;
}

.agent-guardrails__rings {
  position: absolute;
  inset: 0;
  display: grid;
  place-items: center;
  pointer-events: none;
}

.agent-guardrails__rings i {
  grid-area: 1 / 1;
  border: 1.5px dashed color-mix(in srgb, var(--teal) 35%, transparent);
  border-radius: 50%;
}

.agent-guardrails__rings i:nth-child(1) {
  width: 120px;
  height: 120px;
}

.agent-guardrails__rings i:nth-child(2) {
  width: 200px;
  height: 200px;
  opacity: 0.7;
}

.agent-guardrails__rings i:nth-child(3) {
  width: 290px;
  height: 290px;
  opacity: 0.45;
}

.agent-guardrails__core-icon {
  position: relative;
  display: grid;
  place-items: center;
  width: 56px;
  height: 56px;
  border-radius: 18px;
  background: color-mix(in srgb, var(--teal) 20%, var(--vp-c-bg));
  color: color-mix(in srgb, var(--teal) 85%, var(--vp-c-text-1));
  box-shadow: 0 20px 40px -24px rgba(20, 184, 166, 0.7);
}

.agent-guardrails__core strong {
  position: relative;
  margin-top: 6px;
  color: var(--vp-c-text-1);
  font-size: 15px;
}

.agent-guardrails__core > span:last-child {
  position: relative;
  color: var(--vp-c-text-2);
  font-size: 12.5px;
  line-height: 1.45;
}

@media (max-width: 860px) {
  .agent-guardrails {
    grid-template-columns: minmax(0, 1fr);
    grid-template-rows: none;
  }

  .agent-guardrails__core {
    grid-column: 1;
    grid-row: auto;
    min-height: 200px;
    margin-bottom: 8px;
  }

  .agent-guardrails__rails li,
  .agent-guardrails__rails li.is-left,
  .agent-guardrails__rails li.is-right {
    grid-column: 1;
    grid-row: auto;
  }

  .agent-guardrails__rails li::after {
    display: none;
  }
}
</style>
