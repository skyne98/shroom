//! Git plumbing for two features, both shelling out to `git`:
//!
//! * **Rewind.** Each session has a hidden snapshot repo (its own git dir,
//!   with the user's checkout as its work tree). A snapshot before and after
//!   every turn records which paths the turn changed; rewinding puts exactly
//!   those paths back as they were before the chosen prompt. The user's own
//!   repository, index and history are never touched.
//! * **Worktrees.** A subagent spawned with `isolation: 'worktree'` works in
//!   `<repo>/.smash/worktrees/<name>` on branch `smash/<name>`, made from
//!   HEAD (Claude Code's layout). It is removed when the agent leaves no
//!   changes and kept for review otherwise.

use std::collections::{BTreeSet, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

/// Run git in `cwd`; its stdout, or its stderr as the error.
fn git(cwd: &Path, args: &[&str], input: Option<&[u8]>) -> Result<String, String> {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(cwd)
        // Paths are file names, never patterns.
        .env("GIT_LITERAL_PATHSPECS", "1")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|err| format!("git: {err}"))?;
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin
            .write_all(input)
            .map_err(|err| format!("git: {err}"))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|err| format!("git: {err}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(err.trim().to_string())
    }
}

/// NUL-separated git output as a list.
fn names(out: &str) -> Vec<String> {
    out.split('\0')
        .filter(|name| !name.is_empty())
        .map(String::from)
        .collect()
}

/// The top folder of the git checkout around `cwd`.
pub fn toplevel(cwd: &Path) -> Result<PathBuf, String> {
    let out = git(cwd, &["rev-parse", "--show-toplevel"], None)?;
    Ok(PathBuf::from(out.trim()))
}

/// A session's snapshot repo over the user's checkout.
pub struct Shadow {
    git_dir: PathBuf,
    work: PathBuf,
}

impl Shadow {
    /// The snapshot repo kept in `dir` for the git checkout around `cwd`.
    /// None outside a git checkout: rewind then restores only the chat
    /// (without a .gitignore a snapshot could swallow build output).
    pub fn open(dir: &Path, cwd: &Path) -> Option<Self> {
        let work = toplevel(cwd).ok()?;
        let git_dir = dir.join("git");
        if !git_dir.join("HEAD").exists() {
            std::fs::create_dir_all(dir).ok()?;
            git(dir, &["init", "-q", "--bare", "git"], None).ok()?;
            // Snapshots are trees kept by id, never committed: gc must not
            // collect them. Subagent worktrees are not the user's files.
            git(&git_dir, &["config", "gc.auto", "0"], None).ok()?;
            std::fs::create_dir_all(git_dir.join("info")).ok()?;
            std::fs::write(git_dir.join("info").join("exclude"), ".smash/\n").ok()?;
        }
        Some(Self { git_dir, work })
    }

    fn git(&self, args: &[&str], input: Option<&[u8]>) -> Result<String, String> {
        let git_dir = format!("--git-dir={}", self.git_dir.display());
        let work = format!("--work-tree={}", self.work.display());
        let mut all = vec![git_dir.as_str(), work.as_str()];
        all.extend_from_slice(args);
        git(&self.work, &all, input)
    }

    /// Snapshot the checkout (what git would track: .gitignore applies).
    /// ponytail: the first snapshot hashes every file once; git's stat cache
    /// makes later ones cheap (25 ms, then 1 ms here). A size cap per file if
    /// a repo with huge unignored files ever makes that slow.
    pub fn snap(&self) -> Result<String, String> {
        self.git(
            &[
                "-c",
                "advice.addEmbeddedRepo=false",
                "add",
                "-A",
                "--ignore-errors",
            ],
            None,
        )
        // Unreadable files are skipped, not fatal.
        .or_else(|err| {
            if err.contains("unable to") {
                Ok(String::new())
            } else {
                Err(err)
            }
        })?;
        Ok(self.git(&["write-tree"], None)?.trim().to_string())
    }

    /// Paths that differ between two snapshots.
    pub fn changed(&self, from: &str, to: &str) -> Result<Vec<String>, String> {
        let out = self.git(
            &[
                "diff-tree",
                "-r",
                "-z",
                "--name-only",
                "--no-renames",
                from,
                to,
            ],
            None,
        )?;
        Ok(names(&out))
    }

