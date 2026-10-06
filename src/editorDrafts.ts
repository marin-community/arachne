// Keep manual drafts while navigating between threads without persisting source
// contents to localStorage. Reloading the application still prompts via beforeunload.
export interface EditorDraft { original: string; content: string; checkout: string }
export const editorDrafts = new Map<string, EditorDraft>();

// Closing the editor keeps drafts available, so warn on application reload even
// when no editor modal is currently mounted.
if (typeof window !== "undefined") {
  window.addEventListener("beforeunload", event => {
    if (editorDrafts.size) { event.preventDefault(); event.returnValue = ""; }
  });
}
