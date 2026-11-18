<script setup lang="ts">
import Icon from "./Icon.vue";

const nodes = [
  { icon: "user", title: "You", line: "Say when, where and what" },
  { icon: "calendar-clock", title: "Scheduler", line: "Records every change" },
  { icon: "timer", title: "NATS", line: "Holds the timer" },
  { icon: "inbox", title: "Your subject", line: "Subscribers receive it" },
];

const links = ["create", "set timer", "fires"];

const steps = [
  {
    title: "You say when, where and what",
    text: "A timing rule, one NATS subject, and a message with a content type, a body and headers.",
  },
  {
    title: "The change is recorded",
    text: "Create, pause, resume and remove each become an event in that schedule's log.",
  },
  {
    title: "NATS holds the timer",
    text: "The scheduler turns the event into a NATS scheduled message. Nothing waits in the scheduler's memory.",
  },
  {
    title: "The message arrives on your subject",
    text: "Anything subscribed there receives it. A one-time, interval or cron schedule can send the latest message from another subject instead of a fixed body.",
  },
];
</script>

<template>
  <figure class="schedule-flow">
    <ol class="schedule-flow__diagram">
      <template v-for="(node, index) in nodes" :key="node.title">
        <li class="schedule-flow__node">
          <span class="schedule-flow__badge"><Icon :name="node.icon" :size="26" /></span>
          <strong>{{ node.title }}</strong>
          <span>{{ node.line }}</span>
        </li>
        <li v-if="index < links.length" class="schedule-flow__link">
          <span class="schedule-flow__label">{{ links[index] }}</span>
          <span class="schedule-flow__line" aria-hidden="true"><span class="schedule-flow__pulse" /></span>
        </li>
      </template>
    </ol>

    <ol class="schedule-flow__steps">
      <li v-for="step in steps" :key="step.title">
        <strong>{{ step.title }}</strong>
        <span>{{ step.text }}</span>
      </li>
    </ol>
  </figure>
</template>

<style scoped>
.schedule-flow {
  --accent: #65a30d;
  margin: 24px 0 0;
  padding: 32px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 28px;
  background: color-mix(in srgb, var(--vp-c-bg) 86%, transparent);
  box-shadow: 0 24px 60px -32px rgba(36, 28, 90, 0.28);
}

.schedule-flow__diagram {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(72px, 0.8fr) minmax(0, 1fr) minmax(72px, 0.8fr) minmax(0, 1fr) minmax(
      72px,
      0.8fr
    ) minmax(0, 1fr);
  align-items: start;
  margin: 0;
  padding: 0;
  list-style: none;
}

.schedule-flow__node {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 2px;
  margin: 0;
  text-align: center;
}

.schedule-flow__node strong {
  margin-top: 8px;
  color: var(--vp-c-text-1);
  font-size: 15px;
  line-height: 1.3;
}

.schedule-flow__node span:last-child {
  color: var(--vp-c-text-2);
  font-size: 12.5px;
  line-height: 1.35;
}

.schedule-flow__badge {
  display: grid;
  place-items: center;
  width: 60px;
  height: 60px;
  border-radius: 18px;
  color: #fff;
  box-shadow: 0 8px 20px -10px var(--accent);
  background:
    linear-gradient(160deg, rgba(255, 255, 255, 0.24), rgba(255, 255, 255, 0) 55%),
    var(--accent);
}

.schedule-flow__link {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 6px;
  margin: 8px 0 0;
  padding: 0 6px 0 14px;
}

.schedule-flow__label {
  color: var(--vp-c-text-3);
  font-size: 11px;
  font-weight: 500;
  line-height: 1.3;
  text-align: center;
}

.schedule-flow__line {
  position: relative;
  width: 100%;
  height: 2px;
  border-radius: 2px;
  background: color-mix(in srgb, var(--accent) 45%, var(--vp-c-divider));
}

.schedule-flow__line::after {
  content: "";
  position: absolute;
  top: 50%;
  right: -2px;
  width: 7px;
  height: 7px;
  border-top: 2px solid currentColor;
  border-right: 2px solid currentColor;
  color: color-mix(in srgb, var(--accent) 60%, var(--vp-c-divider));
  transform: translateY(-50%) rotate(45deg);
}

.schedule-flow__pulse {
  position: absolute;
  top: 50%;
  left: 0;
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--accent);
  box-shadow: 0 0 10px var(--accent);
  transform: translate(-50%, -50%);
  animation: schedule-flow-travel 2.6s ease-in-out infinite;
}

.schedule-flow__steps {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(min(100%, 170px), 1fr));
  gap: 12px;
  margin: 40px 0 0;
  padding: 0;
  list-style: none;
  counter-reset: step;
}

.schedule-flow__steps li {
  display: flex;
  flex-direction: column;
  gap: 4px;
  margin: 0;
  padding: 16px;
  border-radius: 16px;
  background: var(--vp-c-bg-soft);
  counter-increment: step;
}

.schedule-flow__steps li::before {
  content: counter(step);
  display: grid;
  place-items: center;
  width: 24px;
  height: 24px;
  margin-bottom: 6px;
  border-radius: 50%;
  background: var(--vp-c-text-1);
  color: var(--vp-c-bg);
  font-size: 12px;
  font-weight: 700;
}

.schedule-flow__steps strong {
  color: var(--vp-c-text-1);
  font-size: 14px;
  line-height: 1.35;
}

.schedule-flow__steps span {
  color: var(--vp-c-text-2);
  font-size: 13px;
  line-height: 1.5;
}

@keyframes schedule-flow-travel {
  0% {
    left: 0;
    opacity: 0;
  }
  15%,
  85% {
    opacity: 1;
  }
  100% {
    left: 100%;
    opacity: 0;
  }
}

@keyframes schedule-flow-travel-vertical {
  0% {
    top: 0;
    opacity: 0;
  }
  15%,
  85% {
    opacity: 1;
  }
  100% {
    top: 100%;
    opacity: 0;
  }
}

@media (prefers-reduced-motion: reduce) {
  .schedule-flow__pulse {
    display: none;
  }
}

@media (max-width: 860px) {
  .schedule-flow {
    padding: 24px 16px;
    border-radius: 20px;
  }

  .schedule-flow__diagram {
    grid-template-columns: minmax(0, 1fr);
  }

  .schedule-flow__link {
    flex-direction: row;
    justify-content: center;
    gap: 10px;
    height: 72px;
    margin: 8px 0;
    padding: 0;
  }

  .schedule-flow__link .schedule-flow__label {
    width: 40%;
    text-align: right;
  }

  .schedule-flow__link .schedule-flow__line {
    width: 2px;
    height: 100%;
  }

  .schedule-flow__line::after {
    top: auto;
    right: 50%;
    bottom: -2px;
    transform: translateX(50%) rotate(135deg);
  }

  .schedule-flow__link::after {
    content: "";
    width: 40%;
  }

  .schedule-flow__pulse {
    top: 0;
    left: 50%;
    animation-name: schedule-flow-travel-vertical;
  }

  .schedule-flow__steps {
    margin-top: 32px;
  }
}
</style>
