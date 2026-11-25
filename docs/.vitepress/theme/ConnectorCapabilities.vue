<script setup lang="ts">
import { withBase } from "vitepress";
import BrandTile from "./BrandTile.vue";
import Icon from "./Icon.vue";
import { data } from "./connectors.data";

const eventSources = data.filter((connector) =>
  connector.capabilities.some((capability) => capability.kind === "events" && capability.status === "available"),
);

const telegram = data.find((connector) => connector.name === "Telegram");
</script>

<template>
  <figure class="connector-caps">
    <section class="connector-caps__card" style="--tone: #10b981">
      <header>
        <span class="connector-caps__icon"><Icon name="radio" :size="18" /></span>
        <h3>Events</h3>
        <span class="connector-caps__pill is-go">Available</span>
      </header>
      <p class="connector-caps__lede">Hear what happens.</p>
      <p>Every connector here delivers its provider's events, ready for any agent that subscribes.</p>

      <div class="connector-caps__tiles">
        <a
          v-for="(connector, index) in eventSources"
          :key="connector.url"
          :href="withBase(connector.url)"
          :title="connector.name"
          :style="{ '--i': index }"
        >
          <BrandTile :name="connector.name" :color="connector.color" :icon-path="connector.iconPath" :size="34" />
        </a>
      </div>

      <div class="connector-caps__subject">
        <span>Lands on</span>
        <code>linear-main.Issue</code>
      </div>
    </section>

    <section class="connector-caps__card" style="--tone: #f59e0b">
      <header>
        <span class="connector-caps__icon"><Icon name="zap" :size="18" /></span>
        <h3>Actions</h3>
        <span class="connector-caps__pill is-wait">Designed</span>
      </header>
      <p class="connector-caps__lede">Act in the tool.</p>
      <p>
        Agents will call provider APIs through a connection your project owns. A proxy adds the credential on the way
        out, so the agent never sees the key.
      </p>

      <ol class="connector-caps__flow">
        <li>
          <span class="connector-caps__node"><Icon name="bot" :size="15" /></span>
          <span>Agent</span>
        </li>
        <li class="is-proxy">
          <span class="connector-caps__node"><Icon name="key-round" :size="15" /></span>
          <span>Proxy adds the credential</span>
        </li>
        <li>
          <span class="connector-caps__node"><Icon name="globe" :size="15" /></span>
          <span>Provider API</span>
        </li>
      </ol>

      <a class="connector-caps__link" :href="withBase('/product/keys-and-secrets')">
        How credentials are kept <Icon name="arrow-right" :size="14" />
      </a>
    </section>

    <section class="connector-caps__card" style="--tone: #6f4cff">
      <header>
        <span class="connector-caps__icon"><Icon name="message-square" :size="18" /></span>
        <h3>Channel</h3>
      </header>
      <div class="connector-caps__pills">
        <span class="connector-caps__pill is-go">Telegram today</span>
        <span class="connector-caps__pill is-wait">Slack and Discord designed</span>
      </div>
      <p class="connector-caps__lede">Talk with people.</p>
      <p>Agents hold conversations inside the tool. Slack and Discord events already arrive here.</p>

      <div class="connector-caps__chat">
        <div class="connector-caps__chat-head">
          <BrandTile
            v-if="telegram"
            :name="telegram.name"
            :color="telegram.color"
            :icon-path="telegram.iconPath"
            :size="24"
          />
          <span>Telegram</span>
        </div>
        <p class="connector-caps__bubble is-person">Is the deploy for the billing fix done?</p>
        <p class="connector-caps__bubble is-agent">It finished a few minutes ago. Every check passed.</p>
      </div>
    </section>
  </figure>
</template>

<style scoped>
.connector-caps {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 16px;
  margin: 24px 0 0;
}

.connector-caps__card {
  display: flex;
  flex-direction: column;
  padding: 20px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 24px;
  background:
    radial-gradient(120% 70% at 100% 0%, color-mix(in srgb, var(--tone) 12%, transparent), transparent 60%),
    color-mix(in srgb, var(--vp-c-bg) 86%, transparent);
  transition:
    transform 0.2s ease,
    box-shadow 0.2s ease;
}

.connector-caps__card:hover {
  transform: translateY(-3px);
  box-shadow: 0 26px 56px -34px rgba(36, 28, 90, 0.55);
}

.connector-caps__card header {
  display: flex;
  align-items: center;
  gap: 10px;
}

.connector-caps__card h3 {
  margin: 0;
  padding: 0;
  border: none;
  color: var(--vp-c-text-1);
  font-size: 17px;
  font-weight: 650;
  letter-spacing: normal;
}

.connector-caps__card header .connector-caps__pill {
  margin-left: auto;
}

.connector-caps__icon {
  display: grid;
  flex: none;
  place-items: center;
  width: 36px;
  height: 36px;
  border-radius: 12px;
  background: color-mix(in srgb, var(--tone) 16%, transparent);
  color: color-mix(in srgb, var(--tone) 85%, var(--vp-c-text-1));
}

