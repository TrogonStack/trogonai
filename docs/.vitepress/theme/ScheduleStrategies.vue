<script setup lang="ts">
import Icon from "./Icon.vue";

type Mark = "sent" | "none" | "start" | "skipped" | "extra";

type Slot = { label: string; mark: Mark; note?: string };

// A timer NATS holds, measured in slots: 0 is the left edge of the lane and
// the middle of slot n is n + 0.5. An open timer runs past the right edge. A
// timer set after an earlier one fired leaves a gap before it.
type Timer = { from: number; to: number; open?: boolean; afterSend?: boolean };

type Strategy = {
  name: string;
  icon: string;
  rule: string;
  text: string;
  summary: string;
  slots: Slot[];
  timers: Timer[];
};

type Group = {
  title: string;
  icon: string;
  text: string;
  strategies: Strategy[];
};

const groups: Group[] = [
  {
    title: "NATS keeps the timer",
    icon: "timer",
    text: "The scheduler hands the rule to NATS, and NATS sends your message every time it comes due.",
    strategies: [
      {
        name: "One time",
        icon: "clock",
        rule: "2026-12-01T09:00:00Z",
        text: "Send once, at an exact instant.",
        summary: "A timer runs until December 1 at 09:00 UTC. One message is sent then, and nothing after.",
        slots: [
          { label: "Nov 29", mark: "none" },
          { label: "Nov 30", mark: "none" },
          { label: "Dec 1", mark: "sent", note: "09:00 UTC" },
          { label: "Dec 2", mark: "none" },
          { label: "Dec 3", mark: "none" },
        ],
        timers: [{ from: 0, to: 2.5 }],
      },
      {
        name: "Fixed interval",
        icon: "repeat",
        rule: "every 15m",
        text: "Send again after the same gap, counted from when the schedule starts.",
        summary:
          "The schedule starts at 08:52. A message is sent 15 minutes later, at 09:07, and every 15 minutes after that.",
        slots: [
          { label: "08:52", mark: "start", note: "starts" },
          { label: "09:07", mark: "sent" },
          { label: "09:22", mark: "sent" },
          { label: "09:37", mark: "sent" },
          { label: "09:52", mark: "sent" },
          { label: "10:07", mark: "sent" },
        ],
        timers: [{ from: 0.5, to: 6, open: true }],
      },
      {
        name: "Cron expression",
        icon: "terminal",
        rule: "0 0 9 * * MON-FRI",
        text: "Send on a calendar pattern, here 09:00 on weekdays, in the time zone you name. The first field is seconds.",
        summary: "A message is sent at 09:00 from Monday to Friday. Nothing is sent on Saturday or Sunday.",
        slots: [
          { label: "Mon", mark: "sent", note: "09:00" },
          { label: "Tue", mark: "sent", note: "09:00" },
          { label: "Wed", mark: "sent", note: "09:00" },
          { label: "Thu", mark: "sent", note: "09:00" },
          { label: "Fri", mark: "sent", note: "09:00" },
          { label: "Sat", mark: "none" },
          { label: "Sun", mark: "none" },
        ],
        timers: [{ from: 0, to: 7, open: true }],
      },
    ],
  },
  {
    title: "The scheduler sets each timer",
    icon: "calendar-clock",
    text: "NATS cannot read a recurrence rule. The scheduler works out the next date and sets one timer for it. When that timer fires, it sends your message and sets the next.",
    strategies: [
      {
        name: "Recurrence rule",
        icon: "calendar-days",
        rule: "FREQ=WEEKLY;BYDAY=MO,WE,FR",
        text: "Send on an iCalendar rule, with a start, an optional end or count, dates to skip and extra dates, in the time zone you name.",
        summary:
          "A message is sent on Monday. Wednesday is a skipped date, Thursday is an extra date, and a message is sent on Friday. Each send has its own timer.",
        slots: [
          { label: "Mon", mark: "sent" },
          { label: "Tue", mark: "none" },
          { label: "Wed", mark: "skipped", note: "skipped" },
          { label: "Thu", mark: "extra", note: "extra" },
          { label: "Fri", mark: "sent" },
          { label: "Sat", mark: "none" },
          { label: "Sun", mark: "none" },
        ],
        timers: [
          { from: 0, to: 0.5 },
          { from: 0.5, to: 3.5, afterSend: true },
          { from: 3.5, to: 4.5, afterSend: true },
          { from: 4.5, to: 7, open: true, afterSend: true },
        ],
      },
    ],
  },
];

