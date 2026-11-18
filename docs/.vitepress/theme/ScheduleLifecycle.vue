<script setup lang="ts">
import Icon from "./Icon.vue";

type State = {
  name: string;
  icon: string;
  tone: "go" | "partial" | "stop" | "gone";
  fires: boolean;
  actions: string[];
  note: string;
};

const states: State[] = [
  {
    name: "Scheduled",
    icon: "play",
    tone: "go",
    fires: true,
    actions: ["Pause", "Remove"],
    note: "Fires on its timing rule. A schedule can also be created paused.",
  },
  {
    name: "Paused",
    icon: "pause",
    tone: "partial",
    fires: false,
    actions: ["Resume", "Remove"],
    note: "Kept, with its timer cleared. Resume sets the timer again.",
  },
  {
    name: "Completed",
    icon: "circle-check",
    tone: "gone",
    fires: false,
    actions: ["Remove"],
    note: "A recurrence rule that has run out of occurrences. It cannot be resumed.",
  },
  {
    name: "Removed",
    icon: "ban",
    tone: "stop",
    fires: false,
    actions: [],
    note: "Final. Its id is never reused, and its history stays.",
  },
];
</script>

<template>
  <figure class="schedule-lifecycle">
    <ol class="schedule-lifecycle__track">
      <li v-for="state in states" :key="state.name" :class="['schedule-lifecycle__state', `is-${state.tone}`]">
        <span class="schedule-lifecycle__icon"><Icon :name="state.icon" :size="20" /></span>
        <strong>{{ state.name }}</strong>
        <span :class="['schedule-lifecycle__pill', { 'is-on': state.fires }]">
          <Icon :name="state.fires ? 'check' : 'x'" :size="12" />
          {{ state.fires ? "Fires" : "Does not fire" }}
        </span>
        <span v-if="state.actions.length" class="schedule-lifecycle__actions">
          <span v-for="action in state.actions" :key="action" class="schedule-lifecycle__action">{{ action }}</span>
        </span>
        <span v-else class="schedule-lifecycle__none">No further changes</span>
        <span class="schedule-lifecycle__note">{{ state.note }}</span>
      </li>
    </ol>

    <figcaption class="schedule-lifecycle__caption">
      <span class="schedule-lifecycle__caption-icon"><Icon name="pin" :size="18" /></span>
      <span>
        <strong>A definition never changes.</strong>
        To send something different, or on a different rule, remove the schedule and create a new one under a new id.
      </span>
    </figcaption>
  </figure>
</template>

<style scoped>
.schedule-lifecycle {
  --accent: #65a30d;
  margin: 24px 0 0;
}

.schedule-lifecycle__track {
  position: relative;
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 10px;
  margin: 0;
  padding: 0;
  list-style: none;
}

.schedule-lifecycle__track::before {
  content: "";
  position: absolute;
  top: 37px;
  right: 12.5%;
  left: 12.5%;
  height: 2px;
  background: linear-gradient(90deg, #14b8a6, #f59e0b 35%, #6b7280 65%, #e11d48);
  opacity: 0.5;
}

.schedule-lifecycle__state {
  position: relative;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
  min-width: 0;
  margin: 0;
  padding: 16px 12px 18px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 20px;
  background: var(--vp-c-bg);
  text-align: center;
}

.schedule-lifecycle__state.is-go {
  --tone: #14b8a6;
}

.schedule-lifecycle__state.is-partial {
  --tone: #f59e0b;
}

.schedule-lifecycle__state.is-stop {
  --tone: #e11d48;
}

.schedule-lifecycle__state.is-gone {
  --tone: #6b7280;
}

.schedule-lifecycle__icon {
  display: grid;
  place-items: center;
  width: 42px;
  height: 42px;
  border-radius: 50%;
  background: color-mix(in srgb, var(--tone) 16%, var(--vp-c-bg));
  color: var(--tone);
  box-shadow: 0 0 0 5px var(--vp-c-bg);
}

.schedule-lifecycle__state strong {
  color: var(--vp-c-text-1);
  font-size: 14px;
  line-height: 1.3;
}

.schedule-lifecycle__pill {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 8px;
  border-radius: 999px;
  background: var(--vp-c-bg-soft);
  color: var(--vp-c-text-3);
  font-size: 11.5px;
  font-weight: 500;
}

.schedule-lifecycle__pill.is-on {
  background: color-mix(in srgb, #14b8a6 14%, transparent);
  color: color-mix(in srgb, #14b8a6 75%, var(--vp-c-text-1));
}

.schedule-lifecycle__actions {
  display: flex;
  flex-wrap: wrap;
  justify-content: center;
  gap: 4px;
}

.schedule-lifecycle__action {
  padding: 2px 8px;
  border: 1px solid color-mix(in srgb, var(--accent) 40%, var(--vp-c-divider));
  border-radius: 999px;
  background: color-mix(in srgb, var(--accent) 10%, transparent);
  color: var(--vp-c-text-1);
  font-size: 11.5px;
  font-weight: 500;
}

.schedule-lifecycle__none {
  color: var(--vp-c-text-3);
  font-size: 11.5px;
  font-style: italic;
}

.schedule-lifecycle__note {
  color: var(--vp-c-text-2);
  font-size: 12.5px;
  line-height: 1.45;
}

.schedule-lifecycle__caption {
  display: flex;
  align-items: flex-start;
  gap: 12px;
  margin-top: 16px;
  padding: 16px 20px;
  border: 1px dashed color-mix(in srgb, var(--accent) 40%, var(--vp-c-divider));
  border-radius: 18px;
  color: var(--vp-c-text-2);
  font-size: 14px;
  line-height: 1.5;
}

.schedule-lifecycle__caption strong {
  color: var(--vp-c-text-1);
}

.schedule-lifecycle__caption-icon {
  margin-top: 2px;
  color: var(--accent);
}

@media (max-width: 860px) {
  .schedule-lifecycle__track {
    grid-template-columns: minmax(0, 1fr);
  }

  .schedule-lifecycle__track::before {
    top: 10%;
    bottom: 10%;
    left: 37px;
    width: 2px;
    height: auto;
    background: linear-gradient(180deg, #14b8a6, #f59e0b 35%, #6b7280 65%, #e11d48);
  }

  .schedule-lifecycle__state {
    display: grid;
    grid-template-columns: 42px minmax(0, 1fr);
    justify-items: start;
    align-items: center;
    column-gap: 14px;
    row-gap: 6px;
    text-align: left;
  }

  .schedule-lifecycle__icon {
    grid-row: span 4;
    align-self: start;
  }

  .schedule-lifecycle__actions {
    justify-content: flex-start;
  }
}
</style>
