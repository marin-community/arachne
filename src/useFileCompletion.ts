import { computed, nextTick, onUnmounted, ref, watch, type Ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

// Both composers send plain text. Complete the mention at the caret, then
// leave the @path in the prompt so the agent can see the selected file.
export function useFileCompletion(
  draft: Ref<string>,
  sessionId: Ref<string | null>,
) {
  const input = ref<HTMLInputElement | HTMLTextAreaElement | null>(null);
  const caret = ref(0);
  const matches = ref<string[]>([]);
  const selected = ref(0);
  const dismissed = ref(false);
  let timer: ReturnType<typeof setTimeout> | null = null;
  let sequence = 0;

  const mention = computed(() => {
    const before = draft.value.slice(0, caret.value);
    const found = /(?:^|\s)@([^\s@]*)$/.exec(before);
    if (!found) return null;
    return { start: before.length - found[1].length - 1, query: found[1] };
  });
  const visible = computed(() =>
    !dismissed.value && mention.value !== null && matches.value.length > 0,
  );

  watch([mention, sessionId], ([current, id]) => {
    selected.value = 0;
    dismissed.value = false;
    const request = ++sequence;
    if (timer) clearTimeout(timer);
    matches.value = [];
    if (!current || !id) return;
    timer = setTimeout(async () => {
      timer = null;
      try {
        const files = await invoke<string[]>("complete_files", {
          id,
          query: current.query,
        });
        if (request === sequence) matches.value = files;
      } catch {
        // Completion is optional; a failed lookup must never block typing.
        if (request === sequence) matches.value = [];
      }
    }, 150);
  });

  function updateCaret() {
    caret.value = input.value?.selectionStart ?? draft.value.length;
  }

  function choose(path: string) {
    const current = mention.value;
    if (!current) return;
    const end = caret.value;
    const suffix = draft.value.slice(end);
    const separator = suffix.startsWith(" ") ? "" : " ";
    draft.value = `${draft.value.slice(0, current.start)}@${path}${separator}${suffix}`;
    const nextCaret = current.start + path.length + 1 + separator.length;
    matches.value = [];
    dismissed.value = true;
    nextTick(() => {
      input.value?.focus();
      input.value?.setSelectionRange(nextCaret, nextCaret);
      caret.value = nextCaret;
    });
  }

  function onKeydown(event: KeyboardEvent): boolean {
    if (!visible.value) return false;
    if (event.key === "Escape") {
      event.preventDefault();
      dismissed.value = true;
      return true;
    }
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const delta = event.key === "ArrowDown" ? 1 : -1;
      selected.value = (selected.value + delta + matches.value.length) % matches.value.length;
      return true;
    }
    if (event.key === "Tab" || event.key === "Enter") {
      event.preventDefault();
      choose(matches.value[selected.value]);
      return true;
    }
    return false;
  }

  onUnmounted(() => {
    sequence++;
    if (timer) clearTimeout(timer);
  });

  return { input, matches, selected, visible, updateCaret, choose, onKeydown };
}
