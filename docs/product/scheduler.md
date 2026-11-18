---
title: Scheduler
aside: false
pageClass: wide-page
product:
  name: Scheduler
  tagline: Send a message at a set time or on a repeating rule, without running a timer yourself.
  order: 7
  status: planned
  icon: calendar-clock
  color: "#65A30D"
guarantees:
  - icon: copy
    title: Safe to retry
    text: A repeated command is recognized, so a retry never records the same change twice.
  - icon: list-checks
    title: Changes apply in order
    text: Changes to one schedule are applied one at a time, so a pause never overtakes the create it follows. Other schedules keep moving.
  - icon: history
    title: Nothing is rewritten
    text: Every change is an event in the schedule's log, and removing a schedule keeps its history. The scheduler refuses to start on a log that could drop events.
  - icon: pin
    title: A definition stays put
    text: Pause, resume and remove only change whether a schedule runs. When, where and what it sends never change.
  - icon: ban
    title: Removed is final
    text: A removed schedule stops firing, and its id is never handed to a new schedule.
  - icon: hourglass
    title: Overdue is marked, not fired
    text: A one-time schedule that is already five minutes or more overdue is marked expired instead of firing late.
  - icon: route
    title: One exact destination
    text: A schedule delivers to one exact subject, never a wildcard and never one of the scheduler's own subjects.
progress:
  - name: Schedule commands, events and state
    status: built
    note: Protobuf schemas define every command, event and state a schedule can have.
  - name: Create, pause, resume and remove
    status: built
    note: The decision logic runs natively and as a WebAssembly module, and tests check that both give the same answers.
  - name: Schedule log and read model
    status: built
    note: Changes are stored as events on NATS JetStream with snapshots, and a view answers get and list. Available as a library.
  - name: Processor metrics and traces
    status: built
    note: The processor counts the changes it handled, the timers it set and cleared and its redeliveries, and traces each change.
  - name: Firing through NATS scheduled messages
    status: partial
    note: The processor that sets and clears NATS timers is written and tested against stand-ins. Its tests against a live NATS server run outside CI.
  - name: Recurrence rules
    status: partial
    note: The scheduler works out each next occurrence and records it when it fires. Written and tested, with nothing running it yet.
  - name: Postgres read model
    status: partial
    note: An optional view stored in Postgres exists behind a build flag. Nothing selects it yet.
  - name: Console screens
    status: partial
    note: Screens to list, create, pause, resume and remove schedules exist, running against a stand-in backend.
  - name: Commands over the network
    status: designed
    note: A NATS binding that carries commands to the scheduler is drafted.
  - name: Authorization
    status: designed
    note: The platform's command authorizer is decided, and the scheduler does not go through it yet. Schedules carry no tenant or project today.
---

<ProductHeader />

A schedule says when to send, where to send and what to send. When its time
comes, the message is published on the NATS subject you named, and anything
subscribed to that subject receives it.

::: info Built as a library, not running yet
The scheduling logic is written and tested, but no service runs it yet, so
there is nothing to call today. [Where it stands](#where-it-stands) has the
detail.
:::

## Four ways to say when

Pick the timing rule that fits the job. A schedule uses exactly one.

<ScheduleStrategies />

## From a schedule to a message

The scheduler keeps no timer of its own. It records what you asked for and
hands the waiting to NATS, so no timer lives in a process's memory.

<ScheduleFlow />

## Pause, resume, remove

A schedule moves through a few explicit states, and each state says whether it
still fires.

<ScheduleLifecycle />

## What we promise

<FeatureGrid :items="$frontmatter.guarantees" />

## Where it stands

<BuildProgress :items="$frontmatter.progress" />

No service runs the scheduler yet, and the processor expects a single active
instance. A NATS subject is the only delivery target.

## Learn more

**Understand the design**

- [Decider Platform](/architecture/decider)
- [ADR#0045: Aggregate-Oriented Module Layout for Event-Sourced Services](/adr/0045-event-sourced-service-module-layout)
- [ADR#0057: Decider Command NATS Binding](/adr/0057-decider-command-nats-binding)

**Check its behavior**

- [Verify scheduler decisions with Hydro](/how-to/verify-scheduler-with-hydro)
