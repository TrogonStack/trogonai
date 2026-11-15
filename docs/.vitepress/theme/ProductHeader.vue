<script setup lang="ts">
import { computed } from "vue";
import { useData } from "vitepress";
import BrandTile from "./BrandTile.vue";
import { STATUS_LABELS } from "./connectorLabels";
import { data as areas } from "./product.data";

const { page } = useData();

const area = computed(() => {
  const url = `/${page.value.relativePath.replace(/(index)?\.md$/, "")}`;
  return areas.find((candidate) => candidate.url === url);
});
</script>

<template>
  <div v-if="area" class="product-header">
    <BrandTile :name="area.name" :color="area.color" :icon-stroke="area.iconStroke" :size="64" />
    <div>
      <h1>
        {{ area.name }}
        <span :class="['product-header__status', `is-${area.status}`]">{{ STATUS_LABELS[area.status] }}</span>
      </h1>
      <p>{{ area.tagline }}</p>
    </div>
  </div>
</template>

<style scoped>
.product-header {
  display: flex;
  align-items: center;
  gap: 20px;
  margin-bottom: 8px;
}

.product-header h1 {
  display: flex;
  align-items: center;
  gap: 12px;
  margin: 0;
}

.product-header p {
  margin: 4px 0 0;
  color: var(--vp-c-text-2);
}

.product-header__status {
  padding: 4px 12px;
  border-radius: 999px;
  background: var(--vp-c-bg-soft);
  color: var(--vp-c-text-1);
  font-size: 13px;
  font-weight: 500;
  letter-spacing: normal;
  line-height: 1.4;
}

.product-header__status.is-preview {
  background: var(--vp-c-brand-soft);
  color: var(--vp-c-brand-1);
}

.product-header__status.is-planned {
  color: var(--vp-c-text-3);
}
</style>
