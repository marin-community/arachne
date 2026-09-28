import { computed, ref, watch, type Ref } from "vue";

// Loom publishes this catalog through ACP available_commands_update and
// persists it in sessions.chat.metadata.commands. Names and hints belong to
// the active agent; Arachne only presents them and sends the selected text.
export interface SlashCommand {
  name: string;
  description: string;
  input?: { type?: string; hint?: string } | null;
}

export function useSlashCommands(
  draft: Ref<string>,
  commands: Ref<SlashCommand[]>,
  input: Ref<HTMLTextAreaElement | null>,
) {
  const query = computed(() => draft.value.match(/^\/([^\s/]*)$/)?.[1] ?? null);
  const dismissed = ref(false);
  const matches = computed(() => {
    if (query.value === null || dismissed.value) return [];
    const needle = query.value.toLowerCase();
    return commands.value
      .filter((command) => command.name.toLowerCase().includes(needle))
      .slice(0, 10);
  });
  const selected = ref(0);
  watch(draft, () => { selected.value = 0; dismissed.value = false; });

  const hint = computed(() => {
    const name = draft.value.match(/^\/([^\s]+)\s*$/)?.[1];
    return commands.value.find((command) => command.name === name)?.input?.hint ?? "";
  });

  function choose(command: SlashCommand) {
    // The space closes completion while leaving room for optional arguments.
    draft.value = `/${command.name} `;
    queueMicrotask(() => {
      input.value?.focus();
      input.value?.setSelectionRange(draft.value.length, draft.value.length);
    });
  }

  function show() {
    draft.value = "/";
    queueMicrotask(() => input.value?.focus());
  }

  function onKeydown(event: KeyboardEvent): boolean {
    if (!matches.value.length) return false;
    if (event.key === "Escape") {
      event.preventDefault();
      dismissed.value = true;
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      selected.value = (selected.value + 1) % matches.value.length;
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      selected.value = (selected.value - 1 + matches.value.length) % matches.value.length;
    } else if (event.key === "Tab" || (event.key === "Enter" && !event.shiftKey && !event.metaKey && !event.ctrlKey)) {
      event.preventDefault();
      choose(matches.value[selected.value] ?? matches.value[0]);
    } else {
      return false;
    }
    return true;
  }

  return { matches, selected, hint, choose, show, onKeydown };
}
