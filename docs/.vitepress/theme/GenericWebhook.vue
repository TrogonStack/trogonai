<script setup lang="ts">
import { computed, ref } from "vue";
import Icon from "./Icon.vue";

type Authentication = "hmac" | "bearer";
type Scenario = "delivery" | "retry" | "refused";

const authentication = ref<Authentication>("hmac");
const scenario = ref<Scenario>("delivery");
const body = '{"event":"order.created","order_id":"order-1042"}';

const scenarios: ReadonlyArray<{ id: Scenario; label: string }> = [
  { id: "delivery", label: "Valid delivery" },
  { id: "retry", label: "Same-delivery retry" },
  { id: "refused", label: "Failed authentication" },
];

const refused = computed(() => scenario.value === "refused");
const retried = computed(() => scenario.value === "retry");
const outcome = computed(() => {
  if (refused.value) {
    return {
      code: "401 Unauthorized",
      title: "Refused before publication",
      detail: authentication.value === "hmac" ? "The signature does not verify." : "The bearer token does not match.",
    };
  }
  return retried.value
    ? {
        code: "202 Accepted",
        title: "Original event retained",
        detail: "Within the duplicate window, the same authenticated delivery adds no stored event.",
      }
    : {
        code: "202 Accepted",
        title: "One new event stored",
        detail: "The gateway acknowledges durable receipt. Your consumer performs the business action afterward.",
      };
});

const request = computed(() => {
  const headers = authentication.value === "hmac"
    ? [
        "webhook-timestamp: <current Unix seconds>",
        `webhook-signature: v1,<${refused.value ? "invalid signature" : "computed signature"}>`,
      ]
    : [`Authorization: Bearer <${refused.value ? "invalid token" : "sender credential"}>`];
  return [
    "POST /webhooks/{endpoint_id}",
    "Content-Type: application/json",
    "webhook-id: order-1042-created",
    ...headers,
    "",
    body,
  ].join("\n");
});

const stages = computed(() => [
  { icon: "send", title: "Your system", detail: "Sends the original body", inactive: false },
  {
    icon: "shield-check",
    title: "Gateway",
    detail: refused.value ? "Authentication fails" : "Checks the endpoint and credential",
    inactive: false,
  },
  {
    icon: "database",
    title: "Durable event",
    detail: refused.value ? "Nothing published" : retried.value ? "Original delivery kept" : "WebhookReceived stored",
    inactive: refused.value,
  },
  {
    icon: "workflow",
    title: "Your consumer",
    detail: refused.value ? "No new event" : retried.value ? "No additional event from this retry" : "Chooses and performs its action",
    inactive: refused.value,
  },
]);
</script>

