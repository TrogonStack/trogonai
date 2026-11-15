<script setup lang="ts">
const LABELS = { built: "Built", partial: "In progress", designed: "Designed" } as const;

defineProps<{ items: Array<{ name: string; status: keyof typeof LABELS; note: string }> }>();
</script>

<template>
  <ul class="build-progress">
    <li v-for="item in items" :key="item.name">
      <span :class="['build-progress__dot', `is-${item.status}`]" aria-hidden="true" />
      <span class="build-progress__text">
        <strong>{{ item.name }}</strong>
        <span>{{ item.note }}</span>
      </span>
      <span :class="['build-progress__pill', `is-${item.status}`]">{{ LABELS[item.status] }}</span>
    </li>
  </ul>
</template>

<style scoped>
.build-progress {
  margin: 24px 0 0;
  padding: 8px 20px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 22px;
  background: color-mix(in srgb, var(--vp-c-bg) 86%, transparent);
  list-style: none;
}

.build-progress li {
  display: flex;
  align-items: center;
  gap: 14px;
  margin: 0;
  padding: 14px 0;
}

.build-progress li + li {
  border-top: 1px solid var(--vp-c-divider);
}

.build-progress__dot {
  flex: none;
  width: 10px;
  height: 10px;
  border-radius: 50%;
  border: 2px solid var(--vp-c-text-3);
}

.build-progress__dot.is-built {
  border-color: #14b8a6;
  background: #14b8a6;
}

.build-progress__dot.is-partial {
  border-color: #f59e0b;
  background: linear-gradient(90deg, #f59e0b 50%, transparent 50%);
}

.build-progress__text {
  display: flex;
  flex: 1;
  flex-direction: column;
  min-width: 0;
}

.build-progress__text strong {
  color: var(--vp-c-text-1);
  font-size: 14.5px;
}

.build-progress__text span {
  color: var(--vp-c-text-2);
  font-size: 13px;
  line-height: 1.45;
}

.build-progress__pill {
  flex: none;
  padding: 4px 12px;
  border-radius: 999px;
  background: var(--vp-c-bg-soft);
  color: var(--vp-c-text-3);
  font-size: 12px;
  font-weight: 600;
}

.build-progress__pill.is-built {
  background: color-mix(in srgb, #14b8a6 14%, transparent);
  color: color-mix(in srgb, #14b8a6 75%, var(--vp-c-text-1));
}

.build-progress__pill.is-partial {
  background: color-mix(in srgb, #f59e0b 16%, transparent);
  color: color-mix(in srgb, #f59e0b 70%, var(--vp-c-text-1));
}
</style>
