import test from "node:test";
import assert from "node:assert/strict";
import { deliverySummary, messageSource, watchTriggerKind, watchTriggerLabel } from "../src/trackActivity.ts";

test("watch labels distinguish event-driven work, timers, and mixed rules", () => {
  const event = { trigger: { on: ["pr.merged", "session.exited=error"] } };
  assert.equal(watchTriggerKind(event), "Events");
  assert.equal(watchTriggerLabel(event), "On pr.merged, session.exited=error");
  assert.equal(watchTriggerKind({ trigger: { every: "5m" } }), "Timer");
  assert.equal(watchTriggerKind({ trigger: { every: "5m", event: "attention" } }), "Events + timer");
  assert.equal(watchTriggerLabel({ trigger: { event: "attention", level: "blocked" } }), "On attention=blocked");
  assert.equal(watchTriggerKind({ trigger: {} }), "Manual");
});

test("mailbox records never imply delivery or completion without evidence", () => {
  assert.equal(deliverySummary({ kind: "message", deliveries: [] }), "Recorded");
  assert.equal(deliverySummary({ kind: "result", deliveries: [] }), "Result recorded");
  assert.equal(deliverySummary({ deliveries: [{ state: "delivered" }] }), "Delivered");
  assert.equal(deliverySummary({ deliveries: [{ state: "pending" }] }), "pending");
  assert.equal(deliverySummary({ deliveries: [{ state: "delivered" }, { state: "failed", last_error: "offline" }] }), "Delivery failed");
});


test("handoff drill-down follows source provenance before shared channel ownership", () => {
  const channel = { session_id: "coordinator" };
  assert.equal(messageSource({ author_kind: "session", author_id: "worker" }, channel), "worker");
  assert.equal(messageSource({ author_kind: "user", payload: { arachne: { source_session_id: "source" } } }, channel), "source");
  assert.equal(messageSource({ author_kind: "system", payload: { child_session_id: "child" } }, channel), "child");
  assert.equal(messageSource({ author_kind: "user" }, channel), "coordinator");
  assert.equal(messageSource({ author_kind: "user" }), null);
});
