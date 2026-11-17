<script setup lang="ts">
import BrandTile from "./BrandTile.vue";
import Icon from "./Icon.vue";
import { data } from "./connectors.data";

const SHOWN = ["GitHub", "Slack", "Linear", "Sentry", "Zoom", "GitLab"];

const providers = SHOWN.map((name) => data.find((connector) => connector.name === name)).filter(
  (connector): connector is NonNullable<typeof connector> => connector !== undefined,
);

const steps = [
  { icon: "shield-check", title: "Verify", text: "The provider's own check: a signature, a token or a signed key." },
  { icon: "copy", title: "Dedupe", text: "A delivery id becomes Nats-Msg-Id, so a retry is published once." },
  { icon: "route", title: "Route", text: "Published to {subject_prefix}.{event}, named for the account." },
];

const streams = [
  { name: "GITHUB_ACME-MAIN", subject: "github-acme-main.pull_request" },
  { name: "GITHUB_OTHER-ORG", subject: "github-other-org.issues" },
  { name: "ZOOM_PRIMARY", subject: "zoom-primary.meeting.ended" },
  { name: "SLACK_WORKSPACE", subject: "slack-workspace.message" },
];

const accounts = [
  { name: "acme-main", route: "/sources/github/acme-main/webhook", prefix: "github-acme-main", stream: "GITHUB_ACME-MAIN" },
  { name: "other-org", route: "/sources/github/other-org/webhook", prefix: "github-other-org", stream: "GITHUB_OTHER-ORG" },
];
</script>

<template>
  <figure class="ingress">
    <div class="ingress__flow">
      <section class="ingress__providers">
        <p class="ingress__label">Providers call one URL</p>
        <ul class="ingress__tiles">
          <li v-for="provider in providers" :key="provider.name">
            <BrandTile :name="provider.name" :color="provider.color" :icon-path="provider.iconPath" :size="44" />
            <span>{{ provider.name }}</span>
          </li>
        </ul>
        <code class="ingress__post">POST /sources/<wbr />github/<wbr />acme-main/<wbr />webhook</code>
      </section>

      <div class="ingress__arrow" aria-hidden="true">
        <span class="ingress__line"><span class="ingress__dot" /></span>
      </div>

      <section class="ingress__gateway">
        <header>
          <span class="ingress__gateway-icon"><Icon name="webhook" :size="18" /></span>
          <span>
            <strong>Gateway</strong>
            <code>/sources/<wbr />&lt;source&gt;/<wbr />&lt;integration&gt;/<wbr />webhook</code>
          </span>
        </header>
        <ol class="ingress__steps">
          <li v-for="(step, index) in steps" :key="step.title" :style="{ '--i': index }">
            <span class="ingress__step-icon"><Icon :name="step.icon" :size="15" /></span>
            <span>
              <strong>{{ step.title }}</strong>
              <span>{{ step.text }}</span>
            </span>
          </li>
        </ol>
        <footer>
          <div>
            <Icon name="server" :size="14" />
            <span>One process, one port, with health probes</span>
            <span class="ingress__probes"><code>/-/liveness</code> <code>/-/readiness</code></span>
          </div>
          <div>
            <Icon name="radio" :size="14" />
            <span>Discord and Google Pub/Sub hold long-lived connections in the same process instead of webhooks.</span>
          </div>
        </footer>
      </section>

      <div class="ingress__arrow" aria-hidden="true">
        <span class="ingress__line"><span class="ingress__dot is-late" /></span>
      </div>

      <section class="ingress__right">
        <div class="ingress__streams">
          <p class="ingress__label"><Icon name="database" :size="14" /> One stream per integration</p>
          <ul>
            <li v-for="stream in streams" :key="stream.name">
              <span class="ingress__stream-name">{{ stream.name }}</span>
              <code>{{ stream.subject }}</code>
            </li>
          </ul>
        </div>
        <div class="ingress__down" aria-hidden="true"><span /></div>
        <div class="ingress__agents">
          <span class="ingress__agents-icon"><Icon name="bot" :size="18" /></span>
          <span>
            <strong>Agents subscribe</strong>
            <span>One event, one account or a whole provider, such as <code class="ingress__nowrap">github-acme-main.&gt;</code></span>
          </span>
        </div>
      </section>
    </div>

    <div class="ingress__side">
      <section class="is-refused">
        <span class="ingress__side-icon"><Icon name="ban" :size="16" /></span>
        <span>
          <strong>Fails its check <em>401</em></strong>
          <span>Refused, never published. No agent ever sees it.</span>
        </span>
      </section>
      <section class="is-unroutable">
        <span class="ingress__side-icon"><Icon name="inbox" :size="16" /></span>
        <span>
          <strong>Verified, but cannot be routed</strong>
          <span>
            Published to <code>{subject_prefix}.unroutable</code> with the reason in a header and the original body
            kept. Nothing dropped silently.
          </span>
        </span>
      </section>
    </div>

    <section class="ingress__accounts">
      <p class="ingress__label"><Icon name="users" :size="14" /> Several accounts per provider</p>
      <div class="ingress__account-grid">
        <div v-for="account in accounts" :key="account.name" class="ingress__account">
          <span class="ingress__account-name">github · {{ account.name }}</span>
          <dl>
            <div>
              <dt>route</dt>
              <dd><code>{{ account.route }}</code></dd>
            </div>
            <div>
              <dt>prefix</dt>
              <dd><code>{{ account.prefix }}</code></dd>
            </div>
            <div>
              <dt>stream</dt>
              <dd><code>{{ account.stream }}</code></dd>
            </div>
          </dl>
        </div>
      </div>
    </section>
  </figure>
