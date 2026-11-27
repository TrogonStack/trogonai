<script setup lang="ts">
import { ref, useId } from "vue";
import Icon from "./Icon.vue";

const decision = ref<"permit" | "deny">("permit");
const paused = ref(false);
const group = useId();
</script>

<template>
  <figure class="egress-sequence" :class="{ paused }" aria-label="Sandbox credential custody for a package registry request">
    <div class="controls">
      <fieldset>
        <legend>Registry request</legend>
        <label v-for="option in (['permit', 'deny'] as const)" :key="option">
          <input v-model="decision" type="radio" :name="group" :value="option" />
          <span>{{ option === 'permit' ? 'Allowed' : 'Denied' }}</span>
        </label>
      </fieldset>
      <button class="motion" type="button" @click="paused = !paused">
        <Icon :name="paused ? 'play' : 'pause'" :size="14" />{{ paused ? 'Play flow' : 'Pause flow' }}
      </button>
    </div>

    <div :key="decision" class="diagram" :class="decision">
      <div class="boundary" aria-hidden="true"></div>
      <div class="boundary-label"><Icon name="lock" :size="13" />Credential custody</div>

      <div class="node guest"><span class="badge"><Icon name="terminal" :size="23" /></span>
        <strong>Guest process</strong><small>Holds a placeholder</small>
      </div>
      <div class="wire incoming" aria-hidden="true"><i class="line"><b class="pulse" /></i></div>

      <div class="node proxy"><span class="badge"><Icon name="shield-check" :size="23" /></span>
        <strong>Egress proxy</strong><small>{{ decision === 'permit' ? 'Matches the transform rule' : 'Refuses the destination' }}</small>
      </div>
      <div class="exchange" :class="{ idle: decision === 'deny' }">
        <div class="wire request"><i class="line"><b v-if="decision === 'permit'" class="pulse" /></i></div>
        <div class="wire issued"><i class="line"><b v-if="decision === 'permit'" class="pulse" /></i></div>
      </div>
      <div class="node broker" :class="{ idle: decision === 'deny' }"><span class="badge"><Icon name="key-round" :size="23" /></span>
        <strong>Credential broker</strong><small>{{ decision === 'permit' ? 'Issues a scoped credential' : 'No credential requested' }}</small>
      </div>

      <div class="wire forward" :class="{ idle: decision === 'deny' }" aria-hidden="true">
        <i class="line"><b v-if="decision === 'permit'" class="pulse" /></i>
        <span v-if="decision === 'deny'" class="blocked"><Icon name="ban" :size="18" /></span>
      </div>
      <div class="node upstream" :class="{ idle: decision === 'deny' }"><span class="badge"><Icon name="globe" :size="23" /></span>
        <strong>Package registry</strong><small>{{ decision === 'permit' ? 'Receives the authenticated request' : 'Receives no request' }}</small>
      </div>
    </div>

    <figcaption aria-live="polite">
      <Icon :name="decision === 'permit' ? 'key-round' : 'ban'" :size="17" />
      <span v-if="decision === 'permit'">The guest sends the placeholder. The proxy obtains the bound credential from the broker and substitutes it on the approved upstream request.</span>
      <span v-else>The proxy stops the request at the policy check and reports the violation. The guest still holds only the placeholder.</span>
    </figcaption>
  </figure>
</template>