const legend: { mark: Mark; text: string }[] = [
  { mark: "sent", text: "Message sent" },
  { mark: "skipped", text: "Skipped date" },
  { mark: "extra", text: "Extra date" },
];

// A timer reaches a few pixels past the instant it fires at, so the tick that
// marks the send sits on the bar. The next timer starts clear of that tick.
const OVERHANG = 3;
const GAP = 7;

function timerStyle(timer: Timer, slots: number) {
  const start = timer.afterSend ? GAP : timer.from > 0 ? -OVERHANG : 0;
  const end = timer.open ? 0 : OVERHANG;

  return {
    left: `calc(${(timer.from / slots) * 100}% + ${start}px)`,
    width: `calc(${((timer.to - timer.from) / slots) * 100}% + ${end - start}px)`,
  };
}
</script>

<template>
  <figure class="schedule-strategies">
    <section v-for="group in groups" :key="group.title" class="schedule-strategies__group">
      <header class="schedule-strategies__head">
        <span class="schedule-strategies__head-icon"><Icon :name="group.icon" :size="18" /></span>
        <span class="schedule-strategies__head-copy">
          <strong>{{ group.title }}</strong>
          <span>{{ group.text }}</span>
        </span>
      </header>

      <ul class="schedule-strategies__rows">
        <li v-for="item in group.strategies" :key="item.name" class="schedule-strategies__row">
          <div class="schedule-strategies__what">
            <span class="schedule-strategies__icon"><Icon :name="item.icon" :size="18" /></span>
            <span class="schedule-strategies__copy">
              <strong>{{ item.name }}</strong>
              <code class="schedule-strategies__rule">{{ item.rule }}</code>
              <span class="schedule-strategies__text">{{ item.text }}</span>
            </span>
          </div>

          <div class="schedule-strategies__lane" role="img" :aria-label="item.summary">
            <div class="schedule-strategies__timers" aria-hidden="true">
              <span
                v-for="(timer, index) in item.timers"
                :key="index"
                :class="['schedule-strategies__timer', { 'is-open': timer.open }]"
                :style="timerStyle(timer, item.slots.length)"
              />
            </div>
            <ol class="schedule-strategies__slots" aria-hidden="true" :style="{ '--slots': item.slots.length }">
              <li v-for="slot in item.slots" :key="slot.label" :class="['schedule-strategies__slot', `is-${slot.mark}`]">
                <span class="schedule-strategies__stem" />
                <span :class="['schedule-strategies__mark', `is-${slot.mark}`]">
                  <span class="schedule-strategies__dot" />
                </span>
                <span class="schedule-strategies__label">{{ slot.label }}</span>
                <span class="schedule-strategies__note">{{ slot.note }}</span>
              </li>
            </ol>
          </div>
        </li>
      </ul>
    </section>

    <figcaption class="schedule-strategies__legend">
      <span class="schedule-strategies__key">
        <span class="schedule-strategies__swatch" />
        A timer held by NATS
      </span>
      <span v-for="key in legend" :key="key.mark" class="schedule-strategies__key">
        <span :class="['schedule-strategies__mark', `is-${key.mark}`]">
          <span class="schedule-strategies__dot" />
        </span>
        {{ key.text }}
      </span>
    </figcaption>
  </figure>
</template>

