<script setup lang="ts">
import { ref, computed, watch, onMounted, onUnmounted } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { BackendFleet } from "../backendFleet";
import { normalizeServerUrl, readServerProfiles, rememberServer } from "../serverProfiles";

const props = defineProps<{
  url: string;
  backends?: BackendFleet[];
  token: string;
  connected: boolean;
  saving?: boolean;
  error?: string | null;
}>();
const emit = defineEmits<{
  (e: "close"): void;
  (e: "save", url: string, token: string): void;
  (e: "rename", url: string, name: string): void;
}>();

const url = ref(props.url);
const token = ref(props.token);
const profiles = ref(readServerProfiles(localStorage));
const name = ref(profiles.value.find((p) => p.url === props.url)?.name || "");
const tokenLoading = ref(false);
const localError = ref("");
const urlInput = ref<HTMLInputElement | null>(null);
const validUrl = computed(() => {
  try { return normalizeServerUrl(url.value); } catch { return ""; }
});
const savedProfile = computed(() => profiles.value.find(profile => profile.url === validUrl.value));
const canRename = computed(() => !!savedProfile.value && name.value.trim() !== savedProfile.value.name);
function rename() {
  if (!canRename.value || props.saving) return;
  try {
    profiles.value = rememberServer(localStorage, { url: validUrl.value, name: name.value }, { requirePersistence: true });
    name.value = profiles.value.find(profile => profile.url === validUrl.value)!.name;
    localError.value = "";
    emit("rename", validUrl.value, name.value);
  } catch (error: any) {
    localError.value = `Could not save server name: ${error?.message ?? String(error)}`;
  }
}
const selectedStatus = computed(() => {
  const backend = props.backends?.find(backend => backend.url === validUrl.value);
  return backend ? (backend.connecting ? "Connecting…" : backend.available ? "Available" : "Unavailable") : "Not registered";
});
const choices = computed(() => {
  const rows = [...profiles.value];
  for (const choice of [{ name: "Local Loom", url: "http://127.0.0.1:7878" }, { name: "Current server", url: props.url }]) {
    if (!rows.some((p) => p.url === choice.url)) rows.push(choice);
  }
  return rows;
});

let tokenRequest = 0;
let tokenTimer: ReturnType<typeof setTimeout> | undefined;
watch(url, () => {
  const request = ++tokenRequest;
  clearTimeout(tokenTimer);
  // Clear immediately: editing a host must never forward another server's token.
  token.value = "";
  localError.value = "";
  const normalized = validUrl.value;
  tokenLoading.value = !!normalized;
  if (!normalized) return;
  name.value = profiles.value.find((p) => p.url === normalized)?.name || "";
  tokenTimer = setTimeout(async () => {
    try {
      const stored = await invoke<string | null>("load_token", { baseUrl: normalized });
      if (request === tokenRequest) token.value = stored || "";
    } catch (error: any) {
      if (request === tokenRequest) localError.value = `Could not read this server's Keychain token: ${error?.message ?? String(error)}`;
    } finally {
      if (request === tokenRequest) tokenLoading.value = false;
    }
  }, 200);
});

function submit() {
  if (!validUrl.value || tokenLoading.value || props.saving) return;
  profiles.value = rememberServer(localStorage, { name: name.value, url: validUrl.value });
  emit("save", validUrl.value, token.value.trim());
}
function onKeydown(event: KeyboardEvent) {
  if (event.key === "Escape" && !props.saving) { event.preventDefault(); emit("close"); }
}
onMounted(async () => {
  document.addEventListener("keydown", onKeydown);
  urlInput.value?.focus();
  const request = ++tokenRequest;
  tokenLoading.value = true;
  try {
    const stored = await invoke<string | null>("load_token", { baseUrl: validUrl.value });
    if (request === tokenRequest) token.value = stored || "";
  } catch (error: any) {
    if (request === tokenRequest) localError.value = error?.message ?? String(error);
  } finally { if (request === tokenRequest) tokenLoading.value = false; }
});
onUnmounted(() => { tokenRequest++; clearTimeout(tokenTimer); document.removeEventListener("keydown", onKeydown); });
</script>

<template>
  <div class="modal-backdrop" @click.self="!saving && emit('close')">
    <form class="modal server-settings" role="dialog" aria-modal="true" aria-labelledby="server-settings-title" @submit.prevent="submit">
      <div class="server-heading">
        <h2 id="server-settings-title">Loom servers</h2>
        <span class="server-status">{{ selectedStatus }}</span>
      </div>
      <label>
        <span>Register or update backend</span>
        <select :value="validUrl" :disabled="saving" @change="url = ($event.target as HTMLSelectElement).value">
          <option value="" disabled>Choose a saved server</option>
          <option v-if="validUrl && !choices.some((p) => p.url === validUrl)" :value="validUrl">New server</option>
          <option v-for="server in choices" :key="server.url" :value="server.url">{{ server.name }} · {{ backends?.find(backend => backend.url === server.url)?.available ? 'Available' : 'Unavailable' }} · {{ server.url }}</option>
        </select>
      </label>
      <label>
        <span>Server URL</span>
        <input ref="urlInput" v-model="url" :disabled="saving" placeholder="http://127.0.0.1:7878" spellcheck="false" autocomplete="off" />
      </label>
      <label>
        <span>Server name <em>(optional)</em></span>
        <input v-model="name" :disabled="saving" placeholder="Local Loom, laptop, experiments…" />
      </label>
      <div class="server-rename">
        <button type="button" :disabled="!canRename || saving" @click="rename">Rename server</button>
        <span class="hint">Updates the name everywhere. Keeps this connection and default.</span>
      </div>
      <label>
        <span>Bearer token <em>(optional on loopback)</em></span>
        <input v-model="token" :disabled="saving || tokenLoading" :placeholder="tokenLoading ? 'Loading from Keychain…' : 'Token for this server'" type="password" autocomplete="off" spellcheck="false" />
      </label>
      <div class="hint">All registered backends appear together. Save makes this the default for new projects and ungrouped tracks. Each project stays on its own backend. Tokens are stored per server in Keychain.</div>
      <div v-if="url.trim() && !validUrl" class="server-error" role="alert">Enter an HTTP(S) URL without credentials, query, or fragment.</div>
      <div v-if="error || localError" class="server-error" role="alert">{{ localError || error }}</div>
      <div class="modal-actions">
        <button type="button" :disabled="saving" @click="emit('close')">Cancel</button>
        <button class="primary" type="submit" :disabled="!validUrl || tokenLoading || saving">{{ saving ? 'Connecting…' : 'Save & set default' }}</button>
      </div>
    </form>
  </div>
</template>

<style scoped>
.server-heading { display: flex; justify-content: space-between; align-items: center; gap: 16px; }
.server-heading h2 { margin: 0; }
.server-status { color: var(--muted); font-size: 12px; }
.server-settings select { width: 100%; min-width: 0; padding: 9px; border: 1px solid var(--border); border-radius: 6px; background: var(--bg); color: var(--text); }
.server-rename { display: flex; align-items: center; gap: 12px; }
.server-error { color: var(--danger, #d76969); font-size: 12px; }
</style>
