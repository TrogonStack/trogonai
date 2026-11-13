<script setup lang="ts">
import BrandTile from "./BrandTile.vue";
import Icon from "./Icon.vue";
import { data } from "./connectors.data";

type Check = {
  slug: string;
  how: string;
  text: string;
  steps?: string[];
};

const spotlight: Check[] = [
  {
    slug: "plaid",
    how: "Signed key",
    text: "Each webhook carries a signed ES256 token. Every part of it has to hold up.",
    steps: [
      "Fetch the signing key by its id, then cache it",
      "Check the token was issued recently",
      "Match the body's SHA-256 to the one in the token",
    ],
  },
  {
    slug: "datadog",
    how: "Shared token",
    text: "Datadog does not sign its webhooks, so the gateway asks for a shared token in a header instead.",
    steps: [
      "Compare the token in constant time, so timing reveals nothing",
      "Optionally refuse a request that is not fresh",
    ],
  },
];

const checks: Check[] = [
  { slug: "github", how: "Signature", text: "Signed with your webhook secret." },
  { slug: "linear", how: "Signature", text: "Signed requests, with a 60 second timestamp tolerance." },
  { slug: "x", how: "Challenge", text: "Your consumer secret answers the CRC challenge and checks every signature." },
  {
    slug: "telegram",
    how: "Secret token",
    text: "A secret token header on every update. The gateway can register its own webhook at startup.",
  },
  { slug: "gitlab", how: "Token", text: "Checked against your signing token." },
  { slug: "incidentio", how: "Signature", text: "Checked against your signing secret." },
  { slug: "sentry", how: "Signature", text: "Checked against your client secret." },
  { slug: "notion", how: "Token", text: "Checked against your verification token." },
  { slug: "microsoft-graph", how: "Client state", text: "The client state on each notification must match yours." },
  {
    slug: "slack",
    how: "Signature",
    text: "Signed with your signing secret. Requests older than five minutes are refused, and Slack's SSL check is answered.",
  },
  {
    slug: "zoom",
    how: "Signature",
    text: "Signature over the timestamp and body, inside a five minute window. Zoom's URL validation is answered on the spot.",
  },
];

const outbound: Pick<Check, "slug" | "how">[] = [
  { slug: "discord", how: "Bot token" },
  { slug: "google-pubsub", how: "Streaming pull" },
];

const tones: Record<string, string> = {
  Signature: "#6f4cff",
  "Signed key": "#10b981",
  "Shared token": "#f59e0b",
  Token: "#14b8a6",
  "Secret token": "#14b8a6",
  Challenge: "#0ea5e9",
  "Client state": "#0ea5e9",
  "Bot token": "#ec4899",
  "Streaming pull": "#ec4899",
};

const bySlug = new Map(data.map((connector) => [connector.url.replace(/\.html$/, "").replace(/\/$/, "").split("/").pop(), connector]));

function tile(slug: string) {
  const connector = bySlug.get(slug);
  return {
    name: connector?.name ?? slug,
    color: connector?.color ?? "#6f4cff",
    iconPath: connector?.iconPath ?? null,
  };
}
</script>

<template>
  <figure class="connector-verify">
    <div class="connector-verify__spotlight">
      <section v-for="item in spotlight" :key="item.slug" class="connector-verify__hero">
        <header>
          <BrandTile v-bind="tile(item.slug)" :size="44" />
          <span>
            <strong>{{ tile(item.slug).name }}</strong>
            <em :style="{ '--chip': tones[item.how] }">{{ item.how }}</em>
          </span>
        </header>
        <p>{{ item.text }}</p>
        <ol>
          <li v-for="step in item.steps" :key="step">
            <span class="connector-verify__tick"><Icon name="check" :size="12" /></span>
            {{ step }}
          </li>
        </ol>
      </section>
    </div>

    <ul class="connector-verify__grid">
      <li v-for="item in checks" :key="item.slug">
        <header>
          <BrandTile v-bind="tile(item.slug)" :size="34" />
          <strong>{{ tile(item.slug).name }}</strong>
          <em :style="{ '--chip': tones[item.how] }">{{ item.how }}</em>
        </header>
        <p>{{ item.text }}</p>
      </li>
      <li class="connector-verify__outbound">
        <header>
          <span class="connector-verify__out-icon"><Icon name="arrow-right" :size="16" /></span>
          <strong>Not webhooks</strong>
        </header>
        <p>The gateway opens these connections itself.</p>
        <div v-for="item in outbound" :key="item.slug" class="connector-verify__out-row">
          <BrandTile v-bind="tile(item.slug)" :size="24" />
          <strong>{{ tile(item.slug).name }}</strong>
          <em :style="{ '--chip': tones[item.how] }">{{ item.how }}</em>
        </div>
      </li>
    </ul>

    <figcaption>
      <span class="connector-verify__refused"><Icon name="ban" :size="14" /> Refused</span>
      A request that fails its check is refused on the spot and never published, so no agent ever sees it.
    </figcaption>
  </figure>
