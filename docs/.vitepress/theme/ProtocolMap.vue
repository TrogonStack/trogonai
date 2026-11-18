<script setup lang="ts">
import { computed, ref, useId } from "vue";
import Icon from "./Icon.vue";

type Protocol = "ACP" | "A2A" | "MCP";
type Focus = "All" | Protocol;

const selected = ref<Focus>("All");
const focuses: Focus[] = ["All", "ACP", "A2A", "MCP"];
const markerId = useId();
const conversations = {
  All: {
    title: "Follow the work through its conversations",
    sender: "Caller",
    receiver: "Agent or server",
    request: "A client prompts an agent. The agent can delegate work or call a tool.",
    response: "Session updates, task results and tool output return through their own protocols.",
  },
  ACP: {
    title: "Work with an agent from your client",
    sender: "Editor or client",
    receiver: "Your agent",
    request: "The editor sends a prompt or manages an agent session.",
    response: "The agent returns prompt results and streams session updates to the client.",
  },
  A2A: {
    title: "Hand work to another agent",
    sender: "Your agent",
    receiver: "Another agent",
    request: "Your agent sends a message or follows a task with another agent.",
    response: "The other agent reports task progress, results and artifacts.",
  },
  MCP: {
    title: "Bring tools and context into the work",
    sender: "AI application",
    receiver: "MCP server",
    request: "The AI application calls a tool, reads a resource or retrieves a prompt.",
    response: "The MCP server returns the requested tool result or context.",
  },
} satisfies Record<Focus, { title: string; sender: string; receiver: string; request: string; response: string }>;

const conversation = computed(() => conversations[selected.value]);
const isActive = (protocol: Protocol) => selected.value === "All" || selected.value === protocol;
const connections: Array<{ protocol: Protocol; request: string; response: string }> = [
  { protocol: "ACP", request: "M 240 188 H 380", response: "M 380 212 H 240" },
  { protocol: "A2A", request: "M 620 176 C 690 176 690 68 760 68", response: "M 760 92 C 710 92 710 200 620 200" },
  { protocol: "MCP", request: "M 620 224 C 710 224 690 308 760 308", response: "M 760 332 C 690 332 710 248 620 248" },
];
</script>

