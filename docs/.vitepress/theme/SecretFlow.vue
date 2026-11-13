<script setup lang="ts">
import Icon from "./Icon.vue";

const steps = [
  {
    title: "The agent sends plain HTTP",
    text: "No token, no secret reference. The agent cannot pick the host or the credential.",
  },
  {
    title: "The supervisor signs it",
    text: "Outside the agent's code, the request is signed with a proof that lasts at most 60 seconds and works once.",
  },
  {
    title: "The proxy checks everything first",
    text: "Grant, permission, egress policy and budget are checked, and the audit record is stored, before any credential is touched.",
  },
  {
    title: "Custody hands over the credential",
    text: "The secrets service resolves the connection's credential and refreshes OAuth tokens. The proxy holds it in memory only for the call.",
  },
  {
    title: "The provider answers, scrubbed",
    text: "Cookies and credential-bearing redirects are stripped. If the credential echoes back, the call fails closed.",
  },
];
</script>

<template>
  <figure class="secret-flow">
    <div class="secret-flow__diagram">
      <div class="secret-flow__zone" aria-hidden="true">
        <span class="secret-flow__zone-label"><Icon name="lock" :size="13" /> Custody boundary</span>
      </div>

      <div class="secret-flow__node" style="grid-area: agent">
        <span class="secret-flow__badge is-agent"><Icon name="bot" :size="26" /></span>
        <strong>Agent</strong>
        <span>Never holds a credential</span>
      </div>

      <div class="secret-flow__link" style="grid-area: c1">
        <span class="secret-flow__label">plain HTTP</span>
        <span class="secret-flow__line"><span class="secret-flow__pulse" /></span>
        <span class="secret-flow__chip is-clean"><Icon name="eye-off" :size="12" /> no secret</span>
      </div>

      <div class="secret-flow__node" style="grid-area: sup">
        <span class="secret-flow__badge is-agent"><Icon name="signature" :size="26" /></span>
        <strong>Supervisor</strong>
        <span>Signs every request</span>
      </div>

      <div class="secret-flow__link" style="grid-area: c2">
        <span class="secret-flow__label">signed, 60 s</span>
        <span class="secret-flow__line"><span class="secret-flow__pulse" /></span>
        <span class="secret-flow__chip is-clean"><Icon name="eye-off" :size="12" /> no secret</span>
      </div>

      <div class="secret-flow__node" style="grid-area: proxy">
        <span class="secret-flow__badge is-custody"><Icon name="shield-check" :size="26" /></span>
        <strong>Connection proxy</strong>
        <span>Checks, then injects</span>
      </div>

      <div class="secret-flow__link is-vertical" style="grid-area: cv">
        <span class="secret-flow__line"><span class="secret-flow__pulse is-secret" /></span>
        <span class="secret-flow__chip is-secret"><Icon name="key-round" :size="12" /> credential</span>
      </div>

      <div class="secret-flow__node" style="grid-area: vault">
        <span class="secret-flow__badge is-custody"><Icon name="vault" :size="26" /></span>
        <strong>Secrets service</strong>
        <span>The only place it is decrypted</span>
      </div>

      <div class="secret-flow__link" style="grid-area: c3">
        <span class="secret-flow__label">HTTPS, no redirects</span>
        <span class="secret-flow__line is-secret"><span class="secret-flow__pulse is-secret" /></span>
        <span class="secret-flow__chip is-secret"><Icon name="key-round" :size="12" /> injected</span>
      </div>

      <div class="secret-flow__node" style="grid-area: provider">
        <span class="secret-flow__badge is-provider"><Icon name="globe" :size="26" /></span>
        <strong>Provider API</strong>
        <span>GitHub, Linear and more</span>
      </div>
    </div>

    <ol class="secret-flow__steps">
      <li v-for="step in steps" :key="step.title">
        <strong>{{ step.title }}</strong>
        <span>{{ step.text }}</span>
      </li>
    </ol>
  </figure>
</template>

