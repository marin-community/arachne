<script setup lang="ts">
import { ref, onMounted, onUnmounted, nextTick } from "vue";

// The new-thread sheet: composing a thread happens in the main display
// panel — the thread home — not in a cramped sidebar composer or a
// floating modal. The sheet takes over grid-area `main` while open;
// launching routes through App's launchTask → selectSession flow, which
// swaps the sheet for the live thread. On failure the sheet stays and the
// draft survives, so a retry is one edit away.

// The optional preselected project (Topics [+] on a project heading) is
// shown as context in the sheet and forwarded on launch.
const props = defineProps<{
  launching?: boolean;
  error?: string | null;
  project?: { id: string | null; name: string } | null;
}>();

const emit = defineEmits<{
  (e: "close"): void;
  (e: "launch", task: string, repo: string, project?: { id: string | null; name: string }): void;
}>();

const task = ref("");
const repo = ref("marin-community/arachne");
const taskEl = ref<HTMLTextAreaElement | null>(null);

// Keyboard-first: focus the goal on open, and Esc always closes (even when
// focus has wandered outside the sheet's inputs).
function onKeydown(e: KeyboardEvent) {
  if (e.key === "Escape") emit("close");
}
onMounted(async () => {
  document.addEventListener("keydown", onKeydown);
  await nextTick();
  taskEl.value?.focus();
});
onUnmounted(() => document.removeEventListener("keydown", onKeydown));

function submit() {
  const t = task.value.trim();
  if (!t || props.launching) return;
  // The draft is deliberately not cleared: success unmounts the sheet
  // (App closes it), while failure keeps the text for a retry.
  emit("launch", t, repo.value.trim(), props.project ?? undefined);
}
</script>

<template>
  <section class="new-thread-sheet" aria-label="New thread">
    <div class="nts-inner">
      <header class="nts-head">
        <h1>New thread</h1>
        <span v-if="props.project" class="nts-project" :title="`Files under project ${props.project.name}`">{{ props.project.name }}</span>
        <button
          class="nts-close"
          title="Close (esc)"
          aria-label="Close new thread"
          @click="emit('close')"
        >
          ✕
        </button>
      </header>

      <label class="nts-field">
        <span class="nts-field-name">Goal</span>
        <textarea
          ref="taskEl"
          v-model="task"
          rows="6"
          placeholder="Describe the goal — it becomes the agent's first message…"
          @keydown.enter.exact.prevent="submit"
        ></textarea>
        <span class="nts-hint">Enter to launch · Shift+Enter for a new line</span>
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
          :disabled="!task.trim() || props.launching"
          @click="submit"
        >
          {{ props.launching ? "Launching…" : "Launch thread" }}
        </button>
      </div>
    </div>
  </section>
</template>
