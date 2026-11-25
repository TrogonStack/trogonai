<script setup lang="ts">
import { computed } from "vue";
import { iconMarkup } from "./icons";

const props = withDefaults(defineProps<{ name: string; size?: number }>(), { size: 20 });

const markup = computed(() => iconMarkup(props.name));
</script>

<template>
  <svg
    class="icon"
    :width="size"
    :height="size"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="2"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
    v-html="markup"
  />
</template>

<style scoped>
.icon {
  flex: none;
}
</style>
