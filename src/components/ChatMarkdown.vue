<script setup lang="ts">
import { computed } from "vue";
import DOMPurify from "dompurify";
import { marked } from "marked";

const props = defineProps<{ text: string }>();

const imageDataUrl = /^data:image\/(?:png|jpeg|gif|webp|avif);base64,[a-z\d+/=]+$/i;

function safeUrl(value: string, image: boolean): boolean {
  const url = value.trim();
  if (image && imageDataUrl.test(url)) return true;
  try {
    const parsed = new URL(url);
    return parsed.protocol === "https:" || parsed.protocol === "http:" ||
      (!image && parsed.protocol === "mailto:");
  } catch {
    return false;
  }
}

const html = computed(() => {
  // GFM includes tables, strikethrough, autolinks, and task lists.
  const rendered = marked.parse(props.text, { gfm: true, breaks: false, async: false });
  const clean = DOMPurify.sanitize(rendered, {
    USE_PROFILES: { html: true },
    FORBID_TAGS: ["form", "button", "textarea", "select", "style", "iframe", "object", "embed", "picture", "source", "video", "audio"],
    FORBID_ATTR: ["style", "srcset"],
  });

  // DOMPurify removes active content. Restrict navigation and image loads as
  // well: the WebView has no CSP, and transcript text is untrusted input.
  const template = document.createElement("template");
  template.innerHTML = clean;
  template.content.querySelectorAll("a").forEach((link) => {
    if (!safeUrl(link.getAttribute("href") ?? "", false)) link.removeAttribute("href");
    else {
      link.target = "_blank";
      link.rel = "noopener noreferrer";
    }
  });
  template.content.querySelectorAll("img").forEach((img) => {
    if (!safeUrl(img.getAttribute("src") ?? "", true)) {
      img.replaceWith(document.createTextNode(img.alt));
      return;
    }
    img.loading = "lazy";
    img.decoding = "async";
    img.referrerPolicy = "no-referrer";
  });
  template.content.querySelectorAll("input").forEach((input) => {
    if (input.type !== "checkbox") input.remove();
    else input.disabled = true;
  });
  return template.innerHTML;
});
</script>

<template>
  <!-- data-markdown keeps the raw source on the rendered node so the
       conversation's copy handler can rebuild a markdown clipboard flavor
       (the DOM itself only holds rendered HTML). -->
  <div class="chat-markdown" :data-markdown="text" v-html="html"></div>
</template>
