//! phitex-git against a throwaway repository made here, under `target/`
//! (with the `git` command, for the fixture only): commits, a branch, an
//! annotated tag, staged and unstaged changes, a project in a
//! subdirectory.

use phitex_diff::{ChangeKind, Dir, Files, Options, diff};
use phitex_git::{Error, RefKind, Repo, STAGED};
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args([
            "-c",
            "init.defaultBranch=main",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn write(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

const MAIN: &str = "\\documentclass{article}\n\\begin{document}\n\\input{intro}\n\\input{old}\nEnd.\n\\end{document}\n";

/// The fixture: `paper/` in a repository; returns its root.
fn fixture(name: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/phitex-git-fixture")
        .join(name);
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    let paper = root.join("paper");
    write(&paper.join("main.tex"), MAIN);
    write(&paper.join("intro.tex"), "Version one of the intro.\n");
    write(
        &paper.join("old.tex"),
        "A file only the first version inputs.\n",
    );
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "first version"]);
    git(&root, &["tag", "-a", "v1", "-m", "submitted"]);
    git(&root, &["branch", "draft"]);
    write(
        &paper.join("main.tex"),
        &MAIN.replace("\\input{old}", "\\input{new}"),
    );
    write(&paper.join("intro.tex"), "Version two of the intro.\n");
    write(&paper.join("new.tex"), "A file the second version adds.\n");
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "second version"]);
    // (staged, then a further change not staged)
    write(&paper.join("intro.tex"), "Version three, staged.\n");
    git(&root, &["add", "paper/intro.tex"]);
    write(
        &paper.join("intro.tex"),
        "Version four, in the working tree.\n",
    );
    root
}

#[test]
fn versions() {
    let root = fixture("versions");
    let repo = Repo::discover(&root.join("paper")).unwrap();
    assert_eq!(repo.prefix(), "paper/");
    let read = |rev: &str, p: &str| repo.snapshot(rev).unwrap().read(p);
    assert_eq!(
        read("HEAD", "intro.tex").as_deref(),
        Some("Version two of the intro.\n")
    );
    assert_eq!(
        read("HEAD~1", "intro.tex").as_deref(),
        Some("Version one of the intro.\n")
    );
    assert_eq!(
        read("v1", "intro.tex").as_deref(),
        Some("Version one of the intro.\n")
    );
    assert_eq!(
        read("draft", "old.tex").as_deref(),
        Some("A file only the first version inputs.\n")
    );
    assert_eq!(
        read("main", "new.tex").as_deref(),
        Some("A file the second version adds.\n")
    );
    assert_eq!(
        read(STAGED, "intro.tex").as_deref(),
        Some("Version three, staged.\n")
    );
    assert_eq!(read("v1", "new.tex"), None);
    assert_eq!(
        read("HEAD", "./sub/../intro.tex").as_deref(),
        Some("Version two of the intro.\n")
    );
    let hash = git(&root, &["rev-parse", "HEAD~1"]);
    assert_eq!(
        read(&hash[..10], "intro.tex").as_deref(),
        Some("Version one of the intro.\n")
    );
    assert!(matches!(repo.snapshot("nonesuch"), Err(Error::BadRev(..))));
}

#[test]
fn refs_and_log() {
    let root = fixture("refs");
    let repo = Repo::discover(&root.join("paper")).unwrap();
    let refs = repo.refs().unwrap();
    let names: Vec<(&str, RefKind)> = refs.iter().map(|r| (r.name.as_str(), r.kind)).collect();
    assert_eq!(
        names,
        [
            ("HEAD", RefKind::Head),
            ("draft", RefKind::Branch),
            ("main", RefKind::Branch),
            ("v1", RefKind::Tag)
        ]
    );
    // (the annotated tag's commit, not the tag object)
    assert_eq!(refs[3].id, refs[1].id);
    let log = repo.log(0, 10).unwrap();
    let subjects: Vec<&str> = log.iter().map(|c| c.subject.as_str()).collect();
    assert_eq!(subjects, ["second version", "first version"]);
    assert_eq!(log[0].id, refs[0].id);
    assert_eq!(log[0].parents, [log[1].id]);
    assert_eq!(log[1].parents.len(), 0);
    assert_eq!(log[0].author, "Test");
    assert!(log[0].id.to_string().starts_with(&log[0].short));
    let at = |c: &phitex_git::CommitInfo| -> Vec<String> {
        c.refs.iter().map(|r| r.name.clone()).collect()
    };
    assert_eq!(at(&log[0]), ["HEAD", "main"]);
    assert_eq!(at(&log[1]), ["draft", "v1"]);
    // (paging)
    assert_eq!(repo.log(1, 10).unwrap()[0].id, log[1].id);
    assert_eq!(repo.log(0, 1).unwrap().len(), 1);
}

