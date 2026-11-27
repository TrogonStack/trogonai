<script setup lang="ts">
import { computed, ref } from "vue";
import { withBase } from "vitepress";
import Icon from "./Icon.vue";
import { data as providers, type Environment } from "./sandboxProviders.data";

const environments: Environment[] = ["Provider APIs", "Infrastructure", "Embedded & test"];
const selected = ref<Environment | "All providers">("All providers");
const visible = computed(() =>
  selected.value === "All providers"
    ? providers
    : providers.filter((provider) => provider.environment === selected.value),
);
</script>

<template>
  <div class="providers">
    <div class="provider-filters" role="group" aria-label="Filter providers by execution environment">
      <button type="button" :aria-pressed="selected === 'All providers'" @click="selected = 'All providers'">
        All providers
      </button>
      <button
        v-for="environment in environments"
        :key="environment"
        type="button"
        :aria-pressed="selected === environment"
        @click="selected = environment"
      >
        {{ environment }}
      </button>
    </div>
    <span class="visually-hidden" aria-live="polite">{{ visible.length }} providers shown</span>
    <ul class="provider-grid">
      <li v-for="provider in visible" :key="provider.kind">
        <a
          v-if="provider.logo"
          class="provider-mark"
          :href="provider.logo.website"
          :aria-label="`Visit ${provider.logo.owner ?? provider.name}`"
          :title="`Visit ${provider.logo.owner ?? provider.name}`"
        >
          <img v-if="provider.logo.file" :src="withBase(`/brand/sandbox/${provider.logo.file}`)" alt="" width="40" height="32">
          <span v-else class="brand-kit-link">Brand kit</span>
        </a>
        <a class="provider-card" :href="withBase(`/reference/sandbox-providers#${provider.kind}`)">
          <span class="provider-title">
            <span v-if="provider.icon" class="provider-mark provider-mark--builtin">
              <Icon :name="provider.icon" :size="24" />
            </span>
            <strong>{{ provider.name }}</strong>
            <Icon class="provider-arrow" name="arrow-right" :size="14" />
          </span>
          <span class="provider-summary">{{ provider.summary }}</span>
          <span v-if="provider.status" class="provider-status">{{ provider.status }}</span>
        </a>
      </li>
    </ul>
  </div>
</template>

<style scoped>
.providers {
  display: grid;
  gap: 14px;
  margin: 20px 0 32px;
}
.provider-filters {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}
.provider-filters button {
  padding: 6px 12px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 999px;
  color: var(--vp-c-text-2);
  font-size: 13px;
  line-height: 1.4;
  font-weight: 550;
}
.provider-filters button:hover,
.provider-card:hover {
  border-color: var(--vp-c-brand-1);
}
.provider-filters button[aria-pressed="true"] {
  border-color: var(--vp-c-text-1);
  background: var(--vp-c-text-1);
  color: var(--vp-c-bg);
}
.provider-filters button:focus-visible,
.provider-mark:focus-visible,
.provider-card:focus-visible {
  outline: 2px solid var(--vp-c-brand-1);
  outline-offset: 2px;
}
.provider-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(min(100%, 220px), 1fr));
  gap: 8px;
  margin: 0;
  padding: 0;
  list-style: none;
}
.provider-grid li {
  position: relative;
  margin: 0;
  min-width: 0;
}
.provider-card {
  display: grid;
  grid-template-rows: auto 1fr;
  gap: 8px;
  height: 100%;
  padding: 13px 15px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 14px;
  background: var(--vp-c-bg);
  color: var(--vp-c-text-1);
  text-decoration: none;
}
.provider-title {
  display: flex;
  align-items: center;
  gap: 9px;
  min-height: 48px;
  padding-left: 64px;
  font-size: 15px;
  line-height: 1.25;
}
.provider-mark {
  position: absolute;
  top: 14px;
  left: 16px;
  z-index: 1;
  display: grid;
  place-items: center;
  width: 52px;
  height: 48px;
  border: 1px solid #e4e6ea;
  border-radius: 9px;
  background: #fff;
}
.provider-mark img {
  display: block;
  width: 40px;
  height: 32px;
  object-fit: contain;
}
a.provider-mark:hover { border-color: var(--vp-c-brand-1); }
.brand-kit-link {
  color: #384457;
  font-size: 11px;
  font-weight: 600;
  text-decoration: underline;
}
.provider-mark--builtin {
  color: var(--vp-c-text-2);
  border-color: var(--vp-c-divider);
  background: var(--vp-c-bg-soft);
}
.provider-arrow {
  flex: none;
  margin-left: auto;
  color: var(--vp-c-text-2);
}
.provider-summary {
  color: var(--vp-c-text-2);
  font-size: 13px;
  line-height: 1.45;
  font-weight: 400;
}
.provider-status {
  display: flex;
  align-items: center;
  gap: 6px;
  color: var(--vp-c-text-2);
  font-size: 11.5px;
  line-height: 1.4;
  font-weight: 550;
}
.provider-status::before {
  content: "";
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--vp-c-text-3);
}
</style>
