<script setup lang="ts">
import { withBase } from "vitepress";
import BrandTile from "./BrandTile.vue";
import { data as brands } from "./brandIcons.data";
import { iconMarkup } from "./icons";

type Tier = {
  name: string;
  color: string;
  iconPath?: string;
  iconStroke?: string;
  text: string;
  link?: string;
};

const platform: Tier[] = [
  {
    name: "Managed key",
    color: "#6F4CFF",
    iconStroke: iconMarkup("shield-check"),
    text: "A shared key in the platform's OpenBao. Every use carries an authenticated binding that keeps tenants apart.",
  },
  {
    name: "Dedicated key",
    color: "#8B5CF6",
    iconStroke: iconMarkup("key-square"),
    text: "A key of your own in the platform backend, so your data can be crypto-erased without touching anyone else's.",
  },
];

const customer: Tier[] = [
  {
    name: "AWS KMS",
    color: "#FF9900",
    iconStroke: iconMarkup("cloud"),
    text: "A key in your AWS account. The platform assumes a role you scope to that one key.",
    link: "/how-to/bring-your-own-aws-kms-key",
  },
  {
    name: "Google Cloud KMS",
    color: `#${brands.googlecloud.hex}`,
    iconPath: brands.googlecloud.path,
    text: "A CryptoKey in your project, reached through Workload Identity Federation. No service account key is stored.",
    link: "/how-to/bring-your-own-google-cloud-kms-key",
  },
  {
    name: "OpenBao",
    color: "#2B3A42",
    iconPath: brands.openbao.path,
    text: "A Transit key in your own OpenBao, behind a policy that allows only encrypt and decrypt.",
    link: "/how-to/bring-your-own-openbao",
  },
];
</script>

<template>
  <figure class="custody-tiers">
    <div class="custody-tiers__scale" aria-hidden="true">
      <span>Platform holds the key</span>
      <span class="custody-tiers__bar" />
      <span>You hold the key</span>
    </div>

    <div class="custody-tiers__groups">
      <section class="custody-tiers__group is-platform">
        <h3>Platform managed</h3>
        <div class="custody-tiers__cards">
          <div v-for="tier in platform" :key="tier.name" class="custody-tiers__card">
            <BrandTile :name="tier.name" :color="tier.color" :icon-stroke="tier.iconStroke" :size="44" />
            <strong>{{ tier.name }}</strong>
            <span>{{ tier.text }}</span>
          </div>
        </div>
      </section>

      <section class="custody-tiers__group is-customer">
        <h3>Customer managed: bring your own key</h3>
        <div class="custody-tiers__cards">
          <a
            v-for="tier in customer"
            :key="tier.name"
            class="custody-tiers__card is-link"
            :href="withBase(tier.link ?? '#')"
          >
            <BrandTile
              :name="tier.name"
              :color="tier.color"
              :icon-path="tier.iconPath"
              :icon-stroke="tier.iconStroke"
              :size="44"
            />
            <strong>{{ tier.name }}</strong>
            <span>{{ tier.text }}</span>
            <em>Setup guide</em>
          </a>
        </div>
        <p class="custody-tiers__veto">
          <span class="custody-tiers__veto-dot" />
          Disable the key in your provider and every use stops. The platform cannot override that, and it never falls
          back to a key of its own.
        </p>
      </section>
    </div>
  </figure>
</template>

<style scoped>
.custody-tiers {
  margin: 24px 0 0;
}

.custody-tiers__scale {
  display: flex;
  align-items: center;
  gap: 14px;
  margin-bottom: 18px;
  color: var(--vp-c-text-2);
  font-size: 12px;
  font-weight: 600;
  letter-spacing: 0.04em;
  text-transform: uppercase;
  white-space: nowrap;
}

.custody-tiers__bar {
  flex: 1;
  height: 6px;
  border-radius: 999px;
  background: linear-gradient(90deg, #6f4cff, #8b5cf6 35%, #e11d48);
}

.custody-tiers__groups {
  display: grid;
  grid-template-columns: 2fr 3fr;
  gap: 16px;
}

.custody-tiers__group {
  padding: 18px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 24px;
  background: color-mix(in srgb, var(--vp-c-bg) 86%, transparent);
}

.custody-tiers__group.is-customer {
  border-color: color-mix(in srgb, #e11d48 30%, var(--vp-c-divider));
  background:
    linear-gradient(160deg, color-mix(in srgb, #e11d48 6%, transparent), transparent 60%),
    color-mix(in srgb, var(--vp-c-bg) 86%, transparent);
}

.custody-tiers__group h3 {
  margin: 0 0 14px;
  padding: 0;
  border: none;
  color: var(--vp-c-text-1);
  font-size: 14px;
  font-weight: 600;
  letter-spacing: normal;
}

.custody-tiers__cards {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(min(100%, 150px), 1fr));
  gap: 10px;
}

.custody-tiers__card {
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 16px;
  border-radius: 18px;
  background: var(--vp-c-bg-soft);
  color: inherit;
  text-decoration: none;
  transition:
    transform 0.2s,
    box-shadow 0.2s;
}

.custody-tiers__card strong {
  margin-top: 8px;
  color: var(--vp-c-text-1);
  font-size: 15px;
}

.custody-tiers__card span {
  color: var(--vp-c-text-2);
  font-size: 13px;
  line-height: 1.5;
}

.custody-tiers__card em {
  margin-top: auto;
  padding-top: 6px;
  color: var(--vp-c-brand-1);
  font-size: 12.5px;
  font-style: normal;
  font-weight: 600;
}

.custody-tiers__card.is-link:hover {
  box-shadow: 0 14px 30px -22px rgba(36, 28, 90, 0.5);
  transform: translateY(-2px);
}

.custody-tiers__veto {
  display: flex;
  align-items: baseline;
  gap: 10px;
  margin: 14px 2px 0;
  color: var(--vp-c-text-1);
  font-size: 13.5px;
  line-height: 1.5;
}

.custody-tiers__veto-dot {
  flex: none;
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: #e11d48;
  box-shadow: 0 0 0 4px color-mix(in srgb, #e11d48 20%, transparent);
}

@media (max-width: 860px) {
  .custody-tiers__groups {
    grid-template-columns: 1fr;
  }

  .custody-tiers__scale {
    font-size: 10.5px;
  }
}
</style>
