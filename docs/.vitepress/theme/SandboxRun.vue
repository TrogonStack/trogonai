<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, useId, watch } from "vue";
import Icon from "./Icon.vue";

const eventTypes = {
  command: { icon: "terminal", label: "Command" },
  stdout: { icon: "list", label: "Output" },
  lease: { icon: "clock", label: "Lease" },
  files: { icon: "layers", label: "Workspace" },
  process: { icon: "activity", label: "Process" },
  pass: { icon: "circle-check", label: "Passed" },
  fail: { icon: "x", label: "Failure" },
  cleanup: { icon: "badge-check", label: "Verified" },
} as const;

type OutputLine = { kind: keyof typeof eventTypes; text: string };
type Stage = {
  label: string;
  icon: string;
  title: string;
  nextLabel: string;
  note: string;
  code: "original" | "patch" | "fixed" | "closed";
  lease: string;
  files: string;
  process: string;
  output: OutputLine[];
};

const stages = [
  {
    label: "Request", icon: "send", title: "Open a clean checkout", nextLabel: "Run the test",
    note: "The agent requests a Python workspace. The broker admits the lease and a Daytona worker prepares the project.",
    code: "original", lease: "Ready", files: "Checked out", process: "Idle",
    output: [
      { kind: "lease", text: "Lease admitted · Daytona" },
      { kind: "files", text: "Git workspace ready in /workspace" },
      { kind: "command", text: "ls" },
      { kind: "stdout", text: "shipping.py  test_shipping.py" },
    ],
  },
  {
    label: "Test", icon: "terminal", title: "Free shipping fails at exactly $50", nextLabel: "Apply the fix",
    note: "An order of exactly 5,000 cents should qualify for free shipping. The comparison excludes it.",
    code: "original", lease: "Ready", files: "Unmodified", process: "Test exited · 1",
    output: [
      { kind: "command", text: "pytest -q" },
      { kind: "fail", text: "FAILED test_shipping.py::test_threshold" },
      { kind: "stdout", text: "assert ships_free(5000) is True" },
      { kind: "fail", text: "E   assert False is True" },
    ],
  },
  {
    label: "Fix", icon: "file-diff", title: "One character turns the test green", nextLabel: "Pause the sandbox",
    note: "The agent edits the comparison and reruns the test in the same workspace. It also starts a detached HTTP server.",
    code: "patch", lease: "Ready", files: "Edited", process: "HTTP server running",
    output: [
      { kind: "command", text: "pytest -q" },
      { kind: "pass", text: "1 passed" },
      { kind: "command", text: "python -m http.server 3000" },
      { kind: "process", text: "Detached command running" },
    ],
  },
  {
    label: "Pause", icon: "pause", title: "The files stay when the guest stops", nextLabel: "Resume the sandbox",
    note: "The agent pauses the Daytona sandbox. The disk keeps the edit. The HTTP server stops with the other guest processes.",
    code: "fixed", lease: "Paused", files: "Retained on disk", process: "HTTP server stopped",
    output: [
      { kind: "lease", text: "SDK requested pause" },
      { kind: "lease", text: "Daytona guest stopped" },
      { kind: "process", text: "HTTP server process stopped" },
    ],
  },
  {
    label: "Resume", icon: "play", title: "The fix survives the pause", nextLabel: "Delete the sandbox",
    note: "The agent resumes this lease and runs the test again. The edited file is there. Starting the HTTP server again takes a new command.",
    code: "fixed", lease: "Ready again", files: "Edit preserved", process: "HTTP server stopped",
    output: [
      { kind: "lease", text: "Daytona guest resumed" },
      { kind: "command", text: "pytest -q" },
      { kind: "pass", text: "1 passed" },
    ],
  },
  {
    label: "Delete", icon: "power", title: "Release the lease and confirm cleanup", nextLabel: "Replay from request",
    note: "The agent requests deletion. In this example, the operator also verifies the guest is gone at the provider.",
    code: "closed", lease: "Released", files: "Guest removed", process: "Guest removed",
    output: [
      { kind: "lease", text: "SDK requested deletion" },
      { kind: "lease", text: "Broker reported release" },
      { kind: "cleanup", text: "Provider check: guest removed" },
    ],
  },
] as const satisfies readonly Stage[];