<template>
  <figure class="protocol-map" aria-label="ACP, A2A and MCP relationships around an agent">
    <div class="protocol-map__toolbar">
      <span class="protocol-map__eyebrow"><Icon name="route" :size="16" /> Follow a connection</span>
      <div class="protocol-map__filters" role="group" aria-label="Highlight a protocol">
        <button
          v-for="focus in focuses"
          :key="focus"
          type="button"
          :class="['protocol-map__filter', `is-${focus.toLowerCase()}`]"
          :aria-pressed="selected === focus"
          @click="selected = focus"
        >
          {{ focus }}
        </button>
      </div>
    </div>

    <div class="protocol-map__legend" aria-label="Solid arrows show requests; dashed arrows show responses">
      <span><i class="protocol-map__legend-line" aria-hidden="true" /> Request</span>
      <span><i class="protocol-map__legend-line is-response" aria-hidden="true" /> Response</span>
    </div>

    <div class="protocol-map__stage">
      <svg class="protocol-map__wires" viewBox="0 0 1000 400" preserveAspectRatio="none" aria-hidden="true">
        <defs>
          <marker :id="markerId" viewBox="0 0 8 8" refX="6" refY="4" markerWidth="5" markerHeight="5" orient="auto">
            <path d="M 1 1 L 6 4 L 1 7" fill="none" stroke="context-stroke" stroke-width="1.4" />
          </marker>
        </defs>
        <g
          v-for="connection in connections"
          :key="connection.protocol"
          :class="['protocol-map__connection', `is-${connection.protocol.toLowerCase()}`, { 'is-muted': !isActive(connection.protocol) }]"
        >
          <g v-for="direction in (['request', 'response'] as const)" :key="direction" :class="`is-${direction}`">
            <path class="protocol-map__wire-glow" :d="connection[direction]" />
            <path class="protocol-map__wire" :d="connection[direction]" :marker-end="`url(#${markerId})`" />
            <path class="protocol-map__packets" :d="connection[direction]" />
          </g>
        </g>
      </svg>

      <div :class="['protocol-map__node', 'protocol-map__editor', { 'is-muted': !isActive('ACP') }]">
        <span class="protocol-map__node-icon is-acp"><Icon name="terminal" :size="24" /></span>
        <strong>Editor or client</strong>
        <span>Where you work</span>
        <span class="protocol-map__node-note">Prompts · Permissions</span>
      </div>

      <div class="protocol-map__node protocol-map__agent">
        <span class="protocol-map__agent-orbit" aria-hidden="true" />
        <span class="protocol-map__node-icon"><Icon name="bot" :size="30" /></span>
        <strong>Your agent</strong>
        <span>Where the work happens</span>
        <span class="protocol-map__agent-tag"><span /> Connected through TrogonAI</span>
      </div>

      <div :class="['protocol-map__node', 'protocol-map__peer', { 'is-muted': !isActive('A2A') }]">
        <span class="protocol-map__node-icon is-a2a"><Icon name="bot" :size="24" /></span>
        <strong>Another agent</strong>
        <span>Delegate and collaborate</span>
        <span class="protocol-map__node-note">Tasks · Progress · Artifacts</span>
      </div>

      <div :class="['protocol-map__node', 'protocol-map__tools', { 'is-muted': !isActive('MCP') }]">
        <span class="protocol-map__node-icon is-mcp"><Icon name="plug" :size="24" /></span>
        <strong>Tools & resources</strong>
        <span>Use an MCP server</span>
        <span class="protocol-map__node-note">Tools · Context · Prompts</span>
      </div>

      <span
        v-for="protocol in (['ACP', 'A2A', 'MCP'] as const)"
        :key="protocol"
        :class="['protocol-map__edge-label', `is-${protocol.toLowerCase()}`, { 'is-muted': !isActive(protocol) }]"
      >{{ protocol }}</span>

      <div :class="['protocol-map__mobile-acp', 'is-acp', { 'is-muted': !isActive('ACP') }]" aria-hidden="true">
        <i class="protocol-map__mobile-rail is-request" />
        <i class="protocol-map__mobile-rail is-response" />
        <span>ACP</span>
      </div>
      <div class="protocol-map__mobile-branch" aria-hidden="true">
        <div
          v-for="protocol in (['A2A', 'MCP'] as const)"
          :key="protocol"
          :class="['protocol-map__mobile-edge', `is-${protocol.toLowerCase()}`, { 'is-muted': !isActive(protocol) }]"
        >
          <i class="protocol-map__mobile-rail is-request" />
          <i class="protocol-map__mobile-rail is-response" />
          <span>{{ protocol }}</span>
        </div>
      </div>
    </div>

    <div :class="['protocol-map__detail', `is-${selected.toLowerCase()}`]" aria-live="polite" aria-atomic="true">
      <strong>{{ conversation.title }}</strong>
      <div class="protocol-map__exchange">
        <div v-for="direction in (['request', 'response'] as const)" :key="direction" :class="['protocol-map__lane', `is-${direction}`]">
          <div
            class="protocol-map__lane-route"
            role="group"
            :aria-label="direction === 'request' ? `Request from ${conversation.sender} to ${conversation.receiver}` : `Response from ${conversation.receiver} to ${conversation.sender}`"
          >
            <strong>{{ conversation.sender }}</strong>
            <span class="protocol-map__lane-arrow">
              <span>{{ direction === 'request' ? 'Request' : 'Response' }}</span>
              <i aria-hidden="true" />
            </span>
            <strong>{{ conversation.receiver }}</strong>
          </div>
          <p>{{ conversation[direction] }}</p>
        </div>
      </div>
    </div>

    <figcaption>
      Each pair of arrows is a conversation carried over NATS by its protocol
      adapters. The agent application decides when to delegate work or use a tool.
      Callbacks and notifications follow their protocol's lifecycle.
    </figcaption>
  </figure>
</template>

<style scoped>
.protocol-map {
  --acp: #6366f1;
  --a2a: #0d9488;
  --mcp: #d97706;
  --connection: var(--vp-c-brand-1);
  overflow: hidden;
  margin: 24px 0;
  border: 1px solid var(--vp-c-divider);
  border-radius: 26px;
  background: var(--vp-c-bg);
}

.is-acp { --connection: var(--acp); }
.is-a2a { --connection: var(--a2a); }
.is-mcp { --connection: var(--mcp); }