<style scoped>
.schedule-strategies {
  --accent: #65a30d;
  --accent-ink: color-mix(in srgb, var(--accent) 80%, var(--vp-c-text-1));
  --timer: color-mix(in srgb, var(--accent) 42%, var(--vp-c-bg-soft));
  margin: 24px 0 0;
  overflow: hidden;
  border: 1px solid var(--vp-c-divider);
  border-radius: 28px;
  background: color-mix(in srgb, var(--vp-c-bg) 86%, transparent);
  box-shadow: 0 24px 60px -32px rgba(36, 28, 90, 0.28);
}

.schedule-strategies__group {
  padding: 28px 32px 10px;
}

.schedule-strategies__group + .schedule-strategies__group {
  border-top: 1px solid var(--vp-c-divider);
  background: linear-gradient(160deg, color-mix(in srgb, var(--accent) 8%, transparent), transparent 60%);
}

.schedule-strategies__head {
  display: flex;
  align-items: flex-start;
  gap: 12px;
}

.schedule-strategies__head-icon {
  display: grid;
  flex: none;
  place-items: center;
  width: 32px;
  height: 32px;
  border-radius: 10px;
  background: var(--accent);
  color: #fff;
}

.schedule-strategies__head-copy {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
  color: var(--vp-c-text-2);
  font-size: 14px;
  line-height: 1.5;
}

.schedule-strategies__head-copy strong {
  color: var(--vp-c-text-1);
  font-size: 16px;
  line-height: 1.35;
}

.schedule-strategies__rows {
  margin: 20px 0 0;
  padding: 0;
  list-style: none;
}

.schedule-strategies__row {
  display: grid;
  grid-template-columns: minmax(0, 300px) minmax(0, 1fr);
  align-items: center;
  gap: 28px;
  margin: 0;
  padding: 18px 0;
  border-top: 1px solid var(--vp-c-divider);
}

.schedule-strategies__what {
  display: flex;
  align-items: flex-start;
  gap: 12px;
  min-width: 0;
}

.schedule-strategies__icon {
  display: grid;
  flex: none;
  place-items: center;
  width: 36px;
  height: 36px;
  border-radius: 12px;
  background: color-mix(in srgb, var(--accent) 16%, var(--vp-c-bg));
  color: var(--accent-ink);
}

.schedule-strategies__copy {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 6px;
  min-width: 0;
}

.schedule-strategies__copy strong {
  color: var(--vp-c-text-1);
  font-size: 15px;
  line-height: 1.3;
}

.schedule-strategies__rule {
  max-width: 100%;
  padding: 3px 8px;
  border-radius: 8px;
  background: var(--vp-c-bg-soft);
  color: var(--vp-c-text-1);
  font-size: 12px;
  line-height: 1.4;
  overflow-wrap: anywhere;
}

.schedule-strategies__text {
  color: var(--vp-c-text-2);
  font-size: 13px;
  line-height: 1.5;
}

.schedule-strategies__lane {
  min-width: 0;
  padding: 18px 12px 10px;
  border-radius: 16px;
  background: var(--vp-c-bg-soft);
}

.schedule-strategies__timers {
  position: relative;
  height: 6px;
}

.schedule-strategies__timer {
  position: absolute;
  top: 0;
  height: 6px;
  border-radius: 999px;
  background: var(--timer);
}

.schedule-strategies__timer.is-open {
  border-radius: 999px 0 0 999px;
  background: linear-gradient(90deg, var(--timer) calc(100% - 36px), transparent);
}

.schedule-strategies__slots {
  display: grid;
  grid-template-columns: repeat(var(--slots), minmax(0, 1fr));
  margin: 0;
  padding: 0;
  list-style: none;
}

.schedule-strategies__slot {
  display: flex;
  flex-direction: column;
  align-items: center;
  min-width: 0;
  margin: 0;
}

/* The stem starts on the timer bar, so a send reads as a tick on that timer. */
.schedule-strategies__stem {
  width: 2px;
  height: 18px;
  margin-top: -6px;
  border-radius: 1px;
}

.schedule-strategies__slot.is-sent .schedule-strategies__stem,
.schedule-strategies__slot.is-extra .schedule-strategies__stem {
  background: var(--accent);
}

