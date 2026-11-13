<script setup lang="ts">
import { withBase } from "vitepress";
import BrandTile from "./BrandTile.vue";
import { STATUS_LABELS } from "./connectorLabels";
import { data as areas } from "./product.data";
</script>

<template>
  <ul class="product-overview">
    <li v-for="area in areas" :key="area.url">
      <a class="product-card" :href="withBase(area.url)">
        <div class="product-card__top">
          <BrandTile :name="area.name" :color="area.color" :icon-stroke="area.iconStroke" :size="48" />
          <span :class="['product-card__status', `is-${area.status}`]">{{ STATUS_LABELS[area.status] }}</span>
        </div>
        <span class="product-card__name">{{ area.name }}</span>
        <span class="product-card__tagline">{{ area.tagline }}</span>
      </a>
    </li>
  </ul>
</template>

<style scoped>
.product-overview {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(min(100%, 280px), 1fr));
  gap: 16px;
  margin: 32px 0 0;
  padding: 0;
  list-style: none;
}

.product-overview li {
  margin: 0;
}

.product-card {
  display: flex;
  flex-direction: column;
  gap: 6px;
  height: 100%;
  padding: 22px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 22px;
  background: color-mix(in srgb, var(--vp-c-bg) 86%, transparent);
  color: inherit;
  text-decoration: none;
  transition:
    border-color 0.2s,
    transform 0.2s,
    box-shadow 0.2s;
}

.product-card:hover {
  border-color: var(--vp-c-brand-2);
  box-shadow: 0 18px 40px -28px rgba(36, 28, 90, 0.45);
  transform: translateY(-2px);
}

.product-card__top {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  margin-bottom: 10px;
}

.product-card__name {
  color: var(--vp-c-text-1);
  font-size: 18px;
  font-weight: 600;
}

.product-card__tagline {
  color: var(--vp-c-text-2);
  font-size: 14px;
  line-height: 1.5;
}

.product-card__status {
  padding: 4px 12px;
  border-radius: 999px;
  background: var(--vp-c-bg-soft);
  color: var(--vp-c-text-1);
  font-size: 12px;
  font-weight: 500;
}

.product-card__status.is-preview {
  background: var(--vp-c-brand-soft);
  color: var(--vp-c-brand-1);
}

.product-card__status.is-planned {
  color: var(--vp-c-text-3);
}
</style>
