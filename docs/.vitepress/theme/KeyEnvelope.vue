<script setup lang="ts">
import { type VNode, h } from "vue";
import Icon from "./Icon.vue";

type Layer = { icon: string; name: string; holder: string; text: string };

const layers: Layer[] = [
  {
    icon: "server",
    name: "Key backend",
    holder: "Platform or you",
    text: "The root of trust: the platform's OpenBao, or your own AWS KMS, Google Cloud KMS or OpenBao.",
  },
  {
    icon: "key-round",
    name: "Key-encryption key (KEK)",
    holder: "Never leaves the backend",
    text: "Wraps and unwraps data keys. Everything else knows it only by an opaque KeyRef.",
  },
  {
    icon: "package",
    name: "Wrapped data key (DEK)",
    holder: "Stored next to the data",
    text: "Encrypts the secret itself and is kept only in wrapped form. Unwrapped for one authorized operation, never cached.",
  },
  {
    icon: "file-lock-2",
    name: "Your secret",
    holder: "Ciphertext wherever it is stored",
    text: "OAuth tokens, API keys and other provider credentials, encrypted under the data key.",
  },
];

// Each layer wraps the next one, so the nesting on screen is the nesting of the keys.
function renderLayer(depth: number): VNode | null {
  const layer = layers[depth];
  if (!layer) return null;

  return h("div", { class: "key-envelope__layer", style: { "--depth": depth } }, [
    h("div", { class: "key-envelope__head" }, [
      h("span", { class: "key-envelope__icon" }, [h(Icon, { name: layer.icon, size: 20 })]),
      h("div", [
        h("strong", layer.name),
        h("span", { class: "key-envelope__holder" }, layer.holder),
      ]),
    ]),
    h("p", layer.text),
    renderLayer(depth + 1),
  ]);
}

const Layers = () => renderLayer(0);
</script>

<template>
  <figure class="key-envelope">
    <Layers />

    <figcaption class="key-envelope__note">
      <span class="key-envelope__note-icon"><Icon name="flame" :size="20" /></span>
      <span>
        <strong>Crypto-shredding.</strong>
        Destroy the KEK and every secret under it becomes unreadable at once, without touching the stored data.
      </span>
    </figcaption>
  </figure>
</template>

<style scoped>
.key-envelope {
  --accent: #e11d48;
  margin: 24px 0 0;
}

.key-envelope :deep(.key-envelope__layer) {
  padding: 20px 22px 22px;
  border: 1.5px solid color-mix(in srgb, var(--accent) calc(16% + var(--depth) * 12%), var(--vp-c-divider));
  border-radius: calc(26px - var(--depth) * 3px);
  background:
    linear-gradient(160deg, color-mix(in srgb, var(--accent) calc(3% + var(--depth) * 4%), transparent), transparent 75%),
    var(--vp-c-bg);
  box-shadow: 0 18px 40px -30px rgba(36, 28, 90, 0.45);
}

.key-envelope :deep(.key-envelope__layer .key-envelope__layer) {
  margin-top: 18px;
}

.key-envelope :deep(.key-envelope__head) {
  display: flex;
  align-items: center;
  gap: 12px;
}

.key-envelope :deep(.key-envelope__head strong) {
  display: block;
  color: var(--vp-c-text-1);
  font-size: 15.5px;
  line-height: 1.3;
}

.key-envelope :deep(.key-envelope__holder) {
  color: var(--accent);
  font-size: 12px;
  font-weight: 600;
}

.key-envelope :deep(.key-envelope__icon) {
  display: grid;
  flex: none;
  place-items: center;
  width: 40px;
  height: 40px;
  border-radius: 12px;
  background: color-mix(in srgb, var(--accent) calc(10% + var(--depth) * 7%), transparent);
  color: var(--accent);
}

.key-envelope :deep(.key-envelope__layer p) {
  max-width: 560px;
  margin: 8px 0 0;
  color: var(--vp-c-text-2);
  font-size: 13.5px;
  line-height: 1.5;
}

.key-envelope__note {
  display: flex;
  align-items: flex-start;
  gap: 12px;
  margin-top: 20px;
  padding: 16px 20px;
  border-radius: 18px;
  background: var(--vp-c-bg-soft);
  color: var(--vp-c-text-2);
  font-size: 14px;
  line-height: 1.5;
}

.key-envelope__note strong {
  color: var(--vp-c-text-1);
}

.key-envelope__note-icon {
  margin-top: 1px;
  color: #f97316;
}

@media (max-width: 640px) {
  .key-envelope :deep(.key-envelope__layer) {
    padding: 14px 12px 14px;
  }
}
</style>