.schedule-strategies__slot.is-start .schedule-strategies__stem {
  background: var(--timer);
}

.schedule-strategies__mark {
  display: grid;
  flex: none;
  place-items: center;
  width: 16px;
  height: 16px;
}

.schedule-strategies__dot {
  position: relative;
  box-sizing: border-box;
  width: 4px;
  height: 4px;
  border-radius: 50%;
  background: var(--vp-c-text-3);
}

.schedule-strategies__mark.is-sent .schedule-strategies__dot,
.schedule-strategies__mark.is-extra .schedule-strategies__dot {
  width: 12px;
  height: 12px;
  background: var(--accent);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 22%, transparent);
}

.schedule-strategies__mark.is-extra .schedule-strategies__dot {
  width: 14px;
  height: 14px;
}

.schedule-strategies__mark.is-extra .schedule-strategies__dot::before,
.schedule-strategies__mark.is-extra .schedule-strategies__dot::after {
  content: "";
  position: absolute;
  top: 50%;
  left: 50%;
  width: 8px;
  height: 2px;
  border-radius: 1px;
  background: #fff;
  transform: translate(-50%, -50%);
}

.schedule-strategies__mark.is-extra .schedule-strategies__dot::after {
  transform: translate(-50%, -50%) rotate(90deg);
}

.schedule-strategies__mark.is-start .schedule-strategies__dot {
  width: 10px;
  height: 10px;
  border: 2px solid var(--timer);
  background: var(--vp-c-bg);
}

.schedule-strategies__mark.is-skipped .schedule-strategies__dot {
  width: 12px;
  height: 12px;
  border: 2px solid var(--vp-c-text-3);
  background: transparent;
}

.schedule-strategies__mark.is-skipped .schedule-strategies__dot::after {
  content: "";
  position: absolute;
  top: 50%;
  left: 50%;
  width: 16px;
  height: 2px;
  border-radius: 1px;
  background: var(--vp-c-text-2);
  transform: translate(-50%, -50%) rotate(-45deg);
}

.schedule-strategies__label {
  margin-top: 8px;
  color: var(--vp-c-text-3);
  font-size: 11px;
  font-variant-numeric: tabular-nums;
  line-height: 1.3;
  white-space: nowrap;
}

.schedule-strategies__slot.is-sent .schedule-strategies__label,
.schedule-strategies__slot.is-extra .schedule-strategies__label {
  color: var(--vp-c-text-1);
  font-weight: 600;
}

.schedule-strategies__slot.is-start .schedule-strategies__label,
.schedule-strategies__slot.is-skipped .schedule-strategies__label {
  color: var(--vp-c-text-2);
}

.schedule-strategies__note {
  min-height: 13px;
  color: var(--vp-c-text-2);
  font-size: 10px;
  line-height: 1.3;
  white-space: nowrap;
}

.schedule-strategies__slot.is-extra .schedule-strategies__note {
  color: var(--accent-ink);
}

.schedule-strategies__legend {
  display: flex;
  flex-wrap: wrap;
  gap: 8px 24px;
  padding: 16px 32px;
  border-top: 1px solid var(--vp-c-divider);
  color: var(--vp-c-text-2);
  font-size: 12px;
  line-height: 1.4;
}

.schedule-strategies__key {
  display: inline-flex;
  align-items: center;
  gap: 8px;
}

.schedule-strategies__swatch {
  width: 28px;
  height: 6px;
  border-radius: 999px;
  background: var(--timer);
}

@media (max-width: 1100px) {
  .schedule-strategies__row {
    grid-template-columns: minmax(0, 1fr);
    gap: 14px;
  }
}

@media (max-width: 860px) {
  .schedule-strategies__group {
    padding: 22px 18px 4px;
  }

  .schedule-strategies__lane {
    padding: 18px 4px 10px;
  }

  .schedule-strategies__legend {
    padding: 14px 18px;
  }
}
</style>