.connector-caps__pills {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-top: 12px;
}

.connector-caps__pill {
  --chip: #10b981;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 1px 9px 1px 7px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--chip) 14%, transparent);
  color: color-mix(in srgb, var(--chip) 85%, var(--vp-c-text-1));
  font-size: 11.5px;
  font-weight: 600;
  white-space: nowrap;
}

.connector-caps__pill::before {
  content: "";
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--chip);
}

.connector-caps__pill.is-wait {
  --chip: #f59e0b;
}

.connector-caps__card p {
  margin: 8px 0 0;
  color: var(--vp-c-text-2);
  font-size: 13.5px;
  line-height: 1.5;
}

.connector-caps__card p.connector-caps__lede {
  margin-top: 14px;
  color: var(--vp-c-text-1);
  font-size: 15px;
  font-weight: 600;
}

.connector-caps__tiles {
  display: grid;
  grid-template-columns: repeat(5, max-content);
  gap: 10px;
  margin-top: 16px;
}

.connector-caps__tiles a {
  display: inline-flex;
  border-radius: 9px;
  transition: transform 0.2s ease;
  animation: connector-caps-pop 0.5s ease both;
  animation-delay: calc(var(--i) * 45ms);
}

.connector-caps__tiles a:hover {
  transform: translateY(-3px) scale(1.08);
}

@keyframes connector-caps-pop {
  from {
    opacity: 0;
    transform: translateY(6px) scale(0.9);
  }
}

.connector-caps__subject {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
  margin-top: auto;
  padding-top: 16px;
  color: var(--vp-c-text-3);
  font-size: 12px;
}

.connector-caps__subject code {
  padding: 3px 9px;
  border-radius: 8px;
  background: var(--vp-c-bg-soft);
  color: var(--vp-c-text-1);
  font-size: 12px;
}

.connector-caps__flow {
  display: flex;
  flex-direction: column;
  gap: 0;
  margin: 16px 0 0;
  padding: 0;
  list-style: none;
}

.connector-caps__flow li {
  position: relative;
  display: flex;
  align-items: center;
  gap: 10px;
  margin: 0;
  padding: 8px 12px;
  border-radius: 12px;
  background: var(--vp-c-bg-soft);
  color: var(--vp-c-text-1);
  font-size: 13px;
  font-weight: 500;
}

.connector-caps__flow li + li {
  margin-top: 14px;
}

.connector-caps__flow li + li::before {
  content: "";
  position: absolute;
  top: -13px;
  left: 25px;
  width: 2px;
  height: 12px;
  background: linear-gradient(180deg, var(--vp-c-divider), #f59e0b);
}

.connector-caps__flow li.is-proxy {
  background: color-mix(in srgb, #f59e0b 12%, var(--vp-c-bg-soft));
}

.connector-caps__node {
  display: grid;
  flex: none;
  place-items: center;
  width: 28px;
  height: 28px;
  border-radius: 9px;
  background: var(--vp-c-bg);
  color: color-mix(in srgb, #f59e0b 85%, var(--vp-c-text-1));
}

.connector-caps__link {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  margin-top: auto;
  padding-top: 16px;
  color: var(--vp-c-brand-1);
  font-size: 13px;
  font-weight: 600;
  text-decoration: none;
}

.connector-caps__link:hover {
  text-decoration: underline;
}

.connector-caps__chat {
  display: flex;
  flex-direction: column;
  gap: 8px;
  margin-top: auto;
  padding: 12px;
  border-radius: 16px;
  background: var(--vp-c-bg-soft);
}

.connector-caps__card > p + .connector-caps__chat {
  margin-top: 16px;
}

.connector-caps__chat-head {
  display: flex;
  align-items: center;
  gap: 8px;
  color: var(--vp-c-text-2);
  font-size: 12px;
  font-weight: 600;
}

.connector-caps__card p.connector-caps__bubble {
  max-width: 85%;
  margin: 0;
  padding: 8px 12px;
  border-radius: 14px;
  font-size: 13px;
  line-height: 1.4;
}

.connector-caps__bubble.is-person {
  align-self: flex-end;
  border-bottom-right-radius: 4px;
  background: color-mix(in srgb, #6f4cff 85%, #000);
  color: #fff;
}

.connector-caps__bubble.is-agent {
  align-self: flex-start;
  border-bottom-left-radius: 4px;
  background: var(--vp-c-bg);
  color: var(--vp-c-text-1);
  animation: connector-caps-reply 0.6s ease 0.3s both;
}

@keyframes connector-caps-reply {
  from {
    opacity: 0;
    transform: translateY(6px);
  }
}

@media (max-width: 960px) {
  .connector-caps {
    grid-template-columns: minmax(0, 1fr);
  }
}

@media (prefers-reduced-motion: reduce) {
  .connector-caps__card,
  .connector-caps__tiles a {
    transition: none;
    animation: none;
  }

  .connector-caps__bubble.is-agent {
    animation: none;
  }

  .connector-caps__card:hover,
  .connector-caps__tiles a:hover {
    transform: none;
  }
}
</style>
