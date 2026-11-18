<script setup lang="ts">
import { withBase } from "vitepress";
import Icon from "./Icon.vue";

defineProps<{
  items: Array<{
    name: string;
    fullName: string;
    icon: string;
    from: string;
    to: string;
    purpose: string;
    transport: string;
    link: string;
  }>;
}>();
</script>

<template>
  <ul class="protocol-overview">
    <li v-for="item in items" :key="item.name">
      <div class="protocol-overview__heading">
        <span class="protocol-overview__icon"><Icon :name="item.icon" :size="22" /></span>
        <strong>{{ item.name }}</strong>
      </div>
      <span class="protocol-overview__name">{{ item.fullName }}</span>
      <div class="protocol-overview__route">
        <span>{{ item.from }}</span>
        <Icon name="arrow-right" :size="18" />
        <span>{{ item.to }}</span>
      </div>
      <p>{{ item.purpose }}</p>
      <span class="protocol-overview__transport">{{ item.transport }}</span>
      <a :href="withBase(item.link)">Explore {{ item.name }} <Icon name="arrow-right" :size="14" /></a>
    </li>
  </ul>
</template>

<style scoped>
.protocol-overview {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(min(100%, 250px), 1fr));
  gap: 16px;
  margin: 24px 0 0;
  padding: 0;
  list-style: none;
}

.protocol-overview li {
  display: flex;
  flex-direction: column;
  gap: 12px;
  min-width: 0;
  margin: 0;
  padding: 22px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 22px;
  background: color-mix(in srgb, var(--vp-c-bg) 86%, transparent);
}

.protocol-overview__heading {
  display: flex;
  align-items: center;
  gap: 12px;
  color: var(--vp-c-text-1);
  font-size: 22px;
}

.protocol-overview__icon {
  display: grid;
  flex: none;
  place-items: center;
  width: 44px;
  height: 44px;
  border-radius: 14px;
  background: var(--vp-c-brand-soft);
  color: var(--vp-c-brand-1);
}

.protocol-overview__name {
  color: var(--vp-c-text-2);
  font-size: 13px;
}

.protocol-overview__route {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
  padding: 14px;
  border-radius: 14px;
  background: var(--vp-c-bg-soft);
  color: var(--vp-c-text-1);
  font-size: 13px;
  font-weight: 600;
}

.protocol-overview p {
  flex: 1;
  margin: 0;
  color: var(--vp-c-text-2);
  font-size: 14px;
  line-height: 1.6;
}

.protocol-overview__transport {
  color: var(--vp-c-text-3);
  font-size: 12px;
  line-height: 1.5;
}

.protocol-overview a {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  width: fit-content;
  font-size: 13px;
  font-weight: 600;
}
</style>
