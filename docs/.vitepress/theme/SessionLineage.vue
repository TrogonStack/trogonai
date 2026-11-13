<script setup lang="ts">
import Icon from "./Icon.vue";

type Shape = "new" | "fork" | "child" | "recover";

const ways: { shape: Shape; name: string; text: string }[] = [
  { shape: "new", name: "Start", text: "A fresh session with an empty history." },
  {
    shape: "fork",
    name: "Fork",
    text: "Branch off an existing session to try another way. The original stays as it was.",
  },
  {
    shape: "child",
    name: "Delegate",
    text: "Hand part of the work to another agent in a child session, linked to its parent.",
  },
  {
    shape: "recover",
    name: "Recover",
    text: "Salvage a damaged session into a new one. Running the salvage twice never makes a second copy.",
  },
];

const edits = [
  { icon: "rotate-ccw", name: "Rewind", text: "Hides later turns from readers and the next run. What ran still counts." },
  { icon: "layers", name: "Compact", text: "Marks a summary point so the model reads less. The full history stays." },
  { icon: "archive", name: "Rename, archive, hide", text: "Each is a new event. Hiding a session deletes nothing." },
];
</script>

<template>
  <figure class="session-lineage">
    <ul class="session-lineage__ways">
      <li v-for="way in ways" :key="way.shape">
        <svg class="session-lineage__glyph" viewBox="0 0 160 64" aria-hidden="true">
          <template v-if="way.shape === 'new'">
            <line x1="22" y1="32" x2="138" y2="32" class="is-line" />
            <circle cx="22" cy="32" r="7" class="is-start" />
            <circle cx="62" cy="32" r="4.5" />
            <circle cx="100" cy="32" r="4.5" />
            <circle cx="138" cy="32" r="4.5" />
          </template>
          <template v-else-if="way.shape === 'fork'">
            <line x1="14" y1="22" x2="146" y2="22" class="is-line is-muted" />
            <path d="M62 22 C 82 22, 82 46, 102 46 L 146 46" class="is-line" fill="none" />
            <circle cx="14" cy="22" r="4.5" class="is-muted" />
            <circle cx="38" cy="22" r="4.5" class="is-muted" />
            <circle cx="62" cy="22" r="6" class="is-start" />
            <circle cx="104" cy="22" r="4.5" class="is-muted" />
            <circle cx="146" cy="22" r="4.5" class="is-muted" />
            <circle cx="120" cy="46" r="4.5" />
            <circle cx="146" cy="46" r="4.5" />
          </template>
          <template v-else-if="way.shape === 'child'">
            <line x1="14" y1="18" x2="146" y2="18" class="is-line is-muted" />
            <line x1="62" y1="18" x2="62" y2="46" class="is-line is-dashed" />
            <line x1="62" y1="46" x2="134" y2="46" class="is-line" />
            <circle cx="14" cy="18" r="4.5" class="is-muted" />
            <circle cx="62" cy="18" r="6" class="is-muted" />
            <circle cx="104" cy="18" r="4.5" class="is-muted" />
            <circle cx="146" cy="18" r="4.5" class="is-muted" />
            <circle cx="62" cy="46" r="6" class="is-start" />
            <circle cx="98" cy="46" r="4.5" />
            <circle cx="134" cy="46" r="4.5" />
          </template>
          <template v-else>
            <line x1="14" y1="18" x2="70" y2="18" class="is-line is-broken" />
            <path d="M80 18 l8 -6 m-8 0 l8 6" class="is-x" />
            <path d="M14 18 C 30 46, 40 46, 60 46 L 146 46" class="is-line" fill="none" />
            <circle cx="14" cy="18" r="4.5" class="is-muted" />
            <circle cx="42" cy="18" r="4.5" class="is-muted" />
            <circle cx="70" cy="18" r="4.5" class="is-muted" />
            <circle cx="60" cy="46" r="6" class="is-start" />
            <circle cx="104" cy="46" r="4.5" />
            <circle cx="146" cy="46" r="4.5" />
          </template>
        </svg>
        <strong>{{ way.name }}</strong>
        <span>{{ way.text }}</span>
      </li>
    </ul>

    <figcaption class="session-lineage__edits">
      <span class="session-lineage__edits-title">Inside a session, history is only ever added to</span>
      <ul>
        <li v-for="edit in edits" :key="edit.name">
          <span class="session-lineage__edit-icon"><Icon :name="edit.icon" :size="16" /></span>
          <span>
            <strong>{{ edit.name }}</strong>
            <span>{{ edit.text }}</span>
          </span>
        </li>
      </ul>
    </figcaption>
  </figure>
</template>

<style scoped>
.session-lineage {
  --amber: #f59e0b;
  margin: 24px 0 0;
}

.session-lineage__ways {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(min(100%, 200px), 1fr));
  gap: 12px;
  margin: 0;
  padding: 0;
  list-style: none;
}

.session-lineage__ways li {
  display: flex;
  flex-direction: column;
  gap: 4px;
  margin: 0;
  padding: 16px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 20px;
  background: color-mix(in srgb, var(--vp-c-bg) 86%, transparent);
}

.session-lineage__ways strong {
  margin-top: 6px;
  color: var(--vp-c-text-1);
  font-size: 15px;
}

.session-lineage__ways span {
  color: var(--vp-c-text-2);
  font-size: 13px;
  line-height: 1.5;
}

.session-lineage__glyph {
  width: 100%;
  height: 64px;
  border-radius: 14px;
  background: var(--vp-c-bg-soft);
}

.session-lineage__glyph circle {
  fill: var(--amber);
}

.session-lineage__glyph circle.is-start {
  fill: var(--vp-c-bg);
  stroke: var(--amber);
  stroke-width: 3;
}

.session-lineage__glyph circle.is-muted {
  fill: var(--vp-c-text-3);
}

.session-lineage__glyph .is-line {
  stroke: var(--amber);
  stroke-width: 2.5;
  stroke-linecap: round;
}

.session-lineage__glyph .is-line.is-muted,
.session-lineage__glyph .is-line.is-broken {
  stroke: var(--vp-c-text-3);
}

.session-lineage__glyph .is-line.is-dashed {
  stroke-dasharray: 4 4;
}

.session-lineage__glyph .is-x {
  stroke: #e11d48;
  stroke-width: 2.5;
  stroke-linecap: round;
}

.session-lineage__edits {
  margin-top: 16px;
  padding: 18px 20px;
  border-radius: 20px;
  background: var(--vp-c-bg-soft);
}

.session-lineage__edits-title {
  color: var(--vp-c-text-1);
  font-size: 14px;
  font-weight: 600;
}

.session-lineage__edits ul {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(min(100%, 220px), 1fr));
  gap: 10px;
  margin: 12px 0 0;
  padding: 0;
  list-style: none;
}

.session-lineage__edits li {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  margin: 0;
}

.session-lineage__edit-icon {
  display: grid;
  flex: none;
  place-items: center;
  width: 30px;
  height: 30px;
  border-radius: 10px;
  background: color-mix(in srgb, var(--amber) 16%, transparent);
  color: color-mix(in srgb, var(--amber) 80%, var(--vp-c-text-1));
}

.session-lineage__edits strong {
  display: block;
  color: var(--vp-c-text-1);
  font-size: 13.5px;
}

.session-lineage__edits li span span {
  display: block;
  color: var(--vp-c-text-2);
  font-size: 12.5px;
  line-height: 1.45;
}
</style>
