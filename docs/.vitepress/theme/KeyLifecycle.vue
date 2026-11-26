<script setup lang="ts">
import Icon from "./Icon.vue";

type State = {
  name: string;
  icon: string;
  tone: "go" | "partial" | "stop" | "gone";
  wrap: boolean;
  unwrap: boolean;
  note: string;
};

const states: State[] = [
  { name: "Active", icon: "power", tone: "go", wrap: true, unwrap: true, note: "The only state that protects new secrets." },
  {
    name: "Decrypt-only",
    icon: "lock",
    tone: "partial",
    wrap: false,
    unwrap: true,
    note: "Retired from new work. Existing secrets stay readable, so you can migrate away.",
  },
  { name: "Disabled", icon: "ban", tone: "stop", wrap: false, unwrap: false, note: "Use is suspended." },
  {
    name: "Pending destruction",
    icon: "hourglass",
    tone: "stop",
    wrap: false,
    unwrap: false,
    note: "Scheduled and reviewed, never instant.",
  },
  {
    name: "Destroyed",
    icon: "flame",
    tone: "gone",
    wrap: false,
    unwrap: false,
    note: "Gone for good. Everything it protected is unreadable.",
  },
];
</script>

<template>
  <figure class="key-lifecycle">
    <ol class="key-lifecycle__track">
      <li v-for="state in states" :key="state.name" :class="['key-lifecycle__state', `is-${state.tone}`]">
        <span class="key-lifecycle__icon"><Icon :name="state.icon" :size="20" /></span>
        <strong>{{ state.name }}</strong>
        <span class="key-lifecycle__ops">
          <span :class="['key-lifecycle__op', { 'is-on': state.wrap }]">
            <Icon :name="state.wrap ? 'check' : 'x'" :size="12" /> New secrets
          </span>
          <span :class="['key-lifecycle__op', { 'is-on': state.unwrap }]">
            <Icon :name="state.unwrap ? 'check' : 'x'" :size="12" /> Read secrets
          </span>
        </span>
        <span class="key-lifecycle__note">{{ state.note }}</span>
      </li>
    </ol>

    <figcaption class="key-lifecycle__customer">
      <span class="key-lifecycle__customer-icon"><Icon name="key-round" :size="18" /></span>
      <span>
        <strong>With your own key, your provider decides.</strong>
        If it denies use, every operation stops right away, with no review step. Restore access and it works again.
        Destroy the key there and it is permanent.
      </span>
    </figcaption>
  </figure>
</template>

<style scoped>
.key-lifecycle {
  margin: 24px 0 0;
}

.key-lifecycle__track {
  position: relative;
  display: grid;
  grid-template-columns: repeat(5, minmax(0, 1fr));
  gap: 10px;
  margin: 0;
  padding: 0;
  list-style: none;
}

.key-lifecycle__track::before {
  content: "";
  position: absolute;
  top: 37px;
  right: 10%;
  left: 10%;
  height: 2px;
  background: linear-gradient(90deg, #14b8a6, #f59e0b 30%, #e11d48 70%, var(--vp-c-text-3));
  opacity: 0.5;
}

.key-lifecycle__state {
  position: relative;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
  margin: 0;
  padding: 16px 12px 18px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 20px;
  background: var(--vp-c-bg);
  text-align: center;
}

.key-lifecycle__state.is-go {
  --tone: #14b8a6;
}

.key-lifecycle__state.is-partial {
  --tone: #f59e0b;
}

.key-lifecycle__state.is-stop {
  --tone: #e11d48;
}

.key-lifecycle__state.is-gone {
  --tone: #6b7280;
}

.key-lifecycle__icon {
  display: grid;
  place-items: center;
  width: 42px;
  height: 42px;
  border-radius: 50%;
  background: color-mix(in srgb, var(--tone) 16%, var(--vp-c-bg));
  color: var(--tone);
  box-shadow: 0 0 0 5px var(--vp-c-bg);
}

.key-lifecycle__state strong {
  color: var(--vp-c-text-1);
  font-size: 14px;
  line-height: 1.3;
}

.key-lifecycle__ops {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.key-lifecycle__op {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 8px;
  border-radius: 999px;
  background: var(--vp-c-bg-soft);
  color: var(--vp-c-text-3);
  font-size: 11.5px;
  font-weight: 500;
  text-decoration: line-through;
  text-decoration-color: color-mix(in srgb, var(--vp-c-text-3) 50%, transparent);
}

.key-lifecycle__op.is-on {
  background: color-mix(in srgb, #14b8a6 14%, transparent);
  color: color-mix(in srgb, #14b8a6 75%, var(--vp-c-text-1));
  text-decoration: none;
}

.key-lifecycle__note {
  color: var(--vp-c-text-2);
  font-size: 12.5px;
  line-height: 1.45;
}

.key-lifecycle__customer {
  display: flex;
  align-items: flex-start;
  gap: 12px;
  margin-top: 16px;
  padding: 16px 20px;
  border: 1px dashed color-mix(in srgb, #e11d48 40%, var(--vp-c-divider));
  border-radius: 18px;
  color: var(--vp-c-text-2);
  font-size: 14px;
  line-height: 1.5;
}

.key-lifecycle__customer strong {
  color: var(--vp-c-text-1);
}

.key-lifecycle__customer-icon {
  margin-top: 2px;
  color: #e11d48;
}

@media (max-width: 860px) {
  .key-lifecycle__track {
    grid-template-columns: 1fr;
  }

  .key-lifecycle__track::before {
    top: 10%;
    bottom: 10%;
    left: 37px;
    width: 2px;
    height: auto;
    background: linear-gradient(180deg, #14b8a6, #f59e0b 30%, #e11d48 70%, var(--vp-c-text-3));
  }

  .key-lifecycle__state {
    display: grid;
    grid-template-columns: 42px 1fr;
    align-items: center;
    column-gap: 14px;
    row-gap: 6px;
    text-align: left;
  }

  .key-lifecycle__icon {
    grid-row: span 3;
    align-self: start;
  }

  .key-lifecycle__ops {
    flex-direction: row;
    flex-wrap: wrap;
  }
}
</style>