.protocol-map__toolbar {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: space-between;
  gap: 14px;
  padding: 20px 24px;
  border-bottom: 1px solid var(--vp-c-divider);
}

.protocol-map__eyebrow {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  color: var(--vp-c-text-2);
  font-size: 13px;
  font-weight: 600;
}

.protocol-map__filters {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
  padding: 4px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 12px;
  background: var(--vp-c-bg-soft);
}

.protocol-map__filter {
  padding: 5px 14px;
  border: 1px solid transparent;
  border-radius: 8px;
  background: transparent;
  color: var(--vp-c-text-2);
  font-size: 12px;
  font-weight: 600;
  line-height: 1.5;
  cursor: pointer;
  transition: background 0.2s, color 0.2s, box-shadow 0.2s;
}

.protocol-map__filter[aria-pressed="true"] {
  border-color: color-mix(in srgb, var(--connection) 22%, var(--vp-c-divider));
  background: var(--vp-c-bg);
  color: color-mix(in srgb, var(--connection) 85%, var(--vp-c-text-1));
  box-shadow: 0 2px 5px #0000000a;
}

.protocol-map__filter:focus-visible {
  outline: 2px solid var(--connection);
  outline-offset: 3px;
}

.protocol-map__legend { display: flex; flex-wrap: wrap; justify-content: center; gap: 24px; padding: 18px 16px 0; color: var(--vp-c-text-2); font-size: 11px; }
.protocol-map__legend > span { display: inline-flex; align-items: center; gap: 10px; }
.protocol-map__legend-line { position: relative; width: 36px; border-top: 2px solid var(--vp-c-text-2); }
.protocol-map__legend-line::after { position: absolute; top: -5px; right: 0; width: 7px; height: 7px; border-top: 2px solid; border-right: 2px solid; transform: rotate(45deg); content: ""; }
.protocol-map__legend-line.is-response { border-top-style: dashed; }
.protocol-map__legend-line.is-response::after { right: auto; left: 0; transform: rotate(-135deg); }

.protocol-map__stage {
  position: relative;
  height: 430px;
  margin: 12px 24px;
  background-image: radial-gradient(var(--vp-c-divider) 1px, transparent 1px);
  background-size: 18px 18px;
}

.protocol-map__wires { position: absolute; inset: 0; width: 100%; height: 100%; overflow: visible; }
.protocol-map__connection { transition: opacity 0.25s; }
.protocol-map__connection path { fill: none; stroke: var(--connection); vector-effect: non-scaling-stroke; }
.protocol-map__wire-glow { stroke-width: 9; opacity: 0.08; }
.protocol-map__wire { stroke-width: 1.6; opacity: 0.6; }
.is-response .protocol-map__wire { stroke-dasharray: 5 5; }
.protocol-map__packets { stroke-width: 3; stroke-linecap: round; stroke-dasharray: 2 28; animation: protocol-packets 3s linear infinite; }
.is-a2a .protocol-map__packets { animation-delay: -1s; }
.is-mcp .protocol-map__packets { animation-delay: -2s; }

.protocol-map__node {
  position: absolute;
  top: 50%;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 6px;
  width: 24%;
  padding: 20px 10px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 20px;
  background: var(--vp-c-bg);
  text-align: center;
  transform: translateY(-50%);
  transition: opacity 0.25s, border-color 0.25s;
}

.protocol-map__editor { left: 0; }
.protocol-map__agent {
  left: 38%;
  padding: 28px 8px;
  border-color: color-mix(in srgb, var(--vp-c-brand-1) 40%, var(--vp-c-divider));
  background: linear-gradient(160deg, var(--vp-c-brand-soft), var(--vp-c-bg) 75%);
  box-shadow: 0 18px 50px -25px color-mix(in srgb, var(--vp-c-brand-1) 45%, transparent);
}
.protocol-map__peer { left: 76%; top: 20%; }
.protocol-map__tools { left: 76%; top: 80%; }
.protocol-map__node strong { color: var(--vp-c-text-1); font-size: 15px; line-height: 1.4; }
.protocol-map__agent strong { font-size: 20px; }
.protocol-map__node > span:not([class]) { color: var(--vp-c-text-2); font-size: 12px; line-height: 1.5; }
.protocol-map__node-note { color: var(--vp-c-text-3); font-size: 11px; line-height: 1.5; }