</template>

<style scoped>
.connector-verify {
  margin: 24px 0 0;
}

.connector-verify__spotlight {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 16px;
}

.connector-verify__hero {
  padding: 20px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 24px;
  background:
    radial-gradient(120% 90% at 0% 0%, color-mix(in srgb, #6f4cff 10%, transparent), transparent 60%),
    color-mix(in srgb, var(--vp-c-bg) 86%, transparent);
  transition:
    transform 0.2s ease,
    box-shadow 0.2s ease;
}

.connector-verify__hero:hover,
.connector-verify__grid li:hover {
  transform: translateY(-3px);
  box-shadow: 0 24px 50px -32px rgba(36, 28, 90, 0.55);
}

.connector-verify__hero header,
.connector-verify__grid header {
  display: flex;
  align-items: center;
  gap: 12px;
}

.connector-verify__hero header > span {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px 10px;
}

.connector-verify strong {
  color: var(--vp-c-text-1);
  font-size: 15px;
  font-weight: 600;
  line-height: 1.3;
}

.connector-verify__grid strong {
  font-size: 14px;
}

.connector-verify em {
  --chip: #6f4cff;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 1px 9px 1px 7px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--chip) 14%, transparent);
  color: color-mix(in srgb, var(--chip) 82%, var(--vp-c-text-1));
  font-size: 11.5px;
  font-style: normal;
  font-weight: 600;
  white-space: nowrap;
}

.connector-verify em::before {
  content: "";
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--chip);
}

.connector-verify__grid em {
  margin-left: auto;
}

.connector-verify__hero p {
  margin: 14px 0 0;
  color: var(--vp-c-text-2);
  font-size: 13.5px;
  line-height: 1.5;
}

.connector-verify__hero ol {
  display: flex;
  flex-direction: column;
  gap: 8px;
  margin: 14px 0 0;
  padding: 0;
  list-style: none;
}

.connector-verify__hero li {
  display: flex;
  align-items: center;
  gap: 10px;
  margin: 0;
  padding: 9px 12px;
  border-radius: 12px;
  background: var(--vp-c-bg-soft);
  color: var(--vp-c-text-1);
  font-size: 13px;
  line-height: 1.4;
}

.connector-verify__tick {
  display: grid;
  flex: none;
  place-items: center;
  width: 22px;
  height: 22px;
  border-radius: 50%;
  background: color-mix(in srgb, #10b981 16%, transparent);
  color: #10b981;
}

.connector-verify__grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(min(100%, 250px), 1fr));
  gap: 12px;
  margin: 16px 0 0;
  padding: 0;
  list-style: none;
}

.connector-verify__grid li {
  margin: 0;
  padding: 14px 16px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 20px;
  background: color-mix(in srgb, var(--vp-c-bg) 86%, transparent);
  transition:
    transform 0.2s ease,
    box-shadow 0.2s ease;
}

.connector-verify__grid p {
  margin: 10px 0 0;
  color: var(--vp-c-text-2);
  font-size: 13px;
  line-height: 1.45;
}

.connector-verify__grid li.connector-verify__outbound {
  border-style: dashed;
  background: color-mix(in srgb, #ec4899 5%, transparent);
}

.connector-verify__out-icon {
  display: grid;
  flex: none;
  place-items: center;
  width: 34px;
  height: 34px;
  border-radius: 10px;
  background: color-mix(in srgb, #ec4899 16%, transparent);
  color: #ec4899;
}

.connector-verify__out-row {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-top: 8px;
  padding: 6px 10px;
  border-radius: 12px;
  background: var(--vp-c-bg-soft);
}

.connector-verify__out-row strong {
  min-width: 0;
  font-size: 13px;
}

.connector-verify__out-row em {
  margin-left: auto;
}

.connector-verify figcaption {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px 10px;
  margin-top: 16px;
  color: var(--vp-c-text-2);
  font-size: 13px;
  line-height: 1.5;
}

.connector-verify__refused {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 2px 10px;
  border-radius: 999px;
  background: color-mix(in srgb, #e11d48 14%, transparent);
  color: #e11d48;
  font-size: 12px;
  font-weight: 600;
}

@media (max-width: 860px) {
  .connector-verify__spotlight {
    grid-template-columns: minmax(0, 1fr);
  }
}

@media (prefers-reduced-motion: reduce) {
  .connector-verify__hero,
  .connector-verify__grid li {
    transition: none;
  }

  .connector-verify__hero:hover,
  .connector-verify__grid li:hover {
    transform: none;
  }
}
</style>