const groupName = useId();
const eventDurationMs = 1250;
const current = ref(1);
const stage = computed(() => stages[current.value]);
const atEnd = computed(() => current.value === stages.length - 1);
const playback = ref<"idle" | "playing" | "paused" | "ended">("idle");
const playhead = ref(stages[current.value].output.length - 1);
const precedingEvents = computed(() => stages.slice(0, current.value).reduce((count, item) => count + item.output.length, 0));
const playbackLabels = { idle: "Play run", playing: "Pause playback", paused: "Resume playback", ended: "Replay run" } as const;
const figure = ref<HTMLElement>();
const figureVisible = ref(false);
const track = ref<HTMLDivElement>();
const trackVisible = ref(false);
let observer: IntersectionObserver | undefined;

onMounted(() => {
  observer = new IntersectionObserver((entries) => {
    for (const entry of entries) {
      if (entry.target === track.value) trackVisible.value = entry.isIntersecting;
      if (entry.target === figure.value) {
        figureVisible.value = entry.isIntersecting;
        if (!entry.isIntersecting) pausePlayback();
      }
    }
  });
  if (track.value) observer.observe(track.value);
  if (figure.value) observer.observe(figure.value);
  document.addEventListener("visibilitychange", pauseWhenHidden);
});
onUnmounted(() => {
  observer?.disconnect();
  document.removeEventListener("visibilitychange", pauseWhenHidden);
});

function pauseWhenHidden() {
  if (document.hidden) pausePlayback();
}

function pausePlayback() {
  if (playback.value === "playing") playback.value = "paused";
}

function selectStage(index: number) {
  playback.value = "idle";
  current.value = index;
  playhead.value = stages[index].output.length - 1;
}

function togglePlayback() {
  if (playback.value === "playing") {
    pausePlayback();
    return;
  }
  if (playback.value !== "paused") {
    current.value = 0;
    playhead.value = 0;
  }
  playback.value = "playing";
}

watch([playback, current, playhead], (_, __, onCleanup) => {
  if (playback.value !== "playing") return;
  const timer = window.setTimeout(() => {
    if (document.hidden || !figureVisible.value) {
      pausePlayback();
      return;
    }
    if (playhead.value < stage.value.output.length - 1) {
      playhead.value++;
    } else if (atEnd.value) {
      playback.value = "ended";
    } else {
      current.value++;
      playhead.value = 0;
    }
  }, eventDurationMs);
  onCleanup(() => window.clearTimeout(timer));
});
</script>