.protocol-map__node-icon {
  display: grid;
  flex: none;
  place-items: center;
  width: 46px;
  height: 46px;
  margin-bottom: 6px;
  border: 1px solid color-mix(in srgb, var(--connection) 20%, transparent);
  border-radius: 14px;
  background: color-mix(in srgb, var(--connection) 10%, var(--vp-c-bg));
  color: color-mix(in srgb, var(--connection) 85%, var(--vp-c-text-1));
}
.protocol-map__agent .protocol-map__node-icon { width: 58px; height: 58px; border-radius: 18px; }
.protocol-map__agent-orbit { position: absolute; inset: -8px; pointer-events: none; border: 1px dashed color-mix(in srgb, var(--vp-c-brand-1) 22%, transparent); border-radius: 27px; }
.protocol-map__agent-tag { display: inline-flex; align-items: center; gap: 5px; padding-top: 8px; color: var(--vp-c-text-2); font-size: 11px; line-height: 1.5; }
.protocol-map__agent-tag > span { width: 5px; height: 5px; border-radius: 50%; background: var(--a2a); }

.protocol-map__edge-label {
  position: absolute;
  top: 50%;
  left: 31%;
  padding: 4px 10px;
  border: 1px solid color-mix(in srgb, var(--connection) 28%, var(--vp-c-divider));
  border-radius: 999px;
  background: var(--vp-c-bg);
  color: color-mix(in srgb, var(--connection) 85%, var(--vp-c-text-1));
  font-size: 11px;
  font-weight: 700;
  line-height: 1.5;
  transform: translate(-50%, -50%);
  transition: opacity 0.25s;
}
.protocol-map__edge-label.is-a2a { left: 69%; top: 27%; }
.protocol-map__edge-label.is-mcp { left: 69%; top: 73%; }
.is-muted { opacity: 0.2; }
.is-muted .protocol-map__packets { animation-play-state: paused; }
.protocol-map__mobile-acp, .protocol-map__mobile-branch { display: none; }

.protocol-map__detail { display: flex; flex-direction: column; gap: 14px; padding: 22px 24px; border-top: 1px solid var(--vp-c-divider); background: var(--vp-c-bg-soft); }
.protocol-map__detail > strong { color: var(--vp-c-text-1); font-size: 15px; line-height: 1.5; }
.protocol-map__exchange { display: grid; gap: 12px; }
.protocol-map__lane { display: flex; flex-direction: column; gap: 12px; padding: 16px; border: 1px solid var(--vp-c-divider); border-radius: 14px; background: var(--vp-c-bg); }
.protocol-map__lane-route { display: grid; grid-template-columns: minmax(0, 1fr) minmax(80px, 1.3fr) minmax(0, 1fr); align-items: center; gap: 16px; }
.protocol-map__lane-route > strong { color: var(--vp-c-text-1); font-size: 12px; font-weight: 600; line-height: 1.5; }
.protocol-map__lane-route > strong:last-child { text-align: right; }
.protocol-map__lane-arrow { display: flex; flex-direction: column; gap: 8px; color: color-mix(in srgb, var(--connection) 85%, var(--vp-c-text-1)); text-align: center; font-size: 11px; font-weight: 700; }
.protocol-map__lane-arrow i { position: relative; width: 100%; border-top: 2px solid; }
.protocol-map__lane-arrow i::after { position: absolute; top: -5px; right: 0; width: 7px; height: 7px; border-top: 2px solid; border-right: 2px solid; transform: rotate(45deg); content: ""; }
.is-response .protocol-map__lane-arrow i { border-top-style: dashed; }
.is-response .protocol-map__lane-arrow i::after { right: auto; left: 0; transform: rotate(-135deg); }
.protocol-map__lane p { margin: 0; color: var(--vp-c-text-2); font-size: 13px; line-height: 1.6; }
.protocol-map figcaption { padding: 16px 24px; border-top: 1px solid var(--vp-c-divider); color: var(--vp-c-text-3); font-size: 11px; line-height: 1.7; }

@keyframes protocol-packets { to { stroke-dashoffset: -120; } }