</template>

<style scoped>
.ingress {
  --brand: #6f4cff;
  --go: #10b981;
  --stop: #e11d48;
  --wait: #f59e0b;
  margin: 24px 0 0;
}

.ingress code {
  padding: 2px 7px;
  border-radius: 7px;
  background: var(--vp-c-bg-soft);
  color: var(--vp-c-text-1);
  font-size: 12px;
  overflow-wrap: break-word;
}

.ingress__label {
  display: flex;
  align-items: center;
  gap: 6px;
  margin: 0 0 12px;
  color: var(--vp-c-text-2);
  font-size: 12.5px;
  font-weight: 600;
  letter-spacing: 0.02em;
  text-transform: uppercase;
}

.ingress__flow {
  display: grid;
  grid-template-columns: minmax(0, 0.9fr) 56px minmax(0, 1.25fr) 56px minmax(0, 1fr);
  align-items: center;
}

/* Providers */
.ingress__providers {
  padding: 18px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 22px;
  background: color-mix(in srgb, var(--vp-c-bg) 86%, transparent);
}

.ingress__tiles {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 12px 8px;
  margin: 0;
  padding: 0;
  list-style: none;
}

.ingress__tiles li {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 6px;
  margin: 0;
  color: var(--vp-c-text-2);
  font-size: 11.5px;
}

.ingress__nowrap {
  white-space: nowrap;
}

.ingress__probes {
  display: inline-flex;
  flex-wrap: wrap;
  gap: 6px;
}

.ingress__post {
  display: block;
  margin-top: 14px;
  text-align: center;
}

/* Arrows */
.ingress__arrow {
  position: relative;
  display: grid;
  place-items: center;
  height: 100%;
  min-height: 48px;
}

.ingress__line {
  position: absolute;
  top: 50%;
  left: 6px;
  right: 6px;
  height: 2px;
  background: linear-gradient(90deg, var(--vp-c-divider), var(--brand));
}

.ingress__line::after {
  content: "";
  position: absolute;
  top: -4px;
  right: -1px;
  border: 5px solid transparent;
  border-left: 7px solid var(--brand);
  border-right: 0;
}

.ingress__dot {
  position: absolute;
  top: -3px;
  left: 0;
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--brand);
  box-shadow: 0 0 0 4px color-mix(in srgb, var(--brand) 22%, transparent);
  animation: ingress-travel-x 2.4s ease-in-out infinite;
}

.ingress__dot.is-late {
  animation-delay: 1.2s;
}

/* Gateway */
.ingress__gateway {
  overflow: hidden;
  border: 1px solid color-mix(in srgb, var(--brand) 40%, var(--vp-c-divider));
  border-radius: 24px;
  background: color-mix(in srgb, var(--brand) 5%, var(--vp-c-bg));
  box-shadow: 0 30px 70px -40px color-mix(in srgb, var(--brand) 60%, transparent);
}

.ingress__gateway header {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 14px 18px;
  border-bottom: 1px solid var(--vp-c-divider);
  background: color-mix(in srgb, var(--brand) 8%, var(--vp-c-bg-soft));
}

