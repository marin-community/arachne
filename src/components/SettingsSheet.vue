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
  (e: "save", url: string, token: string | undefined, name: string): void;
  (e: "set-default", url: string): void;
  (e: "rename", url: string, name: string): void;
}>();

const url = ref(props.url);
const selectedUrl = ref(props.url);
const token = ref(props.token);
// null means unknown; an empty string means Keychain confirmed no token.
const loadedToken = ref<string | null>(null);
const tokenReadError = ref("");
const profiles = ref(readServerProfiles(localStorage));
const name = ref(profiles.value.find((p) => p.url === props.url)?.name || props.backends?.find(p => p.url === props.url)?.name || new URL(props.url).host);
const tokenLoading = ref(false);
const localError = ref("");
const urlInput = ref<HTMLInputElement | null>(null);
const validUrl = computed(() => {
  try { return normalizeServerUrl(url.value); } catch { return ""; }
});
const savedProfile = computed(() => profiles.value.find(profile => profile.url === validUrl.value));
const choices = computed(() => {
  const rows = [...profiles.value];
  for (const choice of [...(props.backends ?? []), { name: "Local Loom", url: "http://127.0.0.1:7878" }, { name: "Current server", url: props.url }]) {
    const normalized = normalizeServerUrl(choice.url);
    if (!rows.some(p => p.url === normalized)) rows.push({ name: choice.name, url: normalized });
  }
  return rows;
});
const knownProfile = computed(() => choices.value.find(profile => profile.url === validUrl.value));
const canRename = computed(() => !!knownProfile.value && validUrl.value === selectedUrl.value && name.value.trim() !== knownProfile.value.name);
const tokenChanged = computed(() => token.value.trim() !== (loadedToken.value ?? ""));
const canSetDefault = computed(() => !!props.backends?.some(backend => backend.url === validUrl.value) && validUrl.value === selectedUrl.value && validUrl.value !== props.url && !canRename.value && !tokenChanged.value);
function selectServer(value: string) {
  selectedUrl.value = value;
  url.value = value;
  name.value = choices.value.find(p => p.url === value)?.name || "";
}
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
let tokenRequest = 0;
let tokenTimer: ReturnType<typeof setTimeout> | undefined;
async function loadToken(normalized: string, request: number) {
  try {
    const stored = await invoke<string | null>("load_token", { baseUrl: normalized });
    if (request === tokenRequest) {
      token.value = loadedToken.value = stored || "";
      tokenReadError.value = "";
    }
  } catch (error: any) {
    if (request === tokenRequest) tokenReadError.value = `Could not read this server's Keychain token: ${error?.message ?? String(error)}`;
  } finally {
    if (request === tokenRequest) tokenLoading.value = false;
  }
}
function retryTokenRead() {
  if (!validUrl.value || tokenLoading.value || props.saving) return;
  clearTimeout(tokenTimer);
  token.value = "";
  loadedToken.value = null;
  tokenLoading.value = true;
  return loadToken(validUrl.value, ++tokenRequest);
}
watch(validUrl, () => {
  const request = ++tokenRequest;
  clearTimeout(tokenTimer);
  // Clear immediately: editing a host must never forward another server's token.
  token.value = "";
  loadedToken.value = null;
  tokenReadError.value = "";
  localError.value = "";
  const normalized = validUrl.value;
  tokenLoading.value = !!normalized;
  if (!normalized) return;
  name.value = choices.value.find((p) => p.url === normalized)?.name || "";
  tokenTimer = setTimeout(() => loadToken(normalized, request), 200);
});

let submittedToken: string | undefined;
function submit() {
  if (!validUrl.value || tokenLoading.value || props.saving) return;
  try {
    profiles.value = rememberServer(localStorage, { name: name.value, url: validUrl.value }, { requirePersistence: true });
    name.value = savedProfile.value!.name;
    selectedUrl.value = validUrl.value;
    localError.value = "";
    submittedToken = tokenChanged.value ? token.value.trim() : undefined;
    emit("save", validUrl.value, submittedToken, name.value);
  } catch (error: any) {
    localError.value = `Could not save server configuration: ${error?.message ?? String(error)}`;
  }
}
watch(() => props.saving, (saving, previous) => {
  if (previous && !saving) {
    // Only a confirmed credential write establishes a new baseline. A
    // metadata save cannot resolve a failed or unknown Keychain read.
    if (!props.error && submittedToken !== undefined) {
      loadedToken.value = submittedToken;
      tokenReadError.value = "";
    }
    submittedToken = undefined;
  }
});
function onKeydown(event: KeyboardEvent) {
  if (event.key === "Escape" && !props.saving) { event.preventDefault(); emit("close"); }
}
onMounted(() => {
  document.addEventListener("keydown", onKeydown);
  urlInput.value?.focus();
  return retryTokenRead();
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
        <span>Server to edit</span>
        <select :value="validUrl" :disabled="saving" @change="selectServer(($event.target as HTMLSelectElement).value)">
          <option value="" disabled>Choose a saved server</option>
          <option v-if="validUrl && !choices.some((p) => p.url === validUrl)" :value="validUrl">New server</option>
          <option v-for="server in choices" :key="server.url" :value="server.url">{{ server.name }} · {{ backends?.find(backend => backend.url === server.url)?.available ? 'Available' : 'Unavailable' }} · {{ server.url }}{{ server.url === props.url ? ' · Default' : '' }}</option>
        </select>
      </label>
      <div v-if="validUrl && validUrl !== selectedUrl" class="hint">Saving this address adds or updates that endpoint. The original server and its projects stay at their current address. Enter credentials for this endpoint.</div>
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
      <div v-if="tokenReadError" class="server-error" role="alert">
        {{ tokenReadError }}
        <button type="button" :disabled="saving || tokenLoading" @click="retryTokenRead">Retry token read</button>
      </div>
      <div class="hint">Save configuration keeps the default and existing projects. Set default applies to new projects and ungrouped tracks. Tokens are stored per server in Keychain.</div>
      <div v-if="url.trim() && !validUrl" class="server-error" role="alert">Enter an HTTP(S) URL without credentials, query, or fragment.</div>
      <div v-if="error || localError" class="server-error" role="alert">{{ localError || error }}</div>
      <div class="modal-actions">
        <button type="button" :disabled="saving" @click="emit('close')">Cancel</button>
        <button type="button" :disabled="!canSetDefault || tokenLoading || saving" @click="emit('set-default', validUrl)">Set default</button>
        <button class="primary" type="submit" :disabled="!validUrl || tokenLoading || saving">{{ saving ? 'Saving…' : 'Save configuration' }}</button>
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
