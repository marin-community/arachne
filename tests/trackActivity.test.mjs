import test from "node:test";
import assert from "node:assert/strict";
import { deliverySummary, watchTriggerKind, watchTriggerLabel } from "../src/trackActivity.ts";

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