<style scoped>
.secret-flow {
  --secret: #e11d48;
  --custody: #e11d48;
  --clean: #14b8a6;
  margin: 24px 0 0;
  padding: 32px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 28px;
  background: color-mix(in srgb, var(--vp-c-bg) 86%, transparent);
  box-shadow: 0 24px 60px -32px rgba(36, 28, 90, 0.28);
}

.secret-flow__diagram {
  position: relative;
  display: grid;
  grid-template-areas:
    "agent c1 sup c2 proxy c3 provider"
    ". . . . cv . ."
    ". . . . vault . .";
  grid-template-columns: minmax(0, 1fr) minmax(72px, 0.8fr) minmax(0, 1fr) minmax(72px, 0.8fr) minmax(0, 1fr) minmax(
      72px,
      0.8fr
    ) minmax(0, 1fr);
  grid-template-rows: auto 64px auto;
  align-items: center;
  isolation: isolate;
}

.secret-flow__zone {
  position: relative;
  z-index: -1;
  grid-column: proxy-start / proxy-end;
  grid-row: proxy-start / vault-end;
  align-self: stretch;
  margin: -16px -8px -34px;
  border: 1.5px dashed color-mix(in srgb, var(--custody) 45%, transparent);
  border-radius: 22px;
  background: color-mix(in srgb, var(--custody) 6%, transparent);
}

.secret-flow__zone-label {
  position: absolute;
  bottom: 8px;
  left: 50%;
  display: inline-flex;
  align-items: center;
  gap: 5px;
  color: var(--custody);
  font-size: 11px;
  font-weight: 600;
  letter-spacing: 0.04em;
  text-transform: uppercase;
  white-space: nowrap;
  transform: translateX(-50%);
}

.secret-flow__node {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 2px;
  text-align: center;
}

.secret-flow__node strong {
  margin-top: 8px;
  color: var(--vp-c-text-1);
  font-size: 15px;
  line-height: 1.3;
}

.secret-flow__node span:last-child {
  color: var(--vp-c-text-2);
  font-size: 12.5px;
  line-height: 1.35;
}

.secret-flow__badge {
  display: grid;
  place-items: center;
  width: 60px;
  height: 60px;
  border-radius: 18px;
  color: #fff;
  box-shadow: 0 8px 20px -10px var(--badge);
  background:
    linear-gradient(160deg, rgba(255, 255, 255, 0.24), rgba(255, 255, 255, 0) 55%),
    var(--badge);
}

.secret-flow__badge.is-agent {
  --badge: #14b8a6;
}

.secret-flow__badge.is-custody {
  --badge: #e11d48;
}

.secret-flow__badge.is-provider {
  --badge: #6f4cff;
}

.secret-flow__link {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 6px;
  padding: 0 6px;
  align-self: start;
  margin-top: 14px;
}

.secret-flow__label {
  color: var(--vp-c-text-3);
  font-size: 11px;
  font-weight: 500;
  white-space: nowrap;
}

.secret-flow__line {
  position: relative;
  width: 100%;
  height: 2px;
  border-radius: 2px;
  background: color-mix(in srgb, var(--clean) 45%, var(--vp-c-divider));
}

.secret-flow__line::after {
  content: "";
  position: absolute;
  top: 50%;
  right: -2px;
  width: 7px;
  height: 7px;
  border-top: 2px solid currentColor;
  border-right: 2px solid currentColor;
  color: color-mix(in srgb, var(--clean) 60%, var(--vp-c-divider));
  transform: translateY(-50%) rotate(45deg);
}

.secret-flow__line.is-secret {
  background: color-mix(in srgb, var(--secret) 55%, transparent);
}

.secret-flow__line.is-secret::after {
  color: var(--secret);
}

.secret-flow__pulse {
  position: absolute;
  top: 50%;
  left: 0;
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--clean);
  box-shadow: 0 0 10px var(--clean);
  transform: translate(-50%, -50%);
  animation: secret-flow-travel 2.6s ease-in-out infinite;
}

.secret-flow__pulse.is-secret {
  background: var(--secret);
  box-shadow: 0 0 10px var(--secret);
  animation-delay: 1.3s;
}

