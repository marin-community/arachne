<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";

interface WorkSummary {
  files: number;
  additions: number;
  deletions: number;
  has_commits: boolean;
}

const props = defineProps<{
  checkoutPath: string;
  recovering: boolean;
  repoLabel: string;
  repoUrl: string | null;
  prUrl: string | null;
  prNumber: number | null;
  prDraft?: boolean | null;
  prState?: string | null;
  reviewDecision?: string | null;
  checks?: string | null;
  branch: string;
  integrationBranch?: string | null;
  workSummary: WorkSummary | null;
}>();

const emit = defineEmits<{
  (e: "open-checkout"): void;
  (e: "open-terminal"): void;
  (e: "open-resource", url: string): void;
}>();

const open = ref(false);
const root = ref<HTMLElement | null>(null);

function onDocumentClick(event: MouseEvent) {
  if (root.value && !root.value.contains(event.target as Node)) open.value = false;
}

function openResource(url: string | null) {
  if (!url) return;
  open.value = false;
  emit("open-resource", url);
}

function openCheckout() {
  open.value = false;
  emit("open-checkout");
}

function openTerminal() {
  open.value = false;
  emit("open-terminal");
}

onMounted(() => document.addEventListener("mousedown", onDocumentClick));
onUnmounted(() => document.removeEventListener("mousedown", onDocumentClick));
</script>

<template>
  <div ref="root" class="code-actions">
    <button
      class="code-primary"
      :class="{ recover: !checkoutPath }"
      :disabled="recovering"
      :title="checkoutPath ? `Open ${checkoutPath} in Zed` : 'Materialize this branch and open it in Zed'"
      @click="openCheckout"
    >
      {{ recovering ? "Recovering…" : checkoutPath ? "Open in Zed" : "Recover checkout" }}
    </button>
    <button
      class="code-caret"
      :aria-expanded="open"
      aria-label="More code actions and checkout details"
      title="Code actions and checkout details"
      @click.stop="open = !open"
    >▾</button>

    <div v-if="open" class="code-menu">
      <div class="code-menu-title">Code checkout</div>
      <dl>
        <div>
          <dt>Repository</dt>
          <dd>
            <button v-if="repoUrl" class="code-link" @click="openResource(repoUrl)">{{ repoLabel }}</button>
            <span v-else>{{ repoLabel || "Unavailable" }}</span>
          </dd>
        </div>
        <div v-if="prUrl || prNumber">
          <dt>Pull request</dt>
          <dd>
            <button v-if="prUrl" class="code-link" @click="openResource(prUrl)">PR #{{ prNumber }}</button>
            <span v-else>PR #{{ prNumber }}</span>
            <span v-if="prDraft" class="code-status">draft</span>
            <span v-if="prState" class="code-status">{{ prState }}</span>
            <span v-if="reviewDecision" class="code-status">{{ reviewDecision.toLowerCase().replaceAll('_', ' ') }}</span>
            <span v-if="checks" class="code-status">CI {{ checks }}</span>
          </dd>
        </div>
        <div>
          <dt>Branch</dt>
          <dd class="mono">{{ branch || "Unavailable" }}</dd>
        </div>
        <div v-if="integrationBranch">
          <dt>Integrates into</dt>
          <dd class="mono">{{ integrationBranch }}</dd>
        </div>
        <div>
          <dt>Checkout</dt>
          <dd class="mono path">{{ checkoutPath || "No active checkout" }}</dd>
        </div>
        <div v-if="workSummary">
          <dt>Changes</dt>
          <dd>{{ workSummary.files }} files <span class="additions">+{{ workSummary.additions }}</span> <span class="deletions">−{{ workSummary.deletions }}</span></dd>
        </div>
      </dl>
      <button v-if="checkoutPath" class="terminal-action" @click="openTerminal">Open Terminal</button>
    </div>
  </div>
</template>

<style scoped>
.code-actions {
  position: relative;
  display: inline-flex;
  align-items: stretch;
}
.code-primary {
  border-radius: 6px 0 0 6px;
  border-color: var(--accent);
  color: var(--accent);
  white-space: nowrap;
}
.code-primary.recover { color: #e0af68; border-color: #e0af68; }
.code-caret {
  border-left: 0;
  border-radius: 0 6px 6px 0;
  padding-inline: 7px;
  color: var(--text-dim);
}
.code-menu {
  position: absolute;
  z-index: 50;
  top: calc(100% + 6px);
  right: 0;
  width: min(360px, calc(100vw - 32px));
  padding: 12px;
  background: var(--bg-raised);
  border: 1px solid var(--border);
  border-radius: 8px;
  box-shadow: 0 12px 32px rgba(0, 0, 0, .5);
}
.code-menu-title { margin-bottom: 8px; font-weight: 650; }
dl { display: grid; gap: 7px; margin: 0; }
dl > div { display: grid; grid-template-columns: 88px minmax(0, 1fr); gap: 10px; }
dt { color: var(--text-dim); }
dd { min-width: 0; margin: 0; user-select: text; }
.mono { font-family: var(--mono); font-size: 11px; }
.path { overflow-wrap: anywhere; }
.code-link {
  max-width: 100%;
  padding: 0;
  border: 0;
  background: transparent;
  color: var(--accent);
  overflow: hidden;
  text-overflow: ellipsis;
  vertical-align: bottom;
}
.code-link:hover { background: transparent; text-decoration: underline; }
.code-status {
  margin-left: 5px;
  padding: 1px 4px;
  border: 1px solid var(--border);
  border-radius: 4px;
  color: var(--text-dim);
  font: 10px var(--mono);
}
.additions { color: var(--ok); }
.deletions { color: var(--blocked); }
.terminal-action { width: 100%; margin-top: 11px; }
</style>
