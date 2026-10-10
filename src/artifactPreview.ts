import { ref } from 'vue';
import type { ArtifactTarget } from './artifactLinks';

export interface ArtifactContent { title: string; content: string }

/** Each navigation owns its response, including failures and loading state. */
export function createArtifactPreview(read: (target: ArtifactTarget) => Promise<ArtifactContent>) {
  const target = ref<ArtifactTarget | null>(null);
  const view = ref<ArtifactContent | null>(null);
  const loading = ref(false);
  const error = ref('');
  let generation = 0;
  function close() {
    generation++;
    target.value = null;
    view.value = null;
    loading.value = false;
    error.value = '';
  }
  async function open(next: ArtifactTarget) {
    const request = ++generation;
    target.value = next;
    view.value = null;
    error.value = '';
    loading.value = true;
    try {
      const result = await read(next);
      if (request === generation) view.value = result;
    } catch (cause: any) {
      if (request === generation) error.value = `Artifact unavailable: ${cause?.message ?? String(cause)}`;
    } finally {
      if (request === generation) loading.value = false;
    }
  }
  return { target, view, loading, error, open, close };
}