@media (max-width: 720px) {
  .protocol-map__stage { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); grid-template-rows: auto 64px auto 88px auto; column-gap: 16px; height: auto; margin: 20px 16px; }
  .protocol-map__wires, .protocol-map__edge-label { display: none; }
  .protocol-map__node { position: relative; top: auto; left: auto; width: 100%; min-width: 0; padding: 18px 8px; transform: none; }
  .protocol-map__editor, .protocol-map__agent { grid-column: 1 / -1; justify-self: center; width: min(100%, 260px); }
  .protocol-map__editor { grid-row: 1; }
  .protocol-map__agent { grid-row: 3; }
  .protocol-map__peer { grid-column: 1; grid-row: 5; }
  .protocol-map__tools { grid-column: 2; grid-row: 5; }
  .protocol-map__node strong { font-size: 13px; }
  .protocol-map__agent strong { font-size: 20px; }
  .protocol-map__mobile-acp { position: relative; display: grid; grid-column: 1 / -1; grid-row: 2; place-items: center; }
  .protocol-map__mobile-rail { position: absolute; top: 0; bottom: 0; left: calc(50% - 12px); width: 0; border-left: 2px solid var(--connection); opacity: 0.75; }
  .protocol-map__mobile-rail::after { position: absolute; bottom: 0; left: -5px; width: 7px; height: 7px; border-right: 2px solid var(--connection); border-bottom: 2px solid var(--connection); transform: rotate(45deg); content: ""; }
  .protocol-map__mobile-rail.is-response { left: calc(50% + 12px); border-left-style: dashed; }
  .protocol-map__mobile-rail.is-response::after { top: 0; bottom: auto; transform: rotate(-135deg); }
  .protocol-map__mobile-acp > span, .protocol-map__mobile-edge > span { position: relative; display: inline-flex; align-items: center; gap: 6px; padding: 3px 10px; border: 1px solid color-mix(in srgb, var(--connection) 30%, var(--vp-c-divider)); border-radius: 999px; background: var(--vp-c-bg); color: var(--connection); font-size: 11px; font-weight: 700; }
  .protocol-map__mobile-branch { position: relative; display: grid; grid-template-columns: 1fr 1fr; grid-column: 1 / -1; grid-row: 4; column-gap: 16px; }
  .protocol-map__mobile-branch::before { position: absolute; top: 18px; left: calc(25% - 16px); width: calc(50% + 8px); height: 1px; background: var(--vp-c-divider); content: ""; }
  .protocol-map__mobile-branch::after { position: absolute; top: 26px; left: calc(25% + 8px); width: calc(50% + 8px); height: 1px; border-top: 1px dashed var(--vp-c-text-3); content: ""; }
  .protocol-map__mobile-edge { position: relative; display: grid; align-items: center; justify-items: center; padding-top: 20px; transition: opacity 0.25s; }
  .protocol-map__mobile-edge .protocol-map__mobile-rail.is-request { top: 18px; }
  .protocol-map__mobile-edge .protocol-map__mobile-rail.is-response { top: 26px; }
  .protocol-map__mobile-branch { background: linear-gradient(var(--vp-c-divider), var(--vp-c-divider)) calc(50% - 12px) 0 / 1px 18px no-repeat, linear-gradient(var(--vp-c-text-3), var(--vp-c-text-3)) calc(50% + 12px) 0 / 1px 26px no-repeat; }
}

@media (max-width: 480px) {
  .protocol-map__toolbar { padding: 18px 16px; }
  .protocol-map__filter { padding: 5px 12px; }
  .protocol-map__lane { padding: 14px 12px; }
  .protocol-map__lane-route { grid-template-columns: minmax(0, 1fr) minmax(56px, 0.8fr) minmax(0, 1fr); gap: 8px; }
  .protocol-map__lane-route > strong { font-size: 11px; }
  .protocol-map__detail { min-height: 240px; padding: 20px 16px; }
  .protocol-map figcaption { padding: 16px; }
}

@media (prefers-reduced-motion: reduce) {
  .protocol-map__packets { animation: none; }
  .protocol-map__connection, .protocol-map__node, .protocol-map__edge-label, .protocol-map__mobile-edge, .protocol-map__filter { transition: none; }
}

@media (pointer: coarse) {
  .protocol-map__filter { min-height: 44px; }
}
</style>
