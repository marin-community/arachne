<script setup lang="ts">
import { ref, onMounted, onUnmounted, nextTick } from "vue";
import { addAttachments, filesFromClipboard, imagePreviewUrl, MAX_LAUNCH_TOTAL_BYTES, type FileAttachment } from "../attachments";

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
  (e: "launch", task: string, repo: string, meta?: { project?: { id: string | null; name: string }; attachments?: FileAttachment[] }): void;
}>();

const task = ref("");
const repo = ref("marin-community/arachne");
const taskEl = ref<HTMLTextAreaElement | null>(null);
const attachments = ref<FileAttachment[]>([]);
const attachmentError = ref("");
const attachmentLoading = ref(false);

async function addFiles(files: FileList | File[]) {
  if (attachmentLoading.value) return;
  attachmentLoading.value = true;
  try {
    attachments.value = await addAttachments(attachments.value, files, MAX_LAUNCH_TOTAL_BYTES);
    attachmentError.value = "";
  } catch (error: any) {
    attachmentError.value = error?.message ?? String(error);
  } finally {
    attachmentLoading.value = false;
  }
}
function onFileInput(event: Event) {
  const input = event.target as HTMLInputElement;
  if (input.files) void addFiles(input.files);
  input.value = "";
}
function onPaste(event: ClipboardEvent) {
  if (!event.clipboardData) return;
  const files = filesFromClipboard(event.clipboardData);
  if (!files.length) return;
  event.preventDefault();
  void addFiles(files);
}

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
  if ((!t && !attachments.value.length) || props.launching || attachmentLoading.value) return;
  // The draft is deliberately not cleared: success unmounts the sheet
  // (App closes it), while failure keeps the text for a retry.
  emit("launch", t || `Review ${attachments.value[0].name}`, repo.value.trim(), {
    project: props.project ?? undefined,
    attachments: attachments.value,
  });
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
          @paste="onPaste"
        ></textarea>
        <span class="nts-hint">Enter to launch · Shift+Enter for a new line</span>
      </label>

      <div class="attachment-row" @dragover.prevent @drop.prevent="($event) => $event.dataTransfer?.files && addFiles($event.dataTransfer.files)">
        <label class="attachment-pick">+ Attach files or images<input type="file" multiple :disabled="attachmentLoading" aria-label="Attach files or images to new thread" @change="onFileInput" /></label>
        <span v-for="(file, index) in attachments" :key="file.name" class="attachment-chip">
          <img v-if="imagePreviewUrl(file)" :src="imagePreviewUrl(file)!" class="attachment-preview" alt="" />
          {{ file.name }} <button type="button" :aria-label="`Remove ${file.name}`" @click="attachments.splice(index, 1)">×</button>
        </span>
        <span v-if="attachmentError" class="attachment-error">{{ attachmentError }}</span>
        <span v-if="attachmentLoading" class="attachment-hint">Reading files…</span>
      </div>

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
          :disabled="(!task.trim() && !attachments.length) || props.launching || attachmentLoading"
          @click="submit"
        >
          {{ props.launching ? "Launching…" : "Launch thread" }}
        </button>
      </div>
    </div>
  </section>
</template>