<template>
  <figure class="generic-webhook" aria-label="Explore how a generic webhook is accepted, retried or refused">
    <div class="generic-webhook__toolbar">
      <span class="generic-webhook__eyebrow"><Icon name="webhook" :size="16" /> An order is created</span>
      <div class="generic-webhook__auth" role="group" aria-label="Request authentication">
        <button type="button" :aria-pressed="authentication === 'hmac'" @click="authentication = 'hmac'">
          Signed request
        </button>
        <button type="button" :aria-pressed="authentication === 'bearer'" @click="authentication = 'bearer'">
          Bearer token
        </button>
      </div>
    </div>

    <div class="generic-webhook__scenarios" role="group" aria-label="Delivery scenario">
      <button
        v-for="item in scenarios"
        :key="item.id"
        type="button"
        :aria-pressed="scenario === item.id"
        @click="scenario = item.id"
      >
        {{ item.label }}
      </button>
    </div>

    <ol class="generic-webhook__flow">
      <li
        v-for="(stage, index) in stages"
        :key="stage.title"
        :class="{ 'is-inactive': stage.inactive, 'is-refused': refused && index === 1 }"
      >
        <span class="generic-webhook__stage-icon"><Icon :name="stage.icon" :size="19" /></span>
        <strong>{{ stage.title }}</strong>
        <span>{{ stage.detail }}</span>
      </li>
    </ol>

    <div class="generic-webhook__result" :class="{ 'is-refused': refused }" aria-live="polite" aria-atomic="true">
      <span class="generic-webhook__response">{{ outcome.code }}</span>
      <strong>{{ outcome.title }}</strong>
      <p>{{ outcome.detail }}</p>
    </div>

    <div class="generic-webhook__example">
      <section>
        <h3>Incoming request</h3>
        <pre><code>{{ request }}</code></pre>
      </section>
      <section class="generic-webhook__receipt">
        <h3>{{ refused ? "No delivery event" : retried ? "The original receipt" : "What your consumer receives" }}</h3>
        <div v-if="refused" class="generic-webhook__empty">
          <Icon name="ban" :size="28" />
          <p>The request stops at the gateway. Nothing is published for a consumer to process.</p>
        </div>
        <dl v-else>
          <div><dt>Event</dt><dd><code>WebhookReceived</code></dd></div>
          <div><dt>Project</dt><dd>Orders</dd></div>
          <div><dt>Endpoint</dt><dd>The registered endpoint</dd></div>
          <div><dt>Delivery ID</dt><dd><code>order-1042-created</code></dd></div>
          <div><dt>Body</dt><dd>The exact original request bytes</dd></div>
          <div><dt>Auth headers</dt><dd>Not forwarded to the consumer</dd></div>
        </dl>
      </section>
    </div>

    <figcaption>Interactive illustration. No request is sent and no business action is performed.</figcaption>
  </figure>
</template>

<style scoped>
.generic-webhook {
  --brand: var(--vp-c-brand-1);
  --go: #10b981;
  --stop: #e11d48;
  display: flex;
  flex-direction: column;
  gap: 22px;
  margin: 24px 0;
  padding: 24px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 24px;
  background:
    radial-gradient(100% 40% at 100% 0%, color-mix(in srgb, var(--brand) 8%, transparent), transparent 70%),
    var(--vp-c-bg);
}

.generic-webhook__toolbar,
.generic-webhook__eyebrow,
.generic-webhook__auth,
.generic-webhook__scenarios {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
}

.generic-webhook__toolbar {
  justify-content: space-between;
  gap: 16px;
}

.generic-webhook__eyebrow {
  color: var(--vp-c-text-2);
  font-size: 13px;
  font-weight: 600;
}

.generic-webhook button {
  padding: 7px 11px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 9px;
  color: var(--vp-c-text-2);
  font: inherit;
  font-size: 12px;
  line-height: 1.5;
  cursor: pointer;
  transition: background-color 0.15s, border-color 0.15s, color 0.15s;
}

.generic-webhook button[aria-pressed="true"] {
  border-color: color-mix(in srgb, var(--brand) 45%, var(--vp-c-divider));
  background: color-mix(in srgb, var(--brand) 10%, var(--vp-c-bg));
  color: var(--vp-c-text-1);
}

.generic-webhook button:hover {
  border-color: var(--brand);
}

.generic-webhook button:focus-visible {
  outline: 2px solid var(--brand);
  outline-offset: 3px;
}

.generic-webhook__flow {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 24px;
  margin: 0;
  padding: 0;
  list-style: none;
}

.generic-webhook__flow li {
  position: relative;
  display: flex;
  flex-direction: column;
  gap: 7px;
  min-width: 0;
  margin: 0;
}

.generic-webhook__flow li:not(:last-child)::after {
  content: "→";
  position: absolute;
  top: 9px;
  right: -19px;
  color: var(--vp-c-text-3);
}

.generic-webhook__stage-icon {
  display: grid;
  place-items: center;
  width: 38px;
  height: 38px;
  border-radius: 12px;
  background: color-mix(in srgb, var(--brand) 10%, var(--vp-c-bg-soft));
  color: var(--brand);
}

