<script setup lang="ts">
import Icon from "./Icon.vue";

defineProps<{ items: Array<{ icon: string; title: string; text: string }> }>();
</script>

<template>
  <ul class="feature-grid">
    <li v-for="item in items" :key="item.title">
      <span class="feature-grid__icon"><Icon :name="item.icon" :size="20" /></span>
      <strong>{{ item.title }}</strong>
      <span>{{ item.text }}</span>
    </li>
  </ul>
</template>

<style scoped>
.feature-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(min(100%, 280px), 1fr));
  gap: 12px;
  margin: 24px 0 0;
  padding: 0;
  list-style: none;
}

.feature-grid li {
  display: flex;
  flex-direction: column;
  gap: 6px;
  margin: 0;
  padding: 20px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 20px;
  background: color-mix(in srgb, var(--vp-c-bg) 86%, transparent);
}

.feature-grid__icon {
  display: grid;
  place-items: center;
  width: 40px;
  height: 40px;
  margin-bottom: 6px;
  border-radius: 12px;
  background: var(--vp-c-brand-soft);
  color: var(--vp-c-brand-1);
}

.feature-grid strong {
  color: var(--vp-c-text-1);
  font-size: 15px;
  line-height: 1.35;
}

.feature-grid li > span:last-child {
  color: var(--vp-c-text-2);
  font-size: 13.5px;
  line-height: 1.5;
}
</style>
