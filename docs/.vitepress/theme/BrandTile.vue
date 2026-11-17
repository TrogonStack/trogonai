<script setup lang="ts">
import { computed } from "vue";

const props = withDefaults(
  defineProps<{
    name: string;
    color: string;
    /** Filled 24x24 path, as simple-icons provides. */
    iconPath?: string | null;
    /** Inner markup of a stroked 24x24 icon, as lucide provides. */
    iconStroke?: string | null;
    size?: number;
  }>(),
  { iconPath: null, iconStroke: null, size: 52 },
);

const monogram = computed(() => props.name.charAt(0).toUpperCase());
</script>

<template>
  <span class="brand-tile" :style="{ '--tile-color': color, '--tile-size': `${size}px` }" aria-hidden="true">
    <svg v-if="iconPath" viewBox="0 0 24 24" class="brand-tile__fill">
      <path :d="iconPath" />
    </svg>
    <svg
      v-else-if="iconStroke"
      viewBox="0 0 24 24"
      class="brand-tile__stroke"
      fill="none"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
      v-html="iconStroke"
    />
    <span v-else class="brand-tile__monogram">{{ monogram }}</span>
  </span>
</template>

<style scoped>
.brand-tile {
  position: relative;
  display: inline-grid;
  flex: none;
  place-items: center;
  width: var(--tile-size);
  height: var(--tile-size);
  border-radius: calc(var(--tile-size) * 0.24);
  background:
    linear-gradient(160deg, rgba(255, 255, 255, 0.22), rgba(255, 255, 255, 0) 55%),
    var(--tile-color);
  box-shadow:
    0 1px 2px rgba(16, 16, 26, 0.12),
    0 6px 16px -6px color-mix(in srgb, var(--tile-color) 60%, transparent),
    inset 0 0 0 1px rgba(255, 255, 255, 0.08);
}

.dark .brand-tile {
  box-shadow:
    0 1px 2px rgba(0, 0, 0, 0.4),
    inset 0 0 0 1px rgba(255, 255, 255, 0.14);
}

.brand-tile__fill {
  width: 54%;
  height: 54%;
  fill: #fff;
}

.brand-tile__stroke {
  width: 50%;
  height: 50%;
  stroke: #fff;
}

.brand-tile__monogram {
  color: #fff;
  font-size: calc(var(--tile-size) * 0.46);
  font-weight: 650;
  line-height: 1;
  letter-spacing: -0.02em;
}
</style>
