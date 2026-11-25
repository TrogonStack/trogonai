<script setup lang="ts">
import Icon from "./Icon.vue";

const stages = [
  {
    icon: "badge-check",
    name: "Caller identity",
    text: "The caller's signed token is checked. A request that names no caller never gets further.",
  },
  {
    icon: "network",
    name: "Relationships",
    text: "Is this caller related to this agent?",
    optional: true,
  },
  { icon: "list-checks", name: "Rules", text: "Declarative allow and deny lists." },
  { icon: "filter", name: "Conditions", text: "CEL expressions over the request." },
  { icon: "eye-off", name: "Redaction", text: "A sandboxed module strips what the agent must not see." },
];
</script>

<template>
  <figure class="agent-gateway">
    <div class="agent-gateway__edge">
      <span class="agent-gateway__icon"><Icon name="key-round" :size="16" /></span>
      <span class="agent-gateway__edge-text">
        <span class="agent-gateway__eyebrow">At the public endpoint</span>
        <strong>Proof of possession</strong>
        <span>Callers prove they hold their key on every request.</span>
      </span>
      <em class="agent-gateway__chip">Built</em>
    </div>

    <div class="agent-gateway__frame">
      <span class="agent-gateway__label">
        <Icon name="shield-check" :size="14" /> A2A gateway
        <em class="agent-gateway__chip">Built</em>
      </span>
      <ol class="agent-gateway__stages">
        <li v-for="stage in stages" :key="stage.name">
          <span class="agent-gateway__icon"><Icon :name="stage.icon" :size="16" /></span>
          <strong>
            {{ stage.name }}
            <small v-if="stage.optional">optional</small>
          </strong>
          <span>{{ stage.text }}</span>
          <i class="agent-gateway__drop" aria-hidden="true" />
        </li>
      </ol>
    </div>

    <div class="agent-gateway__agent">
      <span class="agent-gateway__agent-icon"><Icon name="bot" :size="20" /></span>
      <strong>Your agent</strong>
      <span>Sees only what passed every check.</span>
    </div>

    <div class="agent-gateway__rail">
      <Icon name="ban" :size="15" />
      <span>Refused with a clear error, and audited</span>
    </div>

    <p class="agent-gateway__foot">
      Every decision, allowed or refused, is written to the audit stream. The principal is fixed by the subject the
      request arrived on, so a header cannot re-attribute it.
    </p>
  </figure>
</template>

<style scoped>
.agent-gateway {
  --teal: #14b8a6;
  --rose: #e11d48;
  --stage-gap: 22px;
  --rail-gap: 22px;
  display: grid;
  grid-template-columns: minmax(0, 1fr) 150px;
  grid-template-areas:
    "edge ."
    "frame agent"
    "rail ."
    "foot foot";
  column-gap: 36px;
  margin: 24px 0 0;
}

.agent-gateway__edge {
  position: relative;
  grid-area: edge;
  display: flex;
  align-items: center;
  gap: 12px;
  justify-self: start;
  margin-bottom: 34px;
  padding: 12px 16px;
  border: 1px dashed var(--vp-c-divider);
  border-radius: 18px;
  background: color-mix(in srgb, var(--vp-c-bg) 86%, transparent);
}

.agent-gateway__edge::after {
  content: "";
  position: absolute;
  top: 100%;
  left: 46px;
  width: 2px;
  height: 34px;
  background: linear-gradient(180deg, var(--vp-c-divider), color-mix(in srgb, var(--teal) 70%, transparent));
}

.agent-gateway__edge-text {
  display: flex;
  flex-direction: column;
  gap: 1px;
}

.agent-gateway__eyebrow {
  color: var(--vp-c-text-3);
  font-size: 11.5px;
  font-weight: 600;
  letter-spacing: 0.02em;
}

.agent-gateway__edge strong {
  color: var(--vp-c-text-1);
  font-size: 14px;
}

.agent-gateway__edge-text > span:last-child {
  color: var(--vp-c-text-2);
  font-size: 12.5px;
  line-height: 1.4;
}

.agent-gateway__icon {
  display: grid;
  flex: none;
  place-items: center;
  width: 32px;
  height: 32px;
  border-radius: 10px;
  background: color-mix(in srgb, var(--teal) 16%, transparent);
  color: color-mix(in srgb, var(--teal) 85%, var(--vp-c-text-1));
}