<template>
  <figure ref="figure" class="run" aria-label="Illustrated Daytona sandbox run">
    <figcaption class="run-bar">
      <span class="project">Daytona <span class="slash">/</span> checkout</span>
      <div class="playback-tools">
        <span class="example">Illustrative run</span>
        <button class="playback" type="button" @click="togglePlayback">
          <Icon :name="playback === 'playing' ? 'pause' : 'play'" :size="14" />
          {{ playbackLabels[playback] }}
        </button>
      </div>
    </figcaption>

    <fieldset class="scrubber">
      <legend class="sr-only">Select a stage of the run</legend>
      <div ref="track" class="track" :class="{ visible: trackVisible }">
        <label v-for="(item, index) in stages" :key="item.label"
          class="station" :class="{ done: index < current, selected: index === current }">
          <svg v-if="index > 0" class="incoming" aria-hidden="true" focusable="false">
            <line x1="0" y1="3" x2="100%" y2="3" />
          </svg>
          <input type="radio" :name="groupName" :checked="current === index" @change="selectStage(index)" class="sr-only" />
          <span class="node"><Icon :name="item.icon" :size="16" /></span>
          <span class="station-label">{{ item.label }}</span>
        </label>
      </div>
    </fieldset>

    <div class="scene" :aria-live="playback === 'playing' ? 'off' : 'polite'">
      <div class="scene-copy">
        <span class="position">Step {{ current + 1 }} of {{ stages.length }}</span>
        <strong>{{ stage.title }}</strong>
        <p>{{ stage.note }}</p>
      </div>

      <div class="workbench">
        <section class="pane editor" aria-label="Workspace file">
          <div class="pane-heading"><Icon name="code" :size="15" />
            <span>shipping.py</span>
            <small v-if="stage.code === 'patch'">diff</small>
            <small v-if="stage.label === 'Pause'">last read</small>
          </div>
          <div class="code-area" v-if="stage.code !== 'closed'">
            <div class="code-line"><i>1</i><code><em>def</em> ships_free(subtotal_cents):</code></div>
            <div v-if="stage.code === 'original' || stage.code === 'patch'"
              class="code-line" :class="{ removed: stage.code === 'patch', fault: stage.label === 'Test' }">
              <i>{{ stage.code === 'patch' ? '−' : '2' }}</i>
              <code>{{ '    ' }}<em>return</em> subtotal_cents <b>&gt;</b> <u>5000</u></code>
            </div>
            <div v-if="stage.code === 'patch' || stage.code === 'fixed'" class="code-line" :class="{ added: stage.code === 'patch' }">
              <i>{{ stage.code === 'patch' ? '+' : '2' }}</i>
              <code>{{ '    ' }}<em>return</em> subtotal_cents <b>&gt;=</b> <u>5000</u></code>
            </div>
          </div>
          <p v-else class="closed"><Icon name="power" :size="18" />Workspace closed</p>
        </section>

        <section class="pane console" aria-label="Example command output and lease events">
          <div class="pane-heading"><Icon name="history" :size="15" /><span>Run transcript</span></div>
          <ol class="output" :class="{ playing: playback === 'playing' }"
            :style="{ '--event-duration': eventDurationMs + 'ms' }" :start="precedingEvents + 1">
            <li v-for="(line, index) in stage.output" :key="line.text" class="tape-row" :class="[line.kind, { upcoming: index > playhead }]"
              :aria-current="index === playhead ? 'step' : undefined">
              <span class="ordinal" aria-hidden="true">{{ precedingEvents + index + 1 }}</span>
              <span class="event-icon"><Icon :name="eventTypes[line.kind].icon" :size="14" /></span>
              <span class="entry"><strong>{{ eventTypes[line.kind].label }}</strong><span>{{ line.text }}</span></span>
            </li>
          </ol>
        </section>
      </div>

      <p class="outcome-label">After {{ stage.label.toLowerCase() }}</p>
      <dl class="instruments">
        <div><dt><Icon name="clock" :size="14" />Lease</dt><dd>{{ stage.lease }}</dd></div>
        <div><dt><Icon name="layers" :size="14" />Files</dt><dd>{{ stage.files }}</dd></div>
        <div><dt><Icon name="activity" :size="14" />Process</dt><dd>{{ stage.process }}</dd></div>
      </dl>
    </div>
    <div class="transport">
      <a href="https://github.com/TrogonStack/trogonai/tree/main/rsworkspace/crates/sandbox/trogon-sandbox-daytona-worker#behavior">Daytona worker behavior</a>
      <button type="button" @click="selectStage(atEnd ? 0 : current + 1)">
        {{ stage.nextLabel }}
        <Icon :name="atEnd ? 'rotate-ccw' : 'arrow-right'" :size="16" />
      </button>
    </div>
  </figure>
</template>

