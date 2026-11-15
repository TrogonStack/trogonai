<script setup lang="ts">
import { computed } from "vue";
import { useData } from "vitepress";
import BrandTile from "./BrandTile.vue";
import { CAPABILITY_LABELS, STATUS_LABELS } from "./connectorLabels";
import { data as connectors } from "./connectors.data";

const { page } = useData();

const connector = computed(() => {
  const url = `/${page.value.relativePath.replace(/(index)?\.md$/, "")}`;
  return connectors.find((candidate) => candidate.url === url);
});
</script>

<template>
  <div v-if="connector" class="connector-header">
    <div class="connector-header__title">
      <BrandTile :name="connector.name" :color="connector.color" :icon-path="connector.iconPath" :size="64" />
      <div>
        <h1>{{ connector.name }}</h1>
        <p>{{ connector.summary }}</p>
      </div>
    </div>

    <table>
      <thead>
        <tr>
          <th>Capability</th>
          <th>Status</th>
          <th>What it means</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="capability in connector.capabilities" :key="capability.kind">
          <td>{{ CAPABILITY_LABELS[capability.kind].name }}</td>
          <td>{{ STATUS_LABELS[capability.status] }}</td>
          <td>{{ CAPABILITY_LABELS[capability.kind].description }}</td>
        </tr>
      </tbody>
    </table>
  </div>
</template>

<style scoped>
.connector-header__title {
  display: flex;
  align-items: center;
  gap: 20px;
}

.connector-header__title h1 {
  margin: 0;
}

.connector-header table {
  margin-top: 24px;
}

.connector-header__title p {
  margin: 4px 0 0;
  color: var(--vp-c-text-2);
}
</style>
