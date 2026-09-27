<script setup lang="ts">
import { onUnmounted, ref } from "vue";
import { copyText } from "../clipboard";

// The two-squares glyph on every chat bubble, tool call and thinking
// block. Copies the block's raw text, then flips to a checkmark for a
// moment — the clipboard itself gives no feedback that a click landed.
const props = defineProps<{ text: string; label?: string }>();
const copied = ref(false);
let timer: ReturnType<typeof setTimeout> | null = null;

async function copy() {
  if (copied.value) return;
  if (!(await copyText(props.text))) return;
  copied.value = true;
  if (timer) clearTimeout(timer);
  timer = setTimeout(() => (copied.value = false), 1400);
}

onUnmounted(() => {
  if (timer) clearTimeout(timer);
});
</script>

<template>
  <!-- .stop keeps the click from also toggling the disclosure line a
       thought or tool-call header sits inside. -->
  <button
    class="copy-button"
    :class="{ copied }"
    type="button"
    :title="label ?? 'Copy'"
    :aria-label="label ?? 'Copy'"
    @click.stop="copy()"
    @keydown.stop
  >
    <svg
      v-if="copied"
      viewBox="0 0 24 24"
      width="13"
      height="13"
      fill="none"
      stroke="currentColor"
      stroke-width="2.5"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
    >
      <polyline points="20 6 9 17 4 12" />
    </svg>
    <svg
      v-else
      viewBox="0 0 24 24"
      width="13"
      height="13"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
    >
      <rect x="9" y="9" width="13" height="13" rx="2" ry="2" />
      <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
    </svg>
  </button>
</template>