.secret-flow__chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 8px;
  border-radius: 999px;
  font-size: 11px;
  font-weight: 600;
  white-space: nowrap;
}

.secret-flow__chip.is-clean {
  background: color-mix(in srgb, var(--clean) 14%, transparent);
  color: color-mix(in srgb, var(--clean) 80%, var(--vp-c-text-1));
}

.secret-flow__chip.is-secret {
  background: color-mix(in srgb, var(--secret) 14%, transparent);
  color: var(--secret);
}

.secret-flow__link.is-vertical {
  flex-direction: row;
  align-self: stretch;
  justify-content: center;
  margin: 0;
  padding: 12px 0 4px;
  gap: 10px;
}

.secret-flow__link.is-vertical .secret-flow__line {
  width: 2px;
  height: 100%;
}

.secret-flow__link.is-vertical .secret-flow__line::after {
  display: none;
}

.secret-flow__link.is-vertical .secret-flow__pulse {
  top: 0;
  left: 50%;
  animation-name: secret-flow-travel-vertical;
}

.secret-flow__steps {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(min(100%, 170px), 1fr));
  gap: 12px;
  margin: 56px 0 0;
  padding: 0;
  list-style: none;
  counter-reset: step;
}

.secret-flow__steps li {
  display: flex;
  flex-direction: column;
  gap: 4px;
  margin: 0;
  padding: 16px;
  border-radius: 16px;
  background: var(--vp-c-bg-soft);
  counter-increment: step;
}

.secret-flow__steps li::before {
  content: counter(step);
  display: grid;
  place-items: center;
  width: 24px;
  height: 24px;
  margin-bottom: 6px;
  border-radius: 50%;
  background: var(--vp-c-text-1);
  color: var(--vp-c-bg);
  font-size: 12px;
  font-weight: 700;
}

.secret-flow__steps strong {
  color: var(--vp-c-text-1);
  font-size: 14px;
  line-height: 1.35;
}

.secret-flow__steps span {
  color: var(--vp-c-text-2);
  font-size: 13px;
  line-height: 1.5;
}

@keyframes secret-flow-travel {
  0% {
    left: 0;
    opacity: 0;
  }
  15%,
  85% {
    opacity: 1;
  }
  100% {
    left: 100%;
    opacity: 0;
  }
}

@keyframes secret-flow-travel-vertical {
  0% {
    top: 0;
    opacity: 0;
  }
  15%,
  85% {
    opacity: 1;
  }
  100% {
    top: 100%;
    opacity: 0;
  }
}

@media (prefers-reduced-motion: reduce) {
  .secret-flow__pulse {
    display: none;
  }
}

@media (max-width: 860px) {
  .secret-flow {
    padding: 24px 16px;
    border-radius: 20px;
  }

  .secret-flow__diagram {
    grid-template-areas: "agent" "c1" "sup" "c2" "proxy" "cv" "vault" "c3" "provider";
    grid-template-columns: minmax(0, 1fr);
    grid-template-rows: auto 72px auto 100px auto 64px auto 72px auto;
  }

  .secret-flow__zone {
    margin: -30px 0 -14px;
  }

  .secret-flow__zone-label {
    top: 8px;
    bottom: auto;
  }

  .secret-flow__link,
  .secret-flow__link.is-vertical {
    flex-direction: row;
    align-self: stretch;
    justify-content: center;
    gap: 10px;
    margin: 0;
    padding: 0;
  }

  .secret-flow__link .secret-flow__line {
    order: 0;
    width: 2px;
    height: 100%;
  }

  .secret-flow__link .secret-flow__label {
    order: -1;
    width: 40%;
    text-align: right;
  }

  .secret-flow__link .secret-flow__chip {
    width: fit-content;
  }

  .secret-flow__line::after {
    display: none;
  }

  .secret-flow__pulse {
    top: 0;
    left: 50%;
    animation-name: secret-flow-travel-vertical;
  }

  .secret-flow__link.is-vertical::before {
    content: "";
    width: 40%;
  }

  .secret-flow__steps {
    margin-top: 48px;
  }
}
</style>