.generic-webhook__flow strong {
  color: var(--vp-c-text-1);
  font-size: 14px;
}

.generic-webhook__flow li > span:last-child {
  color: var(--vp-c-text-2);
  font-size: 12px;
  line-height: 1.5;
}

.generic-webhook__flow .is-inactive .generic-webhook__stage-icon {
  background: var(--vp-c-bg-soft);
  color: var(--vp-c-text-3);
}

.generic-webhook__flow .is-refused .generic-webhook__stage-icon {
  background: color-mix(in srgb, var(--stop) 10%, var(--vp-c-bg-soft));
  color: var(--stop);
}

.generic-webhook__result {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  gap: 6px 12px;
  padding-top: 18px;
  border-top: 1px solid var(--vp-c-divider);
}

.generic-webhook__response {
  color: color-mix(in srgb, var(--go) 75%, var(--vp-c-text-1));
  font-family: var(--vp-font-family-mono);
  font-size: 13px;
  font-weight: 600;
}

.generic-webhook__result.is-refused .generic-webhook__response {
  color: color-mix(in srgb, var(--stop) 75%, var(--vp-c-text-1));
}

.generic-webhook__result strong {
  color: var(--vp-c-text-1);
  font-size: 13px;
}

.generic-webhook__result p {
  flex-basis: 100%;
  margin: 0;
  color: var(--vp-c-text-2);
  font-size: 13px;
  line-height: 1.5;
}

.generic-webhook__example {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 22px;
}

.generic-webhook__example section {
  min-width: 0;
}

.generic-webhook__example h3 {
  margin: 0 0 12px;
  padding: 0;
  border: none;
  color: var(--vp-c-text-2);
  font-size: 12px;
  font-weight: 600;
  line-height: 1.5;
}

.generic-webhook__example pre {
  margin: 0;
  padding: 16px;
  border-radius: 14px;
  background: var(--vp-c-bg-soft);
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.generic-webhook code {
  padding: 0;
  border: none;
  background: transparent;
  color: var(--vp-c-text-1);
  font-family: var(--vp-font-family-mono);
  font-size: 12px;
  line-height: 1.65;
  overflow-wrap: anywhere;
}

.generic-webhook__receipt dl {
  display: flex;
  flex-direction: column;
  gap: 10px;
  margin: 0;
}

.generic-webhook__receipt dl > div {
  display: grid;
  grid-template-columns: 80px minmax(0, 1fr);
  gap: 10px;
  font-size: 12px;
  line-height: 1.5;
}

.generic-webhook__receipt dt {
  color: var(--vp-c-text-3);
}

.generic-webhook__receipt dd {
  margin: 0;
  color: var(--vp-c-text-1);
  overflow-wrap: anywhere;
}

.generic-webhook__empty {
  display: flex;
  align-items: flex-start;
  gap: 12px;
  padding: 16px;
  border-radius: 14px;
  background: var(--vp-c-bg-soft);
  color: var(--vp-c-text-2);
}

.generic-webhook__empty p,
.generic-webhook figcaption {
  margin: 0;
  color: var(--vp-c-text-2);
  font-size: 12px;
  line-height: 1.5;
}

@media (max-width: 740px) {
  .generic-webhook {
    padding: 18px;
  }

  .generic-webhook__flow {
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 20px 24px;
  }

  .generic-webhook__flow li:nth-child(2)::after {
    content: none;
  }

  .generic-webhook__example {
    grid-template-columns: minmax(0, 1fr);
  }
}

@media (max-width: 420px) {
  .generic-webhook__auth,
  .generic-webhook__scenarios {
    align-items: stretch;
  }

  .generic-webhook button {
    min-height: 44px;
  }
}

@media (prefers-reduced-motion: reduce) {
  .generic-webhook button {
    transition: none;
  }
}
</style>
