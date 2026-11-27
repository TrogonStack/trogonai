<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, useId } from "vue";
import Icon from "./Icon.vue";

const clients = [
  {
    name: "TypeScript",
    icon: "code",
    transport: "WebSocket gateway",
    color: "blue",
    title: "Create a workspace from a web app",
    call: "Sandbox.create",
    result: "Sandbox",
    request: "The SDK sends sandbox_lease over an authenticated WebSocket. The gateway forwards the lease request for the session’s tenant.",
    response: "The gateway returns the grant over the socket. The SDK turns the lease ID, provider and deadline into a Sandbox handle.",
  },
  {
    name: "Rust",
    icon: "terminal",
    transport: "NATS request/reply",
    color: "teal",
    title: "Request a lease from a Rust service",
    call: "SandboxClient::lease",
    result: "LeaseGranted",
    request: "The client sends a typed LeaseRequest on the NATS subject sandbox.lease.request and waits for the broker’s reply.",
    response: "The client returns LeaseGranted on success. A broker denial becomes ClientError::Denied, so a service can handle it explicitly.",
  },
  {
    name: "MCP",
    icon: "plug",
    transport: "MCP server · stdio",
    color: "amber",
    title: "Give an agent a workspace through a tool call",
    call: "sandbox_lease",
    result: "JSON tool result",
    request: "The agent calls sandbox_lease over stdio. The MCP server sets its configured tenant and sends the request with its Rust client.",
    response: "The server returns the grant as JSON tool output. The agent can use lease_id for subsequent calls and read the deadline from the result.",
  },
] as const;

type Client = (typeof clients)[number];
type Route = "All" | Client["name"];

const overview = {
  title: "The round trip for a new workspace",
  color: "violet",
  call: "LeaseRequest",
  result: "LeaseGranted",
  request: "A client asks for a workspace, its lifetime and the capabilities it needs. The broker checks tenant policy and quota, then selects a matching worker.",
  response: "When the worker provisions the workspace, it returns a provider receipt. The broker records the lease and sends its ID, provider and deadline back to the caller.",
};

const selected = ref<Route>("All");
const routes: Route[] = ["All", ...clients.map(client => client.name)];
const detail = computed(() => clients.find(client => client.name === selected.value) ?? overview);
const active = (route: Route) => route === "All" || selected.value === "All" || route === selected.value;

const reduced = ref(false);
const motionOverride = ref<boolean>();
const playing = computed(() => motionOverride.value ?? !reduced.value);
let preference: MediaQueryList | undefined;
function updateMotionPreference() {
  reduced.value = preference?.matches ?? false;
  motionOverride.value = undefined;
}
onMounted(() => {
  preference = window.matchMedia("(prefers-reduced-motion: reduce)");
  updateMotionPreference();
  preference.addEventListener("change", updateMotionPreference);
});
onBeforeUnmount(() => preference?.removeEventListener("change", updateMotionPreference));

const groupId = useId();
const arrowId = useId();
const directions = ["request", "reply"] as const;

function wire(route: Route, color: string, from: number, to: number) {
  return {
    route, color,
    request: `M 0 ${from - 2} C 48 ${from - 2}, 52 ${to - 2}, 100 ${to - 2}`,
    reply: `M 100 ${to + 2} C 52 ${to + 2}, 48 ${from + 2}, 0 ${from + 2}`,
  };
}

const connections = [
  {
    name: "ingress",
    wires: clients.map((client, index) => wire(client.name, client.color, (index + 0.5) * 100 / clients.length, 40 + 10 * index)),
  },
  { name: "dispatch", wires: [wire("All", "violet", 50, 50)] },
];
</script>