<style scoped>
.run {
  --cyan: #65e0c6;
  --muted: #a7b8bb;
  --edge: #35474b;
  overflow: hidden;
  min-width: 0;
  margin: 18px 0;
  border: 1px solid #35494b;
  border-radius: 20px;
  color: #e7f0ed;
  background: #122326;
  font-size: 14px;
  line-height: 1.45;
}
.run-bar, .transport, .pane-heading, .project, .instruments dt, .transport button {
  display: flex;
  align-items: center;
}
.run-bar, .transport {
  justify-content: space-between;
  gap: 10px;
}
.run-bar {
  flex-wrap: wrap;
  padding: 15px 22px;
  border-bottom: 1px solid var(--edge);
  background: #172b2e;
}
.project { gap: 8px; font-weight: 650; }
.playback-tools, .playback { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.playback {
  padding: 5px 10px;
  border: 1px solid #56716f;
  border-radius: 7px;
  background: #203a3b;
  color: var(--cyan);
  font-size: 12px;
  font-weight: 600;
}
.playback:hover { border-color: var(--cyan); }
.playback:focus-visible { outline: 2px solid #f6d982; outline-offset: 2px; }
.slash { color: #7d9699; }
.example, .position, .pane-heading, .station-label, .instruments dt, .code-area, .output {
  font-family: var(--vp-font-family-mono);
}
.example, .position {
  color: var(--muted);
  font-size: 11px;
  letter-spacing: .04em;
}
.scrubber {
  min-width: 0;
  margin: 0;
  padding: 22px 16px 14px;
  border: 0;
  background: radial-gradient(ellipse at 25% 10%, #1f3d3d, transparent 68%);
}
.sr-only {
  position: absolute;
  width: 1px;
  height: 1px;
  clip-path: inset(50%);
  overflow: hidden;
  white-space: nowrap;
}
.track {
  display: grid;
  position: relative;
  grid-template-columns: repeat(6, minmax(0, 1fr));
}
.incoming {
  position: absolute;
  left: calc(-50% + 16px);
  top: 13px;
  width: calc(100% - 32px);
  height: 6px;
  pointer-events: none;
  stroke: #526e70;
  stroke-width: 2.5;
  stroke-linecap: round;
}
.station.done .incoming { stroke: var(--cyan); }
.station.selected .incoming line {
  stroke: var(--cyan);
  stroke-dasharray: 8 6;
}
.track.visible .station.selected .incoming line {
  animation: incoming-flow 1.4s linear 3;
}
@keyframes incoming-flow {
  to { stroke-dashoffset: -28; }
}
@media (prefers-reduced-motion: reduce) {
  .track.visible .station.selected .incoming line { animation: none; }
}
.station {
  position: relative;
  display: grid;
  justify-items: center;
  gap: 7px;
  cursor: pointer;
}
.node {
  display: grid;
  place-items: center;
  width: 32px;
  height: 32px;
  border: 1px solid #617a7b;
  border-radius: 50%;
  color: #c2d0d0;
  background: #1d3033;
}
.station-label { color: #b7c8c8; font-size: 12px; }
.station.done .node { border-color: #4aa794; color: var(--cyan); }
.station.selected .node {
  border-color: var(--cyan);
  color: #0d302a;
  background: var(--cyan);
  box-shadow: 0 0 0 4px #24413f;
}
.station.selected .station-label { color: var(--cyan); }
.station:hover .node { border-color: var(--cyan); }
.station input:focus-visible + .node, .transport button:focus-visible, .transport a:focus-visible {
  outline: 2px solid #f6d982;
  outline-offset: 4px;
}
.scene-copy { padding: 8px 24px 18px; }
.position { color: var(--cyan); }
.scene-copy strong {
  display: block;
  margin: 4px 0;
  font-size: clamp(17px, 2.7vw, 24px);
  font-weight: 650;
  letter-spacing: -.035em;
  color: #f2f7f4;
}
.scene-copy p {
  max-width: 68ch;
  margin: 0;
  color: #c6d3d2;
  font-size: 14px;
  line-height: 1.5;
}
.workbench {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(0, 1.1fr);
  margin: 0 18px;
  overflow: hidden;
  border: 1px solid var(--edge);
  border-radius: 12px;
  background: #0c191c;
}
.pane { min-width: 0; }
.console { border-left: 1px solid var(--edge); }
.pane-heading {
  gap: 8px;
  padding: 9px 12px;
  border-bottom: 1px solid var(--edge);
  color: #cee0dd;
  background: #1b2c2f;
  font-size: 12px;
}
.pane-heading :deep(svg) { color: #8faeaa; }
.pane-heading small { margin-left: auto; color: var(--cyan); }
.code-area, .output {
  min-height: 115px;
  padding: 12px 0;
  color: #d9e6e3;
  font-size: 12px;
  line-height: 1.75;
}
.code-line { display: flex; gap: 7px; padding: 0 9px; }
.code-line i {
  flex: 0 0 10px;
  color: #859e9c;
  font: inherit;
}
.code-line code {
  padding: 0;
  color: inherit;
  background: transparent;
  font: inherit;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}
.code-line em { color: #c0aafa; font-style: normal; }
.code-line b { color: #e8edf0; }
.code-line u { color: #f6ce8b; text-decoration: none; }
.code-line.added { background: #173e32; }
.code-line.removed, .code-line.fault { background: #3a292c; }
.code-line.added i { color: #88e8b5; }
.code-line.removed i { color: #ffada5; }
.console .output {
  display: grid;
  align-content: start;
  position: relative;
  gap: 2px;
  margin: 0;
  padding: 8px;
  list-style: none;
}
.output::before {
  position: absolute;
  content: "";
  top: 27px;
  bottom: 27px;
  left: 53px;
  width: 1px;
  background: var(--edge);
}
.output .tape-row {
  --tone: #99b9c9;
  display: grid;
  grid-template-columns: 20px 26px minmax(0, 1fr);
  align-items: center;
  gap: 7px;
  position: relative;
  margin: 0;
  padding: 4px 6px;
  border-radius: 7px;
  line-height: 1.35;
}
.tape-row.command { --tone: #c4aafe; }
.tape-row.files, .tape-row.process { --tone: #70dcd0; }
.tape-row.lease { --tone: #efc586; }
.tape-row.pass, .tape-row.cleanup { --tone: #8fe1a7; }
.tape-row.fail { --tone: #ffaaa4; }
.tape-row[aria-current] {
  background: color-mix(in srgb, var(--tone) 13%, #0c191c);
  box-shadow: inset 3px 0 var(--tone);
}
.ordinal { color: #a5b7b8; font-size: 11px; text-align: center; }
.event-icon { display: grid; height: 25px; place-items: center; border-radius: 6px; background: #1c3033; color: var(--tone); }
.entry { display: grid; gap: 2px; overflow-wrap: anywhere; }
.entry strong { color: var(--tone); font: 600 11px var(--vp-font-family-base); }
.tape-row.upcoming .entry { color: #a2b5b5; }
@media (prefers-reduced-motion: no-preference) {
  .tape-row { transition: background-color 180ms ease, box-shadow 180ms ease; }
  .output.playing .tape-row[aria-current]::after {
    content: "";
    position: absolute;
    inset: 3px auto auto 0;
    width: 3px;
    height: 6px;
    border-radius: 2px;
    background: #f0faf9;
    animation: read-event var(--event-duration) linear forwards;
  }
  @keyframes read-event {
    to { top: calc(100% - 9px); }
  }
}
.outcome-label { margin: 12px 24px 0; color: var(--muted); font-size: 12px; }
.closed {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 12px;
  color: #b6c5c6;
  font-size: 13px;
}
.instruments {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  margin: 0;
  padding: 5px 24px 15px;
}
.instruments > div + div { padding-left: 14px; border-left: 1px solid var(--edge); }
.instruments dt { gap: 5px; color: var(--muted); font-size: 11px; }
.instruments dd { margin: 3px 0 0; color: #e6f3ee; font-size: 13px; font-weight: 600; }
.transport { flex-wrap: wrap; padding: 11px 18px; border-top: 1px solid var(--edge); }
.transport a { color: #a4d8d0; font-size: 12px; font-weight: 500; }
.transport button {
  margin-left: auto;
  gap: 9px;
  min-height: 36px;
  padding: 6px 13px;
  border: 1px solid var(--cyan);
  border-radius: 7px;
  color: #062e24;
  background: var(--cyan);
  font-size: 13px;
  font-weight: 650;
}
.transport button:hover { background: #9cf1d9; }
@media (max-width: 690px) {
  .workbench { grid-template-columns: 1fr; }
  .console { border-left: 0; border-top: 1px solid var(--edge); }
  .code-area, .output { min-height: 0; }
}
@media (max-width: 500px) {
  .run-bar, .scene-copy { padding-inline: 14px; }
  .scrubber { padding-inline: 5px; }
  .station-label { font-size: 10px; }
  .workbench { margin-inline: 10px; }
  .instruments { padding-inline: 13px; gap: 6px; }
  .instruments > div + div { padding-left: 6px; }
  .instruments dd { font-size: 12px; }
}
</style>
