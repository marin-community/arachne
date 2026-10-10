export type StrategyKind = "land" | "publish";
export interface StrategyOption { value: string; label: string }

export function landStrategies(hasCommits = true): StrategyOption[] {
  return [
    { value: "squash", label: "Squash into canonical branch" },
    { value: "merge", label: "Merge into canonical branch" },
    { value: "rebase", label: "Rebase onto canonical branch" },
    ...(hasCommits ? [{ value: "cherry-pick", label: "Cherry-pick commits" }] : []),
  ];
}
export function publishStrategies(local: boolean): StrategyOption[] {
  return [
    { value: "open-pr", label: "Single PR" },
    { value: "pr-stack", label: "PR stack" },
    ...(local ? [{ value: "land-locally", label: "Squash into local primary current branch" }] : []),
    { value: "squash", label: "Squash into upstream" },
    { value: "merge", label: "Merge into upstream" },
    { value: "rebase", label: "Rebase / fast-forward" },
    { value: "push", label: "Push canonical branch" },
  ];
}

// Versioned keys prevent old root Land choices becoming worker Land choices.
export const strategyMemoryKey = (kind: StrategyKind, repo: string) => `arachne:strategy:v2:${kind}:${repo}`;
export function selectedStrategy(kind: StrategyKind, options: readonly StrategyOption[], remembered: string | null): string {
  const fallback = kind === "land" ? "squash" : "open-pr";
  return options.find(option => option.value === remembered)?.value
    ?? options.find(option => option.value === fallback)?.value ?? options[0]?.value ?? "";
}