.ingress__gateway header > span:last-child {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 4px;
  min-width: 0;
}

.ingress__gateway header strong {
  color: var(--vp-c-text-1);
  font-size: 15px;
}

.ingress__gateway-icon,
.ingress__agents-icon {
  display: grid;
  flex: none;
  place-items: center;
  width: 36px;
  height: 36px;
  border-radius: 12px;
  background: color-mix(in srgb, var(--brand) 18%, transparent);
  color: var(--brand);
}

.ingress__steps {
  display: flex;
  flex-direction: column;
  gap: 8px;
  margin: 0;
  padding: 14px 18px;
  list-style: none;
}

.ingress__steps li {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  margin: 0;
  padding: 10px 12px;
  border-radius: 14px;
  background: var(--vp-c-bg-soft);
  animation: ingress-glow 3.6s ease-in-out infinite;
  animation-delay: calc(var(--i) * 0.6s);
}

.ingress__step-icon {
  display: grid;
  flex: none;
  place-items: center;
  width: 28px;
  height: 28px;
  border-radius: 9px;
  background: color-mix(in srgb, var(--go) 16%, transparent);
  color: color-mix(in srgb, var(--go) 85%, var(--vp-c-text-1));
}

.ingress__steps strong {
  display: block;
  color: var(--vp-c-text-1);
  font-size: 13.5px;
  line-height: 1.35;
}

.ingress__steps li span span {
  display: block;
  color: var(--vp-c-text-2);
  font-size: 12.5px;
  line-height: 1.4;
}

.ingress__gateway footer {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 12px 18px 14px;
  border-top: 1px dashed var(--vp-c-divider);
  color: var(--vp-c-text-2);
  font-size: 12.5px;
  line-height: 1.4;
}

.ingress__gateway footer > div {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px 8px;
}

.ingress__gateway footer > div > :first-child {
  color: var(--brand);
}

.ingress__gateway footer > div:last-child {
  flex-wrap: nowrap;
  align-items: flex-start;
}

.ingress__gateway footer > div:last-child > :first-child {
  flex: none;
  margin-top: 2px;
}

/* Streams and agents */
.ingress__right {
  display: flex;
  flex-direction: column;
}

.ingress__streams {
  padding: 18px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 22px;
  background: color-mix(in srgb, var(--vp-c-bg) 86%, transparent);
}

.ingress__streams ul {
  display: flex;
  flex-direction: column;
  gap: 8px;
  margin: 0;
  padding: 0;
  list-style: none;
}

.ingress__streams li {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 4px;
  margin: 0;
  padding: 8px 10px;
  border-radius: 12px;
  background: var(--vp-c-bg-soft);
}

.ingress__stream-name {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  color: var(--vp-c-text-3);
  font-family: var(--vp-font-family-mono);
  font-size: 11px;
}

.ingress__stream-name::before {
  content: "";
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--go);
}

.ingress__streams code {
  background: var(--vp-c-bg);
}

.ingress__down {
  position: relative;
  height: 28px;
}

.ingress__down span {
  position: absolute;
  top: 4px;
  bottom: 4px;
  left: 50%;
  width: 2px;
  background: linear-gradient(180deg, var(--vp-c-divider), var(--brand));
}

.ingress__down span::after {
  content: "";
  position: absolute;
  bottom: -1px;
  left: -4px;
  border: 5px solid transparent;
  border-top: 7px solid var(--brand);
  border-bottom: 0;
}

.ingress__agents {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 14px;
  border: 1px solid color-mix(in srgb, var(--brand) 40%, var(--vp-c-divider));
  border-radius: 18px;
  background: color-mix(in srgb, var(--brand) 6%, transparent);
}

.ingress__agents strong {
  display: block;
  color: var(--vp-c-text-1);
  font-size: 13.5px;
}

.ingress__agents span span {
  display: block;
  color: var(--vp-c-text-2);
  font-size: 12.5px;
  line-height: 1.5;
}

/* Side paths */
.ingress__side {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 14px;
  margin-top: 16px;
}

.ingress__side section {
  --tone: var(--stop);
  display: flex;
  align-items: flex-start;
  gap: 12px;
  padding: 14px 16px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 20px;
  background: color-mix(in srgb, var(--tone) 5%, var(--vp-c-bg));
}