<template>
  <figure class="lease-map" :class="{ paused: !playing, 'user-playing': motionOverride === true }"
    aria-label="Round trip of a sandbox lease request">
    <div class="toolbar">
      <fieldset>
        <legend>Trace a connection</legend>
        <div class="filters">
          <label v-for="name in routes" :key="name">
            <input v-model="selected" :name="groupId" :value="name" type="radio">
            <span>{{ name }}</span>
          </label>
        </div>
      </fieldset>
      <button class="motion" type="button" :aria-label="playing ? 'Pause packet motion' : 'Play packet motion'"
        @click="motionOverride = !playing">
        <Icon :name="playing ? 'pause' : 'play'" :size="14" />{{ playing ? 'Pause' : 'Play' }}
      </button>
    </div>

    <div class="board">
      <div class="legend">
        <span><i aria-hidden="true" />Request</span>
        <span><i class="return" aria-hidden="true" />Reply</span>
      </div>
      <div class="network">
        <button v-for="client in clients" :key="client.name" type="button"
          :class="['station', 'client', client.color, { muted: !active(client.name) }]"
          :style="{ gridArea: client.color }" :aria-pressed="selected === client.name"
          :aria-label="`Trace the ${client.name} connection`" @click="selected = client.name">
          <span class="badge"><Icon :name="client.icon" :size="18" /></span>
          <span class="station-text"><strong>{{ client.name }}</strong><small>{{ client.transport }}</small></span>
        </button>

        <svg v-for="connection in connections" :key="connection.name" class="cables"
          :class="connection.name" viewBox="0 0 100 100" preserveAspectRatio="none" aria-hidden="true">
          <defs>
            <marker :id="`${arrowId}-${connection.name}`" viewBox="0 0 8 8" refX="7" refY="4"
              markerWidth="5" markerHeight="5" orient="auto">
              <path d="M 1 1 L 7 4 L 1 7" fill="none" stroke="context-stroke" stroke-width="1.7" />
            </marker>
          </defs>
          <g class="orientation">
            <g v-for="line in connection.wires" :key="line.route"
              :class="['pair', line.color]" :data-muted="!active(line.route) || undefined">
              <g v-for="direction in directions" :key="direction" :class="direction">
                <path class="rail" :d="line[direction]" :marker-end="`url(#${arrowId}-${connection.name})`" />
                <path class="packet" :d="line[direction]" pathLength="100" />
              </g>
            </g>
          </g>
        </svg>

        <div class="station hub violet">
          <span class="badge"><Icon name="route" :size="27" /></span>
          <strong>Lease broker</strong>
          <small>Policy · Quota · Placement</small>
          <span class="storage"><Icon name="database" :size="14" />Lease state in JetStream</span>
        </div>
        <div class="station worker violet">
          <span class="badge"><Icon name="server" :size="22" /></span>
          <strong>Provider worker</strong>
          <small>Provisions the workspace</small>
        </div>
      </div>
    </div>

    <div class="exchange" :class="detail.color" aria-live="polite" aria-atomic="true">
      <strong>{{ detail.title }}</strong>
      <div class="lanes">
        <div class="lane">
          <span class="lane-title"><Icon name="arrow-right" :size="16" /> Request</span>
          <code>{{ detail.call }}</code>
          <p>{{ detail.request }}</p>
        </div>
        <div class="lane">
          <span class="lane-title reply-title"><Icon name="arrow-right" :size="16" /> Reply</span>
          <code>{{ detail.result }}</code>
          <p>{{ detail.response }}</p>
        </div>
      </div>
    </div>
    <figcaption><Icon name="server" :size="15" /><span>The broker dispatches over NATS and signs the request when a signing key is configured. The worker returns a receipt with the provider’s sandbox ID.</span></figcaption>
  </figure>
</template>

