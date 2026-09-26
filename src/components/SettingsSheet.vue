<script setup lang="ts">
import { ref, watch } from "vue";

const props = defineProps<{
  url: string;
  token: string;
  connected: boolean;
}>();

const emit = defineEmits<{
  (e: "close"): void;
  (e: "save", url: string, token: string): void;
}>();

const url = ref(props.url);
const token = ref(props.token);
</script>

<template>
  <div class="modal-backdrop" @click.self="emit('close')">
    <div class="modal">
      <h2>Loom Connection</h2>
      <label>
        <span>Server URL</span>
        <input v-model="url" placeholder="http://127.0.0.1:7878" spellcheck="false" />
      </label>
      <label>
        <span>Bearer token <em>(leave empty on loopback — local loom trusts it)</em></span>
        <input v-model="token" placeholder="loom token for remote (optional)" type="password" spellcheck="false" />
      </label>
      <div class="hint">
        Local dev: <code>http://127.0.0.1:7878</code>, no token. Remote (DGX over
        Tailscale): the server URL plus a token from <code>loom tokens create</code>.
      </div>
      <div class="modal-actions">
        <button @click="emit('close')">Cancel</button>
        <button
          class="primary"
          :disabled="!url.trim()"
          @click="emit('save', url.trim(), token.trim())"
        >
          Save &amp; Connect
        </button>
      </div>
    </div>
  </div>
</template>