.agent-gateway__chip {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 1px 9px 1px 7px;
  border-radius: 999px;
  background: color-mix(in srgb, #10b981 14%, transparent);
  color: color-mix(in srgb, #10b981 85%, var(--vp-c-text-1));
  font-size: 11.5px;
  font-style: normal;
  font-weight: 600;
  white-space: nowrap;
}

.agent-gateway__chip::before {
  content: "";
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: #10b981;
}

.agent-gateway__frame {
  position: relative;
  grid-area: frame;
  padding: 46px 14px 16px;
  border: 1px solid color-mix(in srgb, var(--teal) 40%, var(--vp-c-divider));
  border-radius: 24px;
  background: color-mix(in srgb, var(--teal) 5%, var(--vp-c-bg));
}

.agent-gateway__frame::after {
  content: "";
  position: absolute;
  top: 50%;
  right: -37px;
  width: 30px;
  height: 2px;
  background: color-mix(in srgb, var(--teal) 70%, transparent);
}

.agent-gateway__frame::before {
  content: "";
  position: absolute;
  top: calc(50% - 5px);
  right: -37px;
  border: 6px solid transparent;
  border-left: 8px solid color-mix(in srgb, var(--teal) 80%, transparent);
  border-right: 0;
}

.agent-gateway__label {
  position: absolute;
  top: 12px;
  left: 18px;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  color: color-mix(in srgb, var(--teal) 80%, var(--vp-c-text-1));
  font-size: 12.5px;
  font-weight: 600;
}

.agent-gateway__label .agent-gateway__chip {
  margin-left: 4px;
}

.agent-gateway__stages {
  display: grid;
  grid-template-columns: repeat(5, minmax(0, 1fr));
  gap: var(--stage-gap);
  margin: 0;
  padding: 0;
  list-style: none;
}

.agent-gateway__stages li {
  position: relative;
  display: flex;
  flex-direction: column;
  gap: 6px;
  margin: 0;
  padding: 12px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 16px;
  background: var(--vp-c-bg);
}

.agent-gateway__stages li + li::before {
  content: "";
  position: absolute;
  top: 27px;
  left: calc(var(--stage-gap) * -1 + 6px);
  width: 6px;
  height: 6px;
  border-top: 2px solid color-mix(in srgb, var(--teal) 75%, transparent);
  border-right: 2px solid color-mix(in srgb, var(--teal) 75%, transparent);
  transform: rotate(45deg);
}

.agent-gateway__stages strong {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 4px 6px;
  margin-top: 2px;
  color: var(--vp-c-text-1);
  font-size: 13.5px;
  line-height: 1.3;
}

.agent-gateway__stages small {
  padding: 0 7px;
  border-radius: 999px;
  background: var(--vp-c-default-soft);
  color: var(--vp-c-text-2);
  font-size: 10.5px;
  font-weight: 600;
}

.agent-gateway__stages li > span:last-of-type {
  color: var(--vp-c-text-2);
  font-size: 12.5px;
  line-height: 1.45;
}

.agent-gateway__drop {
  position: absolute;
  top: 100%;
  left: 50%;
  width: 0;
  height: calc(16px + 1px + var(--rail-gap));
  border-left: 2px dashed color-mix(in srgb, var(--rose) 40%, transparent);
}

.agent-gateway__drop::after {
  content: "";
  position: absolute;
  bottom: -3px;
  left: -5px;
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: color-mix(in srgb, var(--rose) 60%, transparent);
}

.agent-gateway__agent {
  grid-area: agent;
  display: flex;
  flex-direction: column;
  align-self: center;
  gap: 6px;
  padding: 16px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 22px;
  background: color-mix(in srgb, var(--vp-c-bg) 90%, transparent);
  box-shadow: 0 30px 70px -40px rgba(36, 28, 90, 0.45);
}

.agent-gateway__agent-icon {
  display: grid;
  place-items: center;
  width: 40px;
  height: 40px;
  border-radius: 13px;
  background: color-mix(in srgb, var(--teal) 18%, var(--vp-c-bg));
  color: color-mix(in srgb, var(--teal) 85%, var(--vp-c-text-1));
}

.agent-gateway__agent strong {
  color: var(--vp-c-text-1);
  font-size: 14.5px;
}

.agent-gateway__agent > span:last-child {
  color: var(--vp-c-text-2);
  font-size: 12.5px;
  line-height: 1.45;
}

.agent-gateway__rail {
  grid-area: rail;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  margin-top: var(--rail-gap);
  padding: 9px 14px;
  border: 1px solid color-mix(in srgb, var(--rose) 30%, transparent);
  border-radius: 14px;
  background: color-mix(in srgb, var(--rose) 8%, transparent);
  color: color-mix(in srgb, var(--rose) 80%, var(--vp-c-text-1));
  font-size: 13px;
  font-weight: 600;
}

.agent-gateway__foot {
  grid-area: foot;
  margin: 14px 0 0;
  color: var(--vp-c-text-2);
  font-size: 13px;
  line-height: 1.5;
}

@media (max-width: 860px) {
  .agent-gateway {
    grid-template-columns: minmax(0, 1fr);
    grid-template-areas:
      "edge"
      "frame"
      "agent"
      "rail"
      "foot";
  }

  .agent-gateway__edge {
    justify-self: stretch;
    flex-wrap: wrap;
    margin-bottom: 28px;
  }

  .agent-gateway__edge::after {
    left: 50%;
    height: 28px;
  }

  .agent-gateway__stages {
    grid-template-columns: minmax(0, 1fr);
    gap: 18px;
  }

  .agent-gateway__stages li + li::before {
    top: -14px;
    left: 50%;
    transform: translateX(-4px) rotate(135deg);
  }

  .agent-gateway__drop {
    display: none;
  }

  .agent-gateway__frame::before,
  .agent-gateway__frame::after {
    top: auto;
    right: auto;
    left: 50%;
  }

  .agent-gateway__frame::after {
    bottom: -28px;
    width: 2px;
    height: 22px;
    transform: translateX(-1px);
  }

  .agent-gateway__frame::before {
    bottom: -29px;
    border: 6px solid transparent;
    border-top: 8px solid color-mix(in srgb, var(--teal) 80%, transparent);
    border-bottom: 0;
    transform: translateX(-6px);
  }

  .agent-gateway__agent {
    align-self: stretch;
    margin-top: 28px;
  }

  .agent-gateway__rail {
    margin-top: 14px;
  }
}
</style>