/// A merge: its parents in order, every branch's commits (all refs are
/// tips), a remote-tracking branch, children before parents.
#[test]
fn merge_graph() {
    let root = fixture("merge");
    let paper = root.join("paper");
    git(&root, &["commit", "-q", "-a", "-m", "staged intro"]);
    git(&root, &["checkout", "-q", "draft"]);
    write(&paper.join("side.tex"), "On the draft branch.\n");
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "draft work"]);
    git(&root, &["checkout", "-q", "main"]);
    git(
        &root,
        &["merge", "-q", "--no-ff", "-m", "merge draft", "draft"],
    );
    git(&root, &["update-ref", "refs/remotes/origin/main", "HEAD~1"]);
    let repo = Repo::discover(&paper).unwrap();
    let log = repo.log(0, 100).unwrap();
    let subjects: Vec<&str> = log.iter().map(|c| c.subject.as_str()).collect();
    assert_eq!(subjects[0], "merge draft");
    assert_eq!(log.len(), 5, "{subjects:?}");
    let pos = |s: &str| subjects.iter().position(|x| *x == s).unwrap();
    // (the merge's parents: main's commit first, then draft's)
    assert_eq!(
        log[0].parents,
        [log[pos("staged intro")].id, log[pos("draft work")].id]
    );
    for (k, c) in log.iter().enumerate() {
        for p in &c.parents {
            let j = log.iter().position(|x| x.id == *p).unwrap();
            assert!(j > k, "a parent before its child: {subjects:?}");
        }
    }
    let staged = &log[pos("staged intro")];
    assert!(
        staged
            .refs
            .iter()
            .any(|r| r.name == "origin/main" && r.kind == RefKind::Remote)
    );
    assert!(
        log[pos("draft work")]
            .refs
            .iter()
            .any(|r| r.name == "draft" && r.kind == RefKind::Branch)
    );
}

#[test]
fn moved() {
    let root = fixture("moved");
    let repo = Repo::discover(&root.join("paper")).unwrap();
    let by_branch = repo.snapshot("main").unwrap();
    let hash = by_branch.commit().unwrap().to_string();
    let by_hash = repo.snapshot(&hash).unwrap();
    assert_eq!(by_branch.moved(&repo).unwrap(), None);
    git(&root, &["commit", "-q", "-m", "third version"]);
    let now = by_branch.moved(&repo).unwrap();
    assert!(now.is_some_and(|id| id != by_branch.commit().unwrap()));
    assert_eq!(by_hash.moved(&repo).unwrap(), None);
    assert_eq!(repo.snapshot(STAGED).unwrap().moved(&repo).unwrap(), None);
}

#[test]
fn not_a_repo() {
    let dir = std::env::temp_dir();
    match Repo::discover(&dir) {
        Err(e @ Error::NotARepo(_)) => assert!(e.to_string().contains("compare two folders")),
        Err(e) => panic!("{e}"),
        Ok(_) => {}
    }
}

/// The old version's own `\input` graph: a file the new version no longer
/// inputs is deleted text, a new one added.
#[test]
fn diff_against_a_commit() {
    let root = fixture("diff");
    let paper = root.join("paper");
    let repo = Repo::discover(&paper).unwrap();
    let old = repo.snapshot("v1").unwrap();
    let d = diff(&old, &Dir(paper), "main.tex", &Options::default()).unwrap();
    assert!(
        d.tex.contains("A file \\DIFdelbegin \\DIFdel{only }"),
        "{}",
        d.tex
    );
    assert!(d.tex.contains("\\DIFadd{second }"), "{}", d.tex);
    // (no blank line where `\input` was)
    assert!(
        d.tex.contains("working tree}\\DIFaddend .\nA file"),
        "{}",
        d.tex
    );
    let files: Vec<(&str, &str, ChangeKind)> = d
        .changes
        .iter()
        .map(|c| (c.old.file.as_str(), c.new.file.as_str(), c.kind))
        .collect();
    assert!(
        files.contains(&("intro.tex", "intro.tex", ChangeKind::Change)),
        "{files:?}"
    );
    assert!(
        files.contains(&("old.tex", "new.tex", ChangeKind::Change)),
        "{files:?}"
    );
}