<style scoped>
.egress-sequence {
  --clean: color-mix(in srgb, #14b8a6 70%, var(--vp-c-text-1));
  --secret: color-mix(in srgb, #e11d48 70%, var(--vp-c-text-1));
  --muted: var(--vp-c-text-3);
  container-type: inline-size;
  overflow: hidden;
  margin: 20px 0;
  border: 1px solid var(--vp-c-divider);
  border-radius: 22px;
  background: var(--vp-c-bg);
}
.controls, fieldset, .motion, figcaption, .boundary-label { display: flex; align-items: center; }
.controls { flex-wrap: wrap; justify-content: space-between; gap: 8px; padding: 12px 16px; border-bottom: 1px solid var(--vp-c-divider); background: var(--vp-c-bg-soft); }
fieldset { flex-wrap: wrap; gap: 5px; margin: 0; min-width: 0; border: 0; padding: 0; }
legend { float: left; margin-right: 7px; padding: 0; color: var(--vp-c-text-2); font-size: 13px; font-weight: 600; }
label { position: relative; cursor: pointer; }
label input { position: absolute; inset: 0; width: 100%; height: 100%; margin: 0; opacity: 0; cursor: pointer; }
label span, .motion { display: inline-flex; gap: 6px; align-items: center; padding: 4px 9px; border: 1px solid var(--vp-c-divider); border-radius: 7px; background: var(--vp-c-bg); color: var(--vp-c-text-2); font-size: 12px; font-weight: 600; }
label input:checked + span { border-color: var(--vp-c-text-1); background: var(--vp-c-text-1); color: var(--vp-c-bg); }
label input:focus-visible + span, .motion:focus-visible { outline: 2px solid var(--vp-c-brand-1); outline-offset: 2px; }
.diagram {
  position: relative; isolation: isolate; display: grid; align-items: center;
  grid-template-areas: '. . title . .' 'guest incoming proxy forward upstream' '. . exchange . .' '. . broker . .';
  grid-template-columns: minmax(0, 1fr) minmax(40px, .5fr) minmax(0, 1.25fr) minmax(40px, .5fr) minmax(0, 1fr);
  padding: 18px clamp(14px, 3cqi, 30px);
}
.boundary { grid-area: title-start / proxy-start / broker-end / proxy-end; align-self: stretch; z-index: -1; border: 1px dashed color-mix(in srgb, var(--secret) 52%, transparent); border-radius: 15px; background: color-mix(in srgb, #e11d48 4%, var(--vp-c-bg)); }
.boundary-label { grid-area: title; justify-self: center; gap: 5px; padding: 7px 5px 2px; color: var(--secret); font-size: 11px; font-weight: 650; }
.node { display: grid; justify-items: center; gap: 4px; min-width: 0; padding: 10px 6px; text-align: center; line-height: 1.35; }
.node strong { color: var(--vp-c-text-1); font-size: 14px; }
.node small { max-width: 21ch; color: var(--vp-c-text-2); font-size: 12px; }
.badge { display: grid; place-items: center; width: 40px; height: 40px; margin-bottom: 2px; border-radius: 11px; color: var(--accent); background: color-mix(in srgb, var(--accent) 13%, var(--vp-c-bg)); }
.guest { grid-area: guest; --accent: var(--clean); }
.proxy { grid-area: proxy; --accent: var(--secret); }
.broker { grid-area: broker; --accent: var(--secret); }
.upstream { grid-area: upstream; --accent: var(--vp-c-brand-1); }
.incoming { grid-area: incoming; --delay: 0s; color: var(--clean); }
.forward { grid-area: forward; --delay: 3.9s; color: var(--secret); }
.wire { position: relative; display: grid; align-items: center; justify-items: stretch; min-width: 0; }
.line { position: relative; display: block; height: 2px; border-radius: 2px; background: color-mix(in srgb, currentColor 42%, transparent); }
.line::after { position: absolute; right: 0; top: -3px; content: ''; width: 6px; height: 6px; border: solid currentColor; border-width: 1.5px 1.5px 0 0; transform: rotate(45deg); }
.pulse { position: absolute; width: 8px; height: 8px; inset-inline-start: 0; top: 50%; opacity: 0; border-radius: 50%; transform: translate(-50%, -50%); background: currentColor; box-shadow: 0 0 7px color-mix(in srgb, currentColor 65%, transparent); animation: along 5.8s linear infinite; animation-delay: var(--delay); }
@keyframes along { 0% { inset-inline-start: 0; opacity: 0; } 2%, 18% { opacity: 1; } 20% { inset-inline-start: 100%; opacity: 0; } 100% { inset-inline-start: 100%; opacity: 0; } }
.exchange { grid-area: exchange; display: flex; justify-self: center; margin-block: 12px; }
.exchange .wire { width: 38px; height: 14px; }
.request { --delay: 1.3s; color: var(--clean); }
.issued { --delay: 2.6s; color: var(--secret); }
.exchange .request { transform: rotate(90deg); }
.exchange .issued { transform: rotate(-90deg); }
.idle { --accent: var(--muted); color: var(--muted); }
.idle .wire { color: var(--muted); }
.idle .line { opacity: .45; }
.forward.idle .line::after { display: none; }
.blocked { position: absolute; justify-self: center; padding: 2px; color: var(--vp-c-danger-1); background: var(--vp-c-bg); }
.paused .pulse { animation-play-state: paused; }
figcaption { gap: 10px; min-height: 58px; padding: 10px 16px; border-top: 1px solid var(--vp-c-divider); color: var(--vp-c-text-2); background: var(--vp-c-bg-soft); font-size: 13px; line-height: 1.5; }
figcaption svg { color: var(--secret); }
@media (prefers-reduced-motion: reduce) { .pulse, .motion { display: none; } }
@container (max-width: 560px) {
  .diagram {
    grid-template-areas: 'guest . .' 'incoming . .' 'title title title' 'proxy exchange broker' 'forward . .' 'upstream . .';
    grid-template-columns: minmax(0, 1fr) minmax(22px, .28fr) minmax(0, 1fr);
    padding: 10px;
  }
  .boundary { grid-area: title-start / proxy-start / broker-end / broker-end; }
  .incoming, .forward { width: 36px; height: 36px; justify-self: center; transform: rotate(90deg); }
  .blocked { transform: rotate(-90deg); }
  .exchange { display: grid; gap: 8px; margin: 0; justify-self: stretch; }
  .exchange .wire { width: 100%; height: 12px; }
  .exchange .request { transform: none; }
  .exchange .issued { transform: rotate(180deg); }
}
</style>