<style scoped>
.lease-map {
  --ink: var(--vp-c-text-1);
  --subtle: var(--vp-c-text-2);
  --surface: var(--vp-c-bg);
  --soft: var(--vp-c-bg-soft);
  --edge: var(--vp-c-divider);
  container-type: inline-size;
  overflow: hidden;
  margin: 20px 0 28px;
  border: 1px solid var(--edge);
  border-radius: 20px;
  background: var(--surface);
  color: var(--ink);
}
.blue { --signal: color-mix(in srgb, #528cf5 66%, var(--ink)); }
.teal { --signal: color-mix(in srgb, #0da99a 66%, var(--ink)); }
.amber { --signal: color-mix(in srgb, #e69928 66%, var(--ink)); }
.violet { --signal: color-mix(in srgb, #9882fa 66%, var(--ink)); }
.toolbar, fieldset, .filters, .motion, .legend, .legend span, .storage, .lane-title, figcaption {
  display: flex;
  align-items: center;
}
.toolbar {
  flex-wrap: wrap; justify-content: space-between; gap: 10px 16px;
  padding: 12px 18px; border-bottom: 1px solid var(--edge); background: var(--soft);
}
fieldset { flex-wrap: wrap; gap: 6px 12px; min-width: 0; margin: 0; padding: 0; border: 0; }
legend { float: left; padding: 0; color: var(--subtle); font-size: 13px; font-weight: 600; }
.filters { flex-wrap: wrap; gap: 3px; }
.filters label { position: relative; cursor: pointer; }
.filters input { position: absolute; width: 1px; height: 1px; opacity: 0; }
.filters span, .motion {
  border: 1px solid transparent; border-radius: 7px; font-size: 12px; font-weight: 600;
  line-height: 20px;
}
.filters span { display: block; padding: 3px 9px; color: var(--subtle); }
.filters input:checked + span { background: var(--ink); color: var(--surface); }
.filters input:focus-visible + span, .lease-map button:focus-visible { outline: 2px solid var(--vp-c-brand-1); outline-offset: 2px; }
.motion { gap: 5px; padding: 3px 9px; border-color: var(--edge); background: var(--surface); color: var(--subtle); }
.lease-map button { cursor: pointer; }
.board {
  padding: 12px 20px 20px;
  background: radial-gradient(var(--edge) 0.85px, transparent 1px) 0 0 / 15px 15px;
}
.legend { justify-content: center; gap: 20px; margin-bottom: 8px; color: var(--subtle); font: 500 11px/1.5 var(--vp-font-family-mono); }
.legend span { gap: 7px; }
.legend i { position: relative; width: 22px; border-top: 1.5px solid; }
.legend i::after { position: absolute; right: 0; top: -3.5px; width: 5px; height: 5px; border-right: 1.5px solid; border-top: 1.5px solid; transform: rotate(45deg); content: ""; }
.legend .return { border-top-style: dashed; transform: rotate(180deg); }
.network {
  display: grid; align-items: center;
  grid-template-columns: 24% 13% 27% 13% 23%;
  grid-template-rows: repeat(3, 80px);
  grid-template-areas: "blue wire hub dispatch worker" "teal wire hub dispatch worker" "amber wire hub dispatch worker";
}
.station {
  z-index: 1; position: relative; border: 1px solid color-mix(in srgb, var(--signal) 38%, var(--edge));
  border-radius: 13px; background: var(--surface);
}
.station strong { color: var(--ink); font-size: 15px; line-height: 1.25; }
.station small { color: var(--subtle); font-size: 11.5px; line-height: 1.4; }
.client { display: flex; align-items: center; gap: 9px; width: 100%; min-height: 58px; padding: 9px 11px; text-align: left; transition: opacity .18s; }
.client:hover, .client[aria-pressed="true"] { border-color: var(--signal); box-shadow: 0 0 0 2px color-mix(in srgb, var(--signal) 14%, transparent); }
.station-text { display: grid; gap: 2px; }
.badge { flex: none; display: grid; place-items: center; width: 30px; height: 30px; border-radius: 8px; background: color-mix(in srgb, var(--signal) 12%, var(--surface)); color: var(--signal); }
.hub, .worker { display: grid; place-items: center; gap: 5px; padding: 17px 6px; text-align: center; }
.hub {
  grid-area: hub; border-color: var(--signal);
  background: linear-gradient(155deg, color-mix(in srgb, var(--signal) 12%, var(--surface)), var(--surface) 80%);
  box-shadow: 0 8px 30px -16px var(--signal);
}
.hub .badge, .worker .badge { width: 40px; height: 40px; margin-bottom: 3px; border-radius: 13px; }
.hub strong { font-size: clamp(17px, 2.35cqi, 22px); }
.storage { justify-content: center; gap: 4px; margin-top: 5px; padding: 5px 2px 0; border-top: 1px solid var(--edge); color: var(--subtle); font-size: 10.5px; line-height: 1.4; }
.worker { grid-area: worker; }
.cables { width: 100%; height: 100%; overflow: visible; pointer-events: none; }
.ingress { grid-area: wire; }
.dispatch { grid-area: dispatch; }
.pair { transition: opacity .18s; }
.rail, .packet { fill: none; stroke: var(--signal); vector-effect: non-scaling-stroke; }
.rail { stroke-width: 1.7; opacity: .72; }
.reply .rail { stroke-dasharray: 4 4; }
.packet { stroke-width: 4; stroke-linecap: round; stroke-dasharray: 2 98; animation: travel 2s linear infinite; }
.reply .packet { animation-delay: -1s; }
.teal .packet { animation-duration: 2.35s; }
.amber .packet { animation-duration: 2.7s; }
.muted { opacity: .44; }
.pair[data-muted] { opacity: .19; }
.pair[data-muted] .packet { visibility: hidden; }
.paused .packet, .pair[data-muted] .packet { animation-play-state: paused; }
@keyframes travel { to { stroke-dashoffset: -100; } }
.exchange { display: grid; gap: 10px; padding: 16px 18px; border-top: 1px solid var(--edge); background: var(--soft); }
.exchange > strong { font-size: 15px; line-height: 1.4; }
.lanes { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 10px; }
.lane { display: grid; align-content: start; justify-items: start; gap: 5px; padding: 12px; border: 1px solid var(--edge); border-radius: 10px; background: var(--surface); }
.lane-title { gap: 5px; color: var(--signal); font: 650 11px/1.5 var(--vp-font-family-mono); }
.reply-title svg { transform: rotate(180deg); }
.lease-map .lane code { padding: 0; color: var(--ink); background: transparent; font-size: 13px; font-weight: 600; }
.lease-map .lane p { margin: 0; color: var(--subtle); font-size: 13px; line-height: 1.5; overflow-wrap: anywhere; }
.lease-map figcaption { align-items: start; gap: 8px; padding: 11px 18px; border-top: 1px solid var(--edge); color: var(--subtle); font-size: 12px; line-height: 1.5; }
figcaption svg { margin-top: 2px; }
@container (max-width: 620px) {
  .toolbar { padding: 10px 12px; }
  .board { padding: 14px 10px 20px; }
  .legend { margin-bottom: 12px; }
  .network {
    grid-template-columns: repeat(3, minmax(0, 1fr));
    grid-template-rows: auto 70px auto 54px auto;
    grid-template-areas: "blue teal amber" "wire wire wire" "hub hub hub" "dispatch dispatch dispatch" "worker worker worker";
  }
  .client {
    flex-direction: column; align-self: stretch; justify-self: center; width: calc(100% - 5px);
    gap: 6px; padding: 10px 3px; text-align: center;
  }
  .client strong { font-size: 13px; }
  .client small { font-size: 11px; }
  .hub, .worker { justify-self: center; width: min(76%, 230px); }
  .orientation { transform: matrix(0, 1, 1, 0, 0, 0); }
  .exchange { padding: 14px 12px; }
}
@container (max-width: 440px) {
  .lanes { grid-template-columns: minmax(0, 1fr); }
}
@media (prefers-reduced-motion: reduce) {
  .lease-map:not(.user-playing) .packet { animation-play-state: paused; }
  .pair, .client { transition: none; }
}
</style>