    /// Put `paths` back as they are in snapshot `tree`: files it has are
    /// restored, files it lacks are deleted.
    pub fn restore(&self, tree: &str, paths: &BTreeSet<String>) -> Result<(), String> {
        let known: HashSet<String> =
            names(&self.git(&["ls-tree", "-r", "-z", "--name-only", tree], None)?)
                .into_iter()
                .collect();
        let (back, gone): (Vec<&String>, Vec<&String>) =
            paths.iter().partition(|path| known.contains(*path));
        if !back.is_empty() {
            let list: Vec<u8> = back
                .iter()
                .flat_map(|path| path.bytes().chain([0]))
                .collect();
            self.git(
                &[
                    "checkout",
                    tree,
                    "--pathspec-from-file=-",
                    "--pathspec-file-nul",
                ],
                Some(&list),
            )?;
        }
        for path in gone {
            let file = self.work.join(path);
            match std::fs::remove_file(&file) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(format!("{path}: {err}")),
            }
            // Folders the turn made, now empty.
            let mut dir = file.parent();
            while let Some(at) = dir.filter(|at| *at != self.work) {
                if std::fs::remove_dir(at).is_err() {
                    break;
                }
                dir = at.parent();
            }
        }
        Ok(())
    }
}

/// One prompt the user sent: where the chat and the files stood before it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mark {
    pub prompt: String,
    /// Transcript length (bytes) before the prompt.
    pub body: usize,
    /// Conversation length (messages) before the prompt.
    pub messages: usize,
    /// Snapshot before the turn; None outside a git checkout.
    pub before: Option<String>,
    /// Paths the turn changed; None until it ends (or if it never did).
    pub changed: Option<Vec<String>>,
    /// Unix seconds.
    pub at: u64,
}

impl Mark {
    pub fn new(prompt: &str, body: usize, messages: usize) -> Self {
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        Self {
            prompt: prompt.to_string(),
            body,
            messages,
            before: None,
            changed: None,
            at,
        }
    }
}

