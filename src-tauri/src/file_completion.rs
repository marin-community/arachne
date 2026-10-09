//! Repository file completion before a track has its own session checkout.

use std::path::Path;
use tokio::process::Command;

pub async fn repository_files(repository: &Path, query: &str) -> Result<Vec<String>, String> {
    // Resolve the root so a checkout subdirectory still offers the whole tree.
    let root = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(repository)
        .output()
        .await
        .map_err(|error| error.to_string())?;
    if !root.status.success() {
        return Err(String::from_utf8_lossy(&root.stderr).trim().to_owned());
    }
    let root = String::from_utf8(root.stdout).map_err(|error| error.to_string())?;
    let root = Path::new(root.trim_end_matches(['\n', '\r']));
    let output = Command::new("git")
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .current_dir(root)
        .output()
        .await
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    let needle = query.trim().to_lowercase();
    let mut files = Vec::new();
    for raw in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|raw| !raw.is_empty())
    {
        let Ok(path) = String::from_utf8(raw.to_vec()) else {
            continue;
        };
        if (needle.is_empty() || path.to_lowercase().contains(&needle))
            && root.join(&path).is_file()
        {
            files.push(path);
        }
    }
    files.sort_by_key(|path| {
        let lower = path.to_lowercase();
        let name = lower.rsplit('/').next().unwrap_or(&lower);
        (
            !lower.starts_with(&needle),
            !name.starts_with(&needle),
            path.len(),
            lower,
        )
    });
    files.dedup();
    files.truncate(40);
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn completes_the_whole_tree_including_untracked_files_and_respecting_ignores() {
        let root = std::env::temp_dir().join(format!("arachne-completion-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src/nested")).unwrap();
        let git = |args: &[&str]| {
            assert!(std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .output()
                .unwrap()
                .status
                .success());
        };
        git(&["init"]);
        std::fs::write(root.join(".gitignore"), "ignored/\n*.log\n").unwrap();
        std::fs::write(root.join(".git/info/exclude"), "excluded.txt\n").unwrap();
        std::fs::write(root.join("README.md"), "tracked").unwrap();
        std::fs::write(root.join("deleted.txt"), "deleted").unwrap();
        git(&["add", "README.md", "deleted.txt", ".gitignore"]);
        std::fs::remove_file(root.join("deleted.txt")).unwrap();
        std::fs::write(root.join("src/nested/fresh.ts"), "untracked").unwrap();
        std::fs::write(root.join("src/nested/debug.log"), "ignored").unwrap();
        std::fs::write(root.join("excluded.txt"), "ignored").unwrap();
        std::fs::create_dir_all(root.join("ignored")).unwrap();
        std::fs::write(root.join("ignored/file.ts"), "ignored").unwrap();
        let files = repository_files(&root.join("src"), "").await.unwrap();
        assert!(files.contains(&"README.md".to_owned()));
        assert!(files.contains(&"src/nested/fresh.ts".to_owned()));
        assert!(!files.iter().any(|path| path.contains("ignored")
            || path.ends_with(".log")
            || path == "excluded.txt"
            || path == "deleted.txt"));
        assert_eq!(
            repository_files(&root, "FRESH").await.unwrap(),
            ["src/nested/fresh.ts"]
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
