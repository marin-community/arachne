<script setup lang="ts">
import { ref, onMounted, onUnmounted, nextTick } from "vue";

// The new-topic sheet, mirroring NewThreadSheet: composing a topic takes
// over the main display panel (grid-area main), never a floating modal.
// A topic is a leader chat with card metadata — a short title, a longer
// description, and the goal the agent starts from — so this sheet adds
// Title/Description on top of the thread sheet's Goal/Repository.
// Launching routes through App's launchTopic → selectSession flow, which
// swaps the sheet for the live thread; on failure the drafts survive.

const props = defineProps<{
  launching?: boolean;
  error?: string | null;
}>();

const emit = defineEmits<{
  (e: "close"): void;
  (e: "launch", meta: { title: string; description: string; goal: string; repo: string }): void;
}>();

const title = ref("");
const description = ref("");
const goal = ref("");
const repo = ref("marin-community/arachne");
const titleEl = ref<HTMLInputElement | null>(null);
const goalEl = ref<HTMLTextAreaElement | null>(null);

// Keyboard-first: focus the title on open, and Esc always closes.
function onKeydown(e: KeyboardEvent) {
  if (e.key === "Escape") emit("close");
}
onMounted(async () => {
  document.addEventListener("keydown", onKeydown);
  await nextTick();
  titleEl.value?.focus();
});
onUnmounted(() => document.removeEventListener("keydown", onKeydown));

// Enter on the title walks into the goal — a topic can be sketched
// title-first without reaching for the mouse.
function focusGoal() {
  goalEl.value?.focus();
}

function submit() {
  const t = title.value.trim();
  if (!t || props.launching) return;
  // Goal falls back to the title (a topic can start as just a name; the
  // agent's opening message is the goal, or the title when unstated).
  const g = goal.value.trim() || t;
  // Drafts are deliberately not cleared: success unmounts the sheet,
  // while failure keeps them for a retry.
  emit("launch", {
    title: t,
    description: description.value.trim(),
    goal: g,
    repo: repo.value.trim(),
  });
}
</script>

<template>
  <section class="new-thread-sheet" aria-label="New topic">
    <div class="nts-inner">
      <header class="nts-head">
        <h1>New topic</h1>
        <button
          class="nts-close"
          title="Close (esc)"
          aria-label="Close new topic"
          @click="emit('close')"
        >
          ✕
        </button>
      </header>

      <label class="nts-field">
        <span class="nts-field-name">Title</span>
        <input
          ref="titleEl"
          v-model="title"
          placeholder="Short label for the topic card…"
          spellcheck="false"
          @keydown.enter.prevent="focusGoal"
        />
        <span class="nts-hint">The sidebar and topic cards show this label.</span>
      </label>

      <label class="nts-field">
        <span class="nts-field-name">Description <em class="nts-opt">optional</em></span>
        <textarea
          v-model="description"
          rows="3"
          placeholder="What is this topic for?"
        ></textarea>
      </label>

      <label class="nts-field">
        <span class="nts-field-name">Goal</span>
        <textarea
          ref="goalEl"
          v-model="goal"
          rows="5"
          placeholder="The agent's opening message — what to do first…"
          @keydown.enter.exact.prevent="submit"
        ></textarea>
        <span class="nts-hint">
          Enter to launch · Shift+Enter for a new line · falls back to the title
        </span>
      </label>

      <label class="nts-field">
        <span class="nts-field-name">Repository</span>
        <input
          v-model="repo"
          placeholder="owner/name"
          spellcheck="false"
          @keydown.enter.prevent="submit"
        />
        <span class="nts-hint">
          A fresh worktree + branch is created from this repo.
        </span>
      </label>

      <div v-if="props.error" class="nts-error">{{ props.error }}</div>

      <div class="nts-actions">
        <button @click="emit('close')">Cancel</button>
        <button
          class="primary"
          :disabled="!title.trim() || props.launching"
          @click="submit"
        >
          {{ props.launching ? "Launching…" : "Launch topic" }}
        </button>
      </div>
    </div>
  </section>
</template>
