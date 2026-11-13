<script setup lang="ts">
import { computed, ref } from "vue";
import { withBase } from "vitepress";
import BrandTile from "./BrandTile.vue";
import { STATUS_LABELS } from "./connectorLabels";
import { type CapabilityKind, type Connector, data } from "./connectors.data";

const props = withDefaults(defineProps<{ filters?: boolean; limit?: number; capability?: CapabilityKind }>(), {
  filters: true,
  limit: undefined,
  capability: undefined,
});

const STATUS_RANK = { available: 0, preview: 1, planned: 2 } as const;

// With `capability`, the gallery lists only connectors that declare it and shows that capability's status.
const connectors = computed<Connector[]>(() => {
  if (props.capability === undefined) return data;
  return data
    .flatMap((connector) => {
      const match = connector.capabilities.find((capability) => capability.kind === props.capability);
      return match ? [{ ...connector, status: match.status }] : [];
    })
    .sort((left, right) => STATUS_RANK[left.status] - STATUS_RANK[right.status] || left.name.localeCompare(right.name));
});

const ALL = "All";

const categories = computed(() => [ALL, ...new Set(connectors.value.map((connector) => connector.category))].sort(compareCategory));
const selected = ref(ALL);
const visible = computed(() => {
  const matching =
    selected.value === ALL ? connectors.value : connectors.value.filter((connector) => connector.category === selected.value);
  return props.limit === undefined ? matching : matching.slice(0, props.limit);
});

function compareCategory(left: string, right: string) {
  if (left === ALL) return -1;
  if (right === ALL) return 1;
  return left.localeCompare(right);
}
</script>

<template>
  <section class="connector-gallery">
    <div v-if="filters" class="connector-gallery__filters" role="tablist" aria-label="Filter connectors by category">
      <button
        v-for="category in categories"
        :key="category"
        type="button"
        role="tab"
        :aria-selected="selected === category"
        :class="['connector-gallery__filter', { 'is-active': selected === category }]"
        @click="selected = category"
      >
        {{ category }}
      </button>
    </div>

    <ul class="connector-gallery__grid">
      <li v-for="connector in visible" :key="connector.url">
        <a class="connector-row" :href="withBase(connector.url)">
          <BrandTile :name="connector.name" :color="connector.color" :icon-path="connector.iconPath" />
          <span class="connector-row__text">
            <span class="connector-row__name">{{ connector.name }}</span>
            <span class="connector-row__summary">{{ connector.summary }}</span>
          </span>
          <span :class="['connector-row__status', `is-${connector.status}`]">{{ STATUS_LABELS[connector.status] }}</span>
        </a>
      </li>
    </ul>
  </section>
</template>

<style scoped>
.connector-gallery {
  margin-top: 32px;
  padding: 32px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 28px;
  background: color-mix(in srgb, var(--vp-c-bg) 86%, transparent);
  box-shadow: 0 24px 60px -32px rgba(36, 28, 90, 0.28);
}

.connector-gallery__filters {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-bottom: 24px;
}

.connector-gallery__filter {
  padding: 6px 14px;
  border-radius: 999px;
  background: var(--vp-c-bg-soft);
  color: var(--vp-c-text-2);
  font-size: 14px;
  font-weight: 500;
  transition:
    background-color 0.2s,
    color 0.2s;
}

.connector-gallery__filter:hover {
  color: var(--vp-c-text-1);
}

.connector-gallery__filter.is-active {
  background: var(--vp-c-text-1);
  color: var(--vp-c-bg);
}

.connector-gallery__grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(min(100%, 340px), 1fr));
  gap: 4px 32px;
  margin: 0;
  padding: 0;
  list-style: none;
}

.connector-gallery__grid li {
  margin: 0;
}

.connector-row {
  display: flex;
  align-items: center;
  gap: 16px;
  margin: 0 -12px;
  padding: 12px;
  border-radius: 18px;
  color: inherit;
  text-decoration: none;
  transition: background-color 0.2s;
}

.connector-row:hover {
  background: var(--vp-c-bg-soft);
}

.connector-row__text {
  display: flex;
  flex: 1;
  flex-direction: column;
  min-width: 0;
}

.connector-row__name {
  color: var(--vp-c-text-1);
  font-size: 17px;
  font-weight: 600;
  line-height: 1.3;
}

.connector-row__summary {
  overflow: hidden;
  color: var(--vp-c-text-2);
  font-size: 14px;
  line-height: 1.4;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.connector-row__status {
  flex: none;
  padding: 6px 14px;
  border-radius: 999px;
  background: var(--vp-c-bg-soft);
  color: var(--vp-c-text-1);
  font-size: 13px;
  font-weight: 500;
}

.connector-row:hover .connector-row__status {
  background: var(--vp-c-bg);
}

.connector-row__status.is-preview {
  background: var(--vp-c-brand-soft);
  color: var(--vp-c-brand-1);
}

.connector-row__status.is-planned {
  color: var(--vp-c-text-3);
}

@media (max-width: 640px) {
  .connector-gallery {
    padding: 20px 16px;
    border-radius: 20px;
  }
}
</style>
