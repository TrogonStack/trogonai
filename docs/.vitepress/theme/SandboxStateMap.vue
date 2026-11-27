<script setup lang="ts">
import { computed, ref, useId } from "vue";
import Icon from "./Icon.vue";

const branches = [
  {
    action: "Pause", icon: "pause", hint: "Keep this lease", state: "Paused", next: "Resume the same lease",
    detail: "The name stays attached to this lease. Resume takes it back to Running when the worker resumes the guest.",
    color: "teal",
  },
  {
    action: "Stop", icon: "power", hint: "End this run", state: "Stopping", next: "Start a fresh lease later",
    detail: "When the bound run ends, the sandbox becomes Stopped. Its name stays available for the next start.",
    color: "amber",
  },
  {
    action: "Delete", icon: "x", hint: "Request cleanup", state: "Deleting", next: "Verify cleanup",
    detail: "The sandbox enters Deleting and retains the bound lease for cleanup. The worker still has to finish removing native resources.",
    color: "rose",
  },
] as const;

const groupName = useId();
const selection = ref(0);
const selected = computed(() => branches[selection.value]);
const motionPaused = ref(false);

function connection(index: number, side: "in" | "out", narrow: boolean) {
  const center = 150;
  const branch = 50 + index * 100;
  const from = side === "in" ? center : branch;
  const to = side === "in" ? branch : center;
  return narrow
    ? `M ${from} 0 C ${from} 50, ${to} 50, ${to} 100`
    : `M 0 ${from} C 50 ${from}, 50 ${to}, 100 ${to}`;
}
</script>

<template>
  <figure class="state-map" :class="[{ 'motion-paused': motionPaused }, selected.color]" aria-label="Choose a lifecycle path for a named sandbox">
    <header class="identity">
      <span class="identity-icon"><Icon name="boxes" :size="20" /></span>
      <span class="name"><code>pr-review-workspace</code><small>Example named sandbox</small></span>
      <button class="motion" type="button" :aria-label="motionPaused ? 'Play lines' : 'Pause lines'" @click="motionPaused = !motionPaused">
        <Icon :name="motionPaused ? 'play' : 'pause'" :size="14" />
        <span>{{ motionPaused ? 'Play lines' : 'Pause lines' }}</span>
      </button>
    </header>

    <div class="diagram">
      <section class="node origin">
        <span class="node-icon"><Icon name="terminal" :size="18" /></span>
        <strong>Running</strong>
        <small>The worker holds the active lease.</small>
      </section>

      <template v-for="side in (['in', 'out'] as const)" :key="side">
        <svg v-for="layout in (['wide', 'narrow'] as const)" :key="layout" class="wires"
          :class="[layout, side]" :viewBox="layout === 'wide' ? '0 0 100 300' : '0 0 300 100'"
          preserveAspectRatio="none" aria-hidden="true">
          <path v-for="(_, index) in branches" :key="index" class="wire"
            :data-chosen="selection === index" :d="connection(index, side, layout === 'narrow')" />
          <path class="signal" :d="connection(selection, side, layout === 'narrow')" />
        </svg>
      </template>

      <fieldset class="choices">
        <legend class="visually-hidden">Choose a sandbox action</legend>
        <label v-for="(branch, index) in branches" :key="branch.action" class="choice" :class="{ active: index === selection }">
          <input v-model="selection" type="radio" :name="groupName" :value="index" />
          <Icon :name="branch.icon" :size="15" />
          <span><strong>{{ branch.action }}</strong><small>{{ branch.hint }}</small></span>
        </label>
      </fieldset>

      <section class="node result" aria-live="polite" aria-atomic="true">
        <div v-for="(branch, index) in branches" :key="branch.action" class="result-face"
          :class="{ visible: selection === index }" :aria-hidden="selection !== index">
          <span class="node-icon"><Icon :name="branch.icon" :size="18" /></span>
          <strong>{{ branch.state }}</strong>
          <small>{{ branch.next }}</small>
        </div>
      </section>
    </div>

    <figcaption>
      <Icon name="git-branch" :size="16" />
      <div class="notes">
        <p v-for="(branch, index) in branches" :key="branch.action" :class="{ visible: selection === index }"
          :aria-hidden="selection !== index">{{ branch.detail }}</p>
      </div>
    </figcaption>
  </figure>
</template>