/// The session's marks, oldest first.
pub fn marks(dir: &Path) -> Vec<Mark> {
    std::fs::read_to_string(dir.join("marks.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn save_marks(dir: &Path, marks: &[Mark]) {
    if std::fs::create_dir_all(dir).is_ok()
        && let Ok(text) = serde_json::to_string(marks)
    {
        let _ = std::fs::write(dir.join("marks.json"), text);
    }
}

/// Before a turn: snapshot the checkout and log the mark. The snapshot
/// repo, for [`end_turn`].
pub fn begin_turn(dir: &Path, cwd: &Path, mut mark: Mark) -> Option<(Shadow, String)> {
    let shadow = Shadow::open(dir, cwd);
    let before = shadow.as_ref().and_then(|shadow| shadow.snap().ok());
    mark.before = before.clone();
    let mut all = marks(dir);
    all.push(mark);
    save_marks(dir, &all);
    Some((shadow?, before?))
}

/// After a turn: record what it changed in the last mark.
pub fn end_turn(dir: &Path, (shadow, before): (Shadow, String)) {
    let Ok(after) = shadow.snap() else {
        return;
    };
    let Ok(changed) = shadow.changed(&before, &after) else {
        return;
    };
    let mut all = marks(dir);
    if let Some(last) = all.last_mut()
        && last.before.as_deref() == Some(before.as_str())
    {
        last.changed = Some(changed);
        save_marks(dir, &all);
    }
}

/// Paths the turns from mark `at` on changed (a turn that never ended counts
/// what changed since it began).
fn touched(shadow: &Shadow, marks: &[Mark]) -> Result<BTreeSet<String>, String> {
    let mut paths = BTreeSet::new();
    let mut now: Option<String> = None;
    for mark in marks {
        match (&mark.changed, &mark.before) {
            (Some(changed), _) => paths.extend(changed.iter().cloned()),
            (None, Some(before)) => {
                if now.is_none() {
                    now = Some(shadow.snap()?);
                }
                let now = now.as_deref().unwrap_or_default();
                paths.extend(shadow.changed(before, now)?);
            }
            (None, None) => {}
        }
    }
    Ok(paths)
}

/// Put every file the turns from mark `at` on changed back as it was before
/// that prompt. Files nobody in those turns touched (the user's own edits
/// elsewhere) stay as they are. Returns how many paths were restored.
pub fn rewind(dir: &Path, cwd: &Path, marks: &[Mark], at: usize) -> Result<usize, String> {
    let mark = marks.get(at).ok_or("no such prompt")?;
    let base = mark
        .before
        .as_deref()
        .ok_or("no snapshot before that prompt (not a git checkout then)")?;
    let shadow = Shadow::open(dir, cwd).ok_or("not in a git checkout")?;
    let paths = touched(&shadow, &marks[at..])?;
    shadow.restore(base, &paths)?;
    Ok(paths.len())
}

/// A subagent's own checkout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
}

/// The worktree for `name` of the checkout around `cwd`, made from HEAD
/// (or the one already there). Gitignored files listed in `.worktreeinclude`
/// (`.env`, local config) are copied in, as Claude Code does.
pub fn worktree(cwd: &Path, name: &str) -> Result<Worktree, String> {
    let root = toplevel(cwd).map_err(|_| {
        "worktree isolation needs a git repository; spawn without isolation".to_string()
    })?;
    let home = root.join(".smash");
    let path = home.join("worktrees").join(name);
    let branch = format!("smash/{name}");
    if path.join(".git").exists() {
        return Ok(Worktree { path, branch });
    }
    std::fs::create_dir_all(home.join("worktrees")).map_err(|err| err.to_string())?;
    // The folder hides itself from the user's `git status`.
    let ignore = home.join(".gitignore");
    if !ignore.exists() {
        let _ = std::fs::write(&ignore, "*\n");
    }
    git(&root, &["rev-parse", "--verify", "-q", "HEAD"], None)
        .map_err(|_| "the repository has no commit yet; commit once, then spawn".to_string())?;
    let at = path.to_string_lossy().into_owned();
    let exists = git(&root, &["rev-parse", "--verify", "-q", &branch], None).is_ok();
    let made = if exists {
        // Kept from an earlier run whose folder is gone: carry on with it.
        git(&root, &["worktree", "add", "-q", &at, &branch], None)
    } else {
        git(
            &root,
            &[
                "worktree",
                "add",
                "-q",
                "--no-track",
                "-b",
                &branch,
                &at,
                "HEAD",
            ],
            None,
        )
    };
    made.map_err(|err| format!("could not create the worktree: {err}"))?;
    if root.join(".worktreeinclude").exists() {
        let listed = git(
            &root,
            &[
                "ls-files",
                "-z",
                "--others",
                "--ignored",
                "--exclude-from=.worktreeinclude",
            ],
            None,
        )
        .unwrap_or_default();
        for file in names(&listed) {
            let from = root.join(&file);
            let to = path.join(&file);
            let plain = std::fs::symlink_metadata(&from).is_ok_and(|meta| meta.is_file());
            if plain && let Some(dir) = to.parent() {
                let _ = std::fs::create_dir_all(dir);
                let _ = std::fs::copy(&from, &to);
            }
        }
    }
    Ok(Worktree { path, branch })
}

/// Remove `worktree` and its branch if they hold nothing: no uncommitted
/// change and no commit the main checkout lacks. True when removed (or
/// already gone).
pub fn remove_if_clean(worktree: &Worktree) -> Result<bool, String> {
    if !worktree.path.exists() {
        return Ok(true);
    }
    let dirty = !git(&worktree.path, &["status", "--porcelain"], None)?
        .trim()
        .is_empty();
    let common = git(
        &worktree.path,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        None,
    )?;
    let root = Path::new(common.trim())
        .parent()
        .ok_or("no main checkout")?
        .to_path_buf();
    let range = format!("HEAD..{}", worktree.branch);
    let ahead = git(&root, &["rev-list", "--count", &range], None)?.trim() != "0";
    if dirty || ahead {
        return Ok(false);
    }
    let at = worktree.path.to_string_lossy().into_owned();
    git(&root, &["worktree", "remove", &at], None)?;
    git(&root, &["branch", "-D", "-q", &worktree.branch], None)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> PathBuf {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("smash-git-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.txt"), "one\n").unwrap();
        std::fs::write(root.join(".gitignore"), "*.log\n").unwrap();
        for args in [
            &["init", "-q"][..],
            &["config", "user.email", "t@t"],
            &["config", "user.name", "t"],
            &["add", "-A"],
            &["commit", "-qm", "init"],
        ] {
            git(&root, args, None).unwrap();
        }
        root
    }

    #[test]
    fn rewind_restores_only_what_the_turns_changed() {
        let root = repo();
        let dir = root.with_extension("marks");
        let _ = std::fs::remove_dir_all(&dir);
        // Turn 1 edits a file and adds one in a new folder.
        let turn = begin_turn(&dir, &root, Mark::new("first", 0, 0)).expect("git checkout");
        std::fs::write(root.join("src/a.txt"), "two\n").unwrap();
        std::fs::create_dir_all(root.join("new")).unwrap();
        std::fs::write(root.join("new/b.txt"), "b\n").unwrap();
        end_turn(&dir, turn);
        // Between turns the user edits a file of their own.
        std::fs::write(root.join("mine.txt"), "mine\n").unwrap();
        // Turn 2 never ends (the app died): what changed since counts.
        let _ = begin_turn(&dir, &root, Mark::new("second", 10, 2));
        std::fs::write(root.join("src/a.txt"), "three\n").unwrap();
        std::fs::write(root.join("x.log"), "ignored\n").unwrap();
        let all = marks(&dir);
        assert_eq!(all.len(), 2);
        assert_eq!(
            all[0].changed.as_deref(),
            Some(&["new/b.txt".to_string(), "src/a.txt".to_string()][..])
        );
        assert_eq!(all[1].changed, None);

        let restored = rewind(&dir, &root, &all, 0).unwrap();
        assert_eq!(restored, 2);
        assert_eq!(
            std::fs::read_to_string(root.join("src/a.txt")).unwrap(),
            "one\n"
        );
        assert!(
            !root.join("new").exists(),
            "the folder the turn made is gone"
        );
        assert!(root.join("mine.txt").exists(), "the user's own file stays");
        assert!(root.join("x.log").exists(), "ignored files are not tracked");
        // The user's repository is untouched.
        assert!(
            git(&root, &["status", "--porcelain"], None)
                .unwrap()
                .contains("mine.txt")
        );
        assert_eq!(
            git(&root, &["rev-list", "--count", "HEAD"], None)
                .unwrap()
                .trim(),
            "1"
        );
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn outside_git_there_is_nothing_to_snapshot() {
        let dir = std::env::temp_dir().join(format!("smash-nogit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(Shadow::open(&dir.join("marks"), Path::new("/")).is_none());
        assert!(begin_turn(&dir.join("marks"), Path::new("/"), Mark::new("p", 0, 0)).is_none());
        assert_eq!(
            marks(&dir.join("marks")).len(),
            1,
            "the chat can still rewind"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn worktrees_are_removed_when_empty_and_kept_with_work() {
        let root = repo();
        let empty = worktree(&root, "1-a1").unwrap();
        assert_eq!(empty.branch, "smash/1-a1");
        assert!(empty.path.join("src/a.txt").exists());
        assert!(
            git(&root, &["status", "--porcelain"], None)
                .unwrap()
                .is_empty(),
            ".smash hides itself"
        );
        assert!(remove_if_clean(&empty).unwrap());
        assert!(!empty.path.exists());
        assert!(git(&root, &["rev-parse", "--verify", "-q", "smash/1-a1"], None).is_err());

        let busy = worktree(&root, "1-a2").unwrap();
        std::fs::write(busy.path.join("src/a.txt"), "agent\n").unwrap();
        assert!(
            !remove_if_clean(&busy).unwrap(),
            "uncommitted work keeps it"
        );
        git(&busy.path, &["commit", "-qam", "agent"], None).unwrap();
        assert!(!remove_if_clean(&busy).unwrap(), "a commit keeps it");
        assert_eq!(
            std::fs::read_to_string(root.join("src/a.txt")).unwrap(),
            "one\n",
            "the main checkout is untouched"
        );
        // Its folder deleted by hand: the branch carries on.
        git(
            &root,
            &[
                "worktree",
                "remove",
                "--force",
                &busy.path.to_string_lossy(),
            ],
            None,
        )
        .unwrap();
        let again = worktree(&root, "1-a2").unwrap();
        assert_eq!(
            std::fs::read_to_string(again.path.join("src/a.txt")).unwrap(),
            "agent\n"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn worktreeinclude_copies_ignored_config() {
        let root = repo();
        std::fs::write(root.join("secret.log"), "key\n").unwrap();
        std::fs::write(root.join("other.log"), "no\n").unwrap();
        std::fs::write(root.join(".worktreeinclude"), "secret.log\n").unwrap();
        let tree = worktree(&root, "1-a3").unwrap();
        assert!(tree.path.join("secret.log").exists());
        assert!(!tree.path.join("other.log").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