.ingress__side section.is-unroutable {
  --tone: var(--wait);
}

.ingress__side-icon {
  display: grid;
  flex: none;
  place-items: center;
  width: 32px;
  height: 32px;
  border-radius: 10px;
  background: color-mix(in srgb, var(--tone) 16%, transparent);
  color: color-mix(in srgb, var(--tone) 85%, var(--vp-c-text-1));
}

.ingress__side strong {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
  color: var(--vp-c-text-1);
  font-size: 13.5px;
  line-height: 1.35;
}

.ingress__side em {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 1px 9px 1px 7px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--tone) 14%, transparent);
  color: color-mix(in srgb, var(--tone) 85%, var(--vp-c-text-1));
  font-family: var(--vp-font-family-mono);
  font-size: 11.5px;
  font-style: normal;
  font-weight: 600;
}

.ingress__side em::before {
  content: "";
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--tone);
}

.ingress__side section > span:last-child > span {
  display: block;
  margin-top: 4px;
  color: var(--vp-c-text-2);
  font-size: 12.5px;
  line-height: 1.5;
}

/* Accounts */
.ingress__accounts {
  margin-top: 16px;
  padding: 18px;
  border: 1px dashed color-mix(in srgb, var(--brand) 45%, var(--vp-c-divider));
  border-radius: 22px;
  background: color-mix(in srgb, var(--brand) 4%, transparent);
}

.ingress__account-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 12px;
}

.ingress__account {
  padding: 12px 14px;
  border-radius: 16px;
  background: var(--vp-c-bg-soft);
}

.ingress__account-name {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 1px 10px 1px 8px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--brand) 14%, transparent);
  color: color-mix(in srgb, var(--brand) 85%, var(--vp-c-text-1));
  font-size: 12px;
  font-weight: 600;
}

.ingress__account-name::before {
  content: "";
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--brand);
}

.ingress__account dl {
  margin: 10px 0 0;
}

.ingress__account dl > div {
  display: grid;
  grid-template-columns: 56px minmax(0, 1fr);
  gap: 8px;
  padding: 4px 0;
}

.ingress__account dt {
  padding-top: 2px;
  color: var(--vp-c-text-3);
  font-family: var(--vp-font-family-mono);
  font-size: 11.5px;
}

.ingress__account dd {
  margin: 0;
  min-width: 0;
}

.ingress__account code {
  background: var(--vp-c-bg);
}

@keyframes ingress-travel-x {
  0% {
    left: 0;
    opacity: 0;
  }
  15% {
    opacity: 1;
  }
  85% {
    opacity: 1;
  }
  100% {
    left: calc(100% - 8px);
    opacity: 0;
  }
}

@keyframes ingress-travel-y {
  0% {
    top: 0;
    opacity: 0;
  }
  15% {
    opacity: 1;
  }
  85% {
    opacity: 1;
  }
  100% {
    top: calc(100% - 8px);
    opacity: 0;
  }
}

@keyframes ingress-glow {
  0%,
  70%,
  100% {
    box-shadow: 0 0 0 0 transparent;
  }
  35% {
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--go) 35%, transparent);
  }
}

@media (max-width: 860px) {
  .ingress__flow {
    grid-template-columns: minmax(0, 1fr);
  }

  .ingress__arrow {
    height: 52px;
  }

  .ingress__line {
    top: 6px;
    bottom: 6px;
    left: 50%;
    right: auto;
    width: 2px;
    height: auto;
    background: linear-gradient(180deg, var(--vp-c-divider), var(--brand));
  }

  .ingress__line::after {
    top: auto;
    right: auto;
    bottom: -1px;
    left: -4px;
    border: 5px solid transparent;
    border-top: 7px solid var(--brand);
    border-bottom: 0;
  }

  .ingress__dot {
    top: 0;
    left: -3px;
    animation-name: ingress-travel-y;
  }

  .ingress__tiles {
    grid-template-columns: repeat(6, minmax(0, 1fr));
  }

  .ingress__side,
  .ingress__account-grid {
    grid-template-columns: minmax(0, 1fr);
  }
}

@media (max-width: 560px) {
  .ingress__tiles {
    grid-template-columns: repeat(3, minmax(0, 1fr));
  }
}

@media (prefers-reduced-motion: reduce) {
  .ingress__dot {
    display: none;
  }

  .ingress__steps li {
    animation: none;
  }
}
</style>