<style scoped>
.state-map {
  --tone: #0c816e;
  --wire: color-mix(in srgb, var(--vp-c-text-3) 40%, var(--vp-c-bg));
  container-type: inline-size;
  margin: 18px 0 24px;
  overflow: hidden;
  border: 1px solid var(--vp-c-divider);
  border-radius: 22px;
  background: var(--vp-c-bg);
  box-shadow: 0 22px 60px -44px rgb(41 40 83 / 45%);
}
.state-map.amber { --tone: #a55f0a; }
.state-map.rose { --tone: #b74962; }
.dark .state-map { --tone: #5ddbbb; }
.dark .state-map.amber { --tone: #edb867; }
.dark .state-map.rose { --tone: #ff9ab2; }
.identity, figcaption { display: flex; align-items: center; gap: 12px; padding: 12px 18px; }
.identity { border-bottom: 1px solid var(--vp-c-divider); background: var(--vp-c-bg-soft); }
.identity-icon, .node-icon {
  display: grid; place-items: center; flex: none; width: 32px; height: 32px; border-radius: 10px;
  color: var(--tone); background: color-mix(in srgb, var(--tone) 12%, var(--vp-c-bg));
}
.name { display: grid; gap: 1px; }
.name code { padding: 0; color: var(--vp-c-text-1); background: transparent; font-size: 14px; font-weight: 600; }
.state-map small { display: block; color: var(--vp-c-text-2); font-size: 12.5px; line-height: 1.4; }
.motion {
  display: inline-flex; align-items: center; justify-content: center; gap: 5px; margin-left: auto;
  min-height: 32px; padding: 3px 9px; border: 1px solid var(--vp-c-divider); border-radius: 8px;
  color: var(--vp-c-text-2); background: var(--vp-c-bg); font-size: 12px;
}
.motion:hover, .choice:hover { border-color: var(--tone); }
.motion:focus-visible, .choice:has(input:focus-visible) { outline: 2px solid var(--tone); outline-offset: 2px; }
.diagram {
  position: relative;
  display: grid; grid-template-areas: "origin incoming choices outgoing result";
  grid-template-columns: minmax(0, 1fr) minmax(36px, .4fr) minmax(0, 1.12fr) minmax(36px, .4fr) minmax(0, 1fr);
  align-items: center; padding: 20px clamp(12px, 3cqi, 32px);
  background: radial-gradient(var(--vp-c-divider) .75px, transparent .85px) 0 0 / 13px 13px;
}
.node, .choice { border: 1px solid var(--vp-c-divider); border-radius: 14px; background: var(--vp-c-bg); }
.node { padding: 14px; }
.origin, .result-face { display: grid; justify-items: start; align-content: center; gap: 5px; }
.origin { grid-area: origin; }
.result { grid-area: result; display: grid; border-color: var(--tone); box-shadow: 0 0 0 3px color-mix(in srgb, var(--tone) 10%, transparent); }
.result-face, .notes p { grid-area: 1 / 1; visibility: hidden; }
.visible, .notes p.visible { visibility: visible; }
.wires { position: absolute; inset: 0; width: 100%; height: 100%; overflow: visible; pointer-events: none; }
.wires.in { grid-area: incoming; }
.wires.out { grid-area: outgoing; }
.wires.narrow { display: none; }
.wires path { fill: none; vector-effect: non-scaling-stroke; }
.wire { stroke: var(--wire); stroke-width: 1.5; }
.wire[data-chosen="true"] { stroke: color-mix(in srgb, var(--tone) 30%, var(--vp-c-bg)); stroke-width: 3; }
.signal { stroke: var(--tone); stroke-width: 2.75; stroke-linecap: round; stroke-dasharray: 6 8; animation: sandbox-line 1.15s linear infinite; }
@keyframes sandbox-line { to { stroke-dashoffset: -28; } }
.motion-paused .signal { animation-play-state: paused; }
.choices {
  display: grid; grid-area: choices; grid-template-rows: repeat(3, 1fr);
  min-width: 0; margin: 0; padding: 0; border: 0;
}
.choice {
  position: relative; display: flex; gap: 9px; align-items: center; margin: 4px 0;
  padding: 8px 10px; cursor: pointer; font-size: 14px; line-height: 1.35;
}
.choice input { position: absolute; inset: 0; width: 100%; height: 100%; margin: 0; opacity: 0; cursor: pointer; }
.choice.active { color: var(--tone); border-color: var(--tone); background: color-mix(in srgb, var(--tone) 6%, var(--vp-c-bg)); }
.visually-hidden { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); }
figcaption { align-items: flex-start; border-top: 1px solid var(--vp-c-divider); color: var(--vp-c-text-2); font-size: 13px; line-height: 1.5; }
figcaption > svg { margin-top: 2px; color: var(--tone); }
.notes { display: grid; }
.notes p { margin: 0; line-height: inherit; }
@media (prefers-reduced-motion: reduce) {
  .signal { animation: none; }
  .motion { display: none; }
}
@container (max-width: 600px) {
  .diagram {
    grid-template-areas: "origin" "incoming" "choices" "outgoing" "result"; grid-template-columns: minmax(0, 1fr);
    grid-template-rows: auto 32px auto 32px auto; padding: 14px 8px;
  }
  .origin, .result { justify-self: center; width: min(100%, 224px); }
  .origin, .result-face { grid-template-columns: auto 1fr; column-gap: 9px; }
  .origin .node-icon, .result-face .node-icon { grid-row: span 2; }
  .origin small, .result-face small { grid-column: 2; }
  .node { padding: 9px 12px; }
  .wires.wide { display: none; }
  .wires.narrow { display: block; }
  .choices { grid-template-rows: 1fr; grid-template-columns: repeat(3, minmax(0, 1fr)); }
  .choice { flex-direction: column; justify-content: center; gap: 3px; min-height: 52px; margin: 0 2px; padding: 5px 2px; }
  .choice small { display: none; }
}
@container (max-width: 375px) {
  .identity, figcaption { gap: 8px; padding: 10px; }
  .motion span { display: none; }
}
</style>
