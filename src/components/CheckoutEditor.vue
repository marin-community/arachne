<script setup lang="ts">
import { computed, onBeforeUnmount, nextTick, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { editorDrafts } from "../editorDrafts";

const props = defineProps<{ sessionId: string; path: string }>();
const emit = defineEmits<{ (e: "close"): void; (e: "saved"): void }>();
const original = ref("");
const content = ref("");
const checkout = ref("");
const loading = ref(true);
const saving = ref(false);
const error = ref("");
const notice = ref("");
const textarea = ref<HTMLTextAreaElement | null>(null);
const scrollTop = ref(0);
const dirty = computed(() => content.value !== original.value);
const key = computed(() => `${props.sessionId}:${checkout.value}:${props.path}`);
const lines = computed(() => content.value.split("\n").length);
const wrap = ref(false);
const fileName = computed(() => props.path.split("/").pop());
const errorMessage = (e: unknown) => (e as { message?: string })?.message ?? String(e);
let generation = 0;
function remember() {
  if (dirty.value && checkout.value) editorDrafts.set(key.value, { original: original.value, content: content.value, checkout: checkout.value });
  else editorDrafts.delete(key.value);
}
async function load(discard = false, restore = false) {
  if (dirty.value && !discard && !window.confirm("Discard your draft and load the latest file?")) return;
  const current = ++generation;
  loading.value = true;
  error.value = "";
  try {
    const result = await invoke<{ content: string; checkout: string }>("read_checkout_file", { sessionId: props.sessionId, path: props.path });
    if (current !== generation) return;
    // Resolve the current checkout before looking up drafts: session IDs can
    // collide across different Loom servers.
    checkout.value = result.checkout;
    const draft = restore ? editorDrafts.get(key.value) : undefined;
    original.value = draft?.original ?? result.content;
    content.value = draft?.content ?? result.content;
    notice.value = draft ? "Restored your unsaved draft. Saving checks for changes made by the agent." : "";
    if (!draft) editorDrafts.delete(key.value);
  } catch (e) { if (current === generation) error.value = errorMessage(e); }
  finally { if (current === generation) { loading.value = false; await nextTick(); textarea.value?.focus(); } }
}
async function save() {
  if (!dirty.value || saving.value || loading.value || !checkout.value) return;
  saving.value = true;
  error.value = "";
  // The textarea is disabled during saving so the saved snapshot is unambiguous.
  const saved = original.value.includes("\r\n") && !original.value.replaceAll("\r\n", "").includes("\n")
    ? content.value.replace(/\r?\n/g, "\r\n") : content.value;
  try {
    await invoke("save_checkout_file", { sessionId: props.sessionId, path: props.path, checkout: checkout.value, original: original.value, content: saved });
    original.value = saved;
    content.value = saved;
    notice.value = "Saved to this checkout. Ask the agent to validate before integrating or landing.";
    remember();
    emit("saved");
  } catch (e) { error.value = errorMessage(e); }
  finally { saving.value = false; await nextTick(); textarea.value?.focus(); }
}
function close() { remember(); emit("close"); }
function onKeydown(event: KeyboardEvent) {
  if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "s") {
    event.preventDefault(); event.stopPropagation(); void save();
  } else if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); if (!saving.value) close(); }
  else if (event.key === "Tab" && event.target === textarea.value && !event.shiftKey) {
    event.preventDefault();
    const target = textarea.value!;
    target.setRangeText("  ", target.selectionStart, target.selectionEnd, "end");
    content.value = target.value;
  }
}
onBeforeUnmount(() => { remember(); generation++; });
watch(content, remember);
watch(() => [props.sessionId, props.path], () => { void load(true, true); }, { immediate: true });
</script>

<template>
  <Teleport to="body">
    <div class="checkout-editor-backdrop" @keydown="onKeydown" @click.self="!saving && close()">
      <section class="checkout-editor" role="dialog" aria-modal="true" :aria-label="`Edit ${path}`">
        <header>
          <div class="editor-file"><strong>{{ fileName }}<span v-if="dirty" aria-label="Unsaved changes"> ●</span></strong><small :title="path">{{ path }}</small></div>
          <label class="editor-wrap"><input v-model="wrap" type="checkbox" /> Wrap</label>
          <button :disabled="loading || saving" @click="load()">Reload</button>
          <button class="primary" :disabled="!dirty || loading || saving" title="Save (⌘S / Ctrl+S)" @click="save">{{ saving ? "Saving…" : "Save" }}</button>
          <button :disabled="saving" aria-label="Close editor" title="Close (Escape); drafts are kept while the app is open" @click="close">×</button>
        </header>
        <div v-if="error" class="editor-message editor-error" role="alert">{{ error }}</div>
        <div v-if="notice" class="editor-message" role="status">{{ notice }}</div>
        <div v-if="loading" class="editor-message">Loading file…</div>
        <div v-else-if="checkout" class="editor-code">
          <div v-if="!wrap" class="editor-gutter" aria-hidden="true"><pre :style="{ transform: `translateY(-${scrollTop}px)` }">{{ Array.from({ length: lines }, (_, index) => index + 1).join('\n') }}</pre></div>
          <textarea ref="textarea" v-model="content" :disabled="saving" :wrap="wrap ? 'soft' : 'off'" spellcheck="false" autocorrect="off" autocapitalize="off" :aria-label="`Contents of ${path}`" autofocus @scroll="scrollTop = ($event.target as HTMLTextAreaElement).scrollTop" />
        </div>
        <footer><span>{{ lines }} lines · {{ dirty ? "Unsaved draft kept while this app is open" : "Saved" }}</span><span>⌘S / Ctrl+S save · Tab indent · Esc close</span></footer>
      </section>
    </div>
  </Teleport>
</template>

<style scoped>
.checkout-editor-backdrop { position: fixed; inset: 0; z-index: 200; background: #0008; display: grid; place-items: center; padding: 24px; }
.checkout-editor { width: min(1080px, 95vw); height: min(820px, 88vh); min-height: 200px; background: var(--bg); border: 1px solid var(--border); border-radius: 12px; box-shadow: 0 24px 80px #0006; display: flex; flex-direction: column; overflow: hidden; }
header { display: flex; align-items: center; gap: 8px; padding: 12px 16px; background: var(--bg-raised); border-bottom: 1px solid var(--border); }
.editor-file { flex: 1; display: flex; flex-direction: column; min-width: 0; }
.editor-file strong { font-size: 13px; }
.editor-file small { color: var(--text-dim); font: 11px var(--mono); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.editor-wrap { display: flex; gap: 4px; align-items: center; font-size: 11px; color: var(--text-dim); }
.editor-code { display: flex; min-height: 0; flex: 1; }
.editor-code textarea { flex: 1; min-width: 0; resize: none; border: 0; outline: none; color: var(--text); background: var(--bg); font: 13px/21px var(--mono); padding: 12px; tab-size: 2; white-space: pre; overflow: auto; }
.editor-code textarea[wrap="soft"] { white-space: pre-wrap; }
.editor-gutter { overflow: hidden; min-width: 45px; border-right: 1px solid var(--border); color: var(--text-dim); user-select: none; }
.editor-gutter pre { margin: 0; padding: 12px 8px; text-align: right; font: 13px/21px var(--mono); }
.editor-message { padding: 10px 16px; color: var(--text-dim); font-size: 12px; border-bottom: 1px solid var(--border); }
.editor-error { color: var(--blocked); }
footer { display: flex; justify-content: space-between; gap: 12px; flex-wrap: wrap; padding: 8px 16px; border-top: 1px solid var(--border); color: var(--text-dim); font-size: 11px; }
</style>
