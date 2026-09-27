<script setup lang="ts">
import { ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { ChatDisplayBlock } from "../chatRows";

const props = defineProps<{ block: ChatDisplayBlock; sessionId: string }>();

interface ImagePart { key: string; src: string | null; alt: string }
interface ResourcePart { type?: unknown; name?: unknown }
const images = ref<ImagePart[]>([]);
const resourceCache = new Map<string, Promise<string | null>>();

function resourceImage(sessionId: string, name: string): Promise<string | null> {
  const key = `${sessionId}\0${name}`;
  const cached = resourceCache.get(key);
  if (cached) return cached;
  const request = invoke<string>("load_session_image", { id: sessionId, name }).catch(() => null);
  if (resourceCache.size >= 8) resourceCache.delete(resourceCache.keys().next().value!);
  resourceCache.set(key, request);
  return request;
}

function imageDataUrl(part: unknown): string | null {
  if (!part || typeof part !== "object") return null;
  const item = part as Record<string, unknown>;
  if (item.type !== "image" || typeof item.data !== "string") return null;
  const mime = item.mime_type ?? item.mimeType;
  if (typeof mime !== "string" || !/^image\/(png|jpeg|gif|webp|avif)$/.test(mime)) return null;
  if (!/^[A-Za-z0-9+/]+={0,2}$/.test(item.data)) return null;
  return `data:${mime};base64,${item.data}`;
}

function imageResourceNames(block: ChatDisplayBlock): string[] {
  if (!block.payload || typeof block.payload !== "object") return [];
  const resources = (block.payload as { resources?: unknown }).resources;
  if (!Array.isArray(resources)) return [];
  return resources
    .filter((part): part is ResourcePart => !!part && typeof part === "object")
    .filter((part) => part.type === "resource_link" && typeof part.name === "string" &&
      /\.(?:png|jpe?g|gif|webp|avif|bmp|ico)$/i.test(part.name))
    .map((part) => part.name as string);
}

watch(() => [props.block, props.sessionId] as const, async (_next, _old, onCleanup) => {
  let cancelled = false;
  onCleanup(() => { cancelled = true; });
  const inline = (props.block.content ?? []).flatMap((part, index) => {
    const src = imageDataUrl(part);
    return src ? [{ key: `inline:${index}`, src, alt: `Tool image ${index + 1}` }] : [];
  });
  images.value = inline;
  const names = imageResourceNames(props.block);
  const loaded = await Promise.all(names.map(async (name) => {
    const src = await resourceImage(props.sessionId, name);
    // Non-image resources or files no longer in the worktree have no preview.
    return { key: `resource:${name}`, src, alt: name };
  }));
  if (!cancelled) images.value = [...inline, ...loaded];
}, { immediate: true });
</script>

<template>
  <div v-if="images.length" class="chat-images">
    <template v-for="image in images" :key="image.key">
      <a v-if="image.src" :href="image.src" target="_blank" rel="noopener noreferrer">
        <img :src="image.src" :alt="image.alt" loading="lazy" />
      </a>
      <span v-else class="chat-image-unavailable">Image unavailable: {{ image.alt }}</span>
    </template>
  </div>
</template>
