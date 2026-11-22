//! phitex-git against a throwaway repository made here, under `target/`
//! (with the `git` command, for the fixture only): commits, a branch, an
//! annotated tag, staged and unstaged changes, a project in a
//! subdirectory.

use phitex_diff::{ChangeKind, Dir, Files, Options, diff};
use phitex_git::{BlameOpts, Error, RefKind, Repo, STAGED};
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
        .join("../../../../target/phitex-git-fixture")
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
        d.tex.contains("A file \\DIFdelbegin \\DIFdel{only} "),
        "{}",
        d.tex
    );
    assert!(d.tex.contains("\\DIFadd{second} "), "{}", d.tex);
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

/// Blame: each line with the commit that last changed it, by its author
/// (name, email, subject); a line only the working tree has is not
/// committed; a merge's lines from its side branch keep the side's commit;
/// a figure's image blamed whole.
#[test]
fn blame() {
    let root = fixture("blame");
    let paper = root.join("paper");
    let commit = |who: &str, msg: &str| {
        git(&root, &["add", "-A"]);
        git(
            &root,
            &[
                "-c",
                &format!("user.name={who}"),
                "-c",
                &format!("user.email={}@example.com", who.to_lowercase()),
                "commit",
                "-q",
                "-m",
                msg,
            ],
        );
    };
    write(&paper.join("body.tex"), "One.\nTwo.\nThree.\n");
    write(&paper.join("fig.png"), "\u{89}PNG first");
    commit("Alice", "Body: one, two, three");
    write(
        &paper.join("body.tex"),
        "One.\nTwo, rewritten.\nThree.\nFour.\n",
    );
    commit("Bob", "Rewrite two, add four");
    git(&root, &["checkout", "-q", "-b", "side", "HEAD"]);
    write(
        &paper.join("body.tex"),
        "Zero.\nOne.\nTwo, rewritten.\nThree.\nFour.\n",
    );
    commit("Carol", "Add zero on a branch");
    git(&root, &["checkout", "-q", "main"]);
    write(&paper.join("fig.png"), "\u{89}PNG second");
    commit("Dana", "A new figure");
    git(
        &root,
        &["merge", "-q", "--no-ff", "-m", "merge side", "side"],
    );
    let repo = Repo::discover(&paper).unwrap();
    let now = b"Zero.\nOne.\nTwo, rewritten.\nThree, not committed.\nFour.\n";
    let b = repo.blame("body.tex", Some(now)).unwrap();
    let who: Vec<Option<&str>> = b
        .lines
        .iter()
        .map(|l| l.map(|k| b.commits[k].author.as_str()))
        .collect();
    assert_eq!(
        who,
        [Some("Carol"), Some("Alice"), Some("Bob"), None, Some("Bob")]
    );
    let bob = &b.commits[b.lines[2].unwrap()];
    assert_eq!(bob.subject, "Rewrite two, add four");
    assert_eq!(bob.email, "bob@example.com");
    assert!(bob.id.to_string().starts_with(&bob.short));
    // (as HEAD has it: every line committed)
    let at_head = repo.blame("body.tex", None).unwrap();
    assert_eq!(at_head.lines.len(), 5);
    assert!(at_head.lines.iter().all(Option::is_some));
    // (a file HEAD does not have: nothing committed)
    let new = repo.blame("nonesuch.tex", Some(b"a\nb\n")).unwrap();
    assert_eq!(new.lines, [None, None]);
    // (a figure: its last change; changed on disk, not committed)
    let fig = repo.last_change("fig.png", None).unwrap().unwrap();
    assert_eq!(fig.author, "Dana");
    assert_eq!(
        repo.last_change("fig.png", Some("\u{89}PNG second".as_bytes()))
            .unwrap()
            .unwrap()
            .subject,
        "A new figure"
    );
    assert_eq!(repo.last_change("fig.png", Some(b"other")).unwrap(), None);
    // (Dana's commit skipped: the figure as it was before, Alice's)
    let skipped = repo
        .last_change_skipping("fig.png", None, &[fig.id])
        .unwrap()
        .unwrap();
    assert_eq!(skipped.author, "Alice");
}

/// The repository `blame_by_words_moves_and_skips` reads: Alice writes two
/// lines, Bob changes a word, Carol only spacing, Dana moves a line up.
fn blame_words_paper() -> PathBuf {
    let root = fixture("blame-words");
    let paper = root.join("paper");
    let commit = |who: &str, msg: &str| {
        git(&root, &["add", "-A"]);
        git(
            &root,
            &[
                "-c",
                &format!("user.name={who}"),
                "-c",
                &format!("user.email={}@example.com", who.to_lowercase()),
                "commit",
                "-q",
                "-m",
                msg,
            ],
        );
    };
    write(
        &paper.join("w.tex"),
        "The quick brown fox jumps over the lazy dog.\nA second line, which Alice wrote too.\n",
    );
    commit("Alice", "Two lines");
    write(
        &paper.join("w.tex"),
        "The slow brown fox jumps over the lazy dog.\nA second line, which Alice wrote too.\n",
    );
    commit("Bob", "quick to slow");
    write(
        &paper.join("w.tex"),
        "The  slow brown fox jumps over the  lazy dog.\nA second line, which Alice wrote too.\n",
    );
    commit("Carol", "Spacing");
    write(
        &paper.join("w.tex"),
        "A second line, which Alice wrote too.\nThe  slow brown fox jumps over the  lazy dog.\n",
    );
    commit("Dana", "Move the second line up");
    paper
}

/// Blame by words, spacing ignored, moved text and skipped commits (the
/// viewer's Blame settings): a word Bob changed in Alice's line is Bob's,
/// the rest Alice's; Carol's spacing-only edit changes no one's words;
/// Dana's moved line keeps Alice as its writer (or is Dana's, the mover
/// credited); Bob's commit skipped, his word goes back to Alice.
#[test]
fn blame_by_words_moves_and_skips() {
    let paper = blame_words_paper();
    let repo = Repo::discover(&paper).unwrap();
    let text = std::fs::read(paper.join("w.tex")).unwrap();
    let who = |b: &phitex_git::Blame, word: &str| -> Option<String> {
        let at = text
            .windows(word.len())
            .position(|w| w == word.as_bytes())
            .unwrap();
        b.runs
            .iter()
            .find(|r| r.0 <= at && at < r.1)
            .and_then(|r| r.2)
            .map(|k| b.commits[k].author.clone())
    };
    let words = BlameOpts {
        words: true,
        ignore_space: true,
        moved: true,
        skip: vec![],
    };
    let b = repo.blame_with("w.tex", Some(&text), &words).unwrap();
    assert_eq!(who(&b, "slow").as_deref(), Some("Bob"));
    assert_eq!(who(&b, "brown").as_deref(), Some("Alice"));
    assert_eq!(who(&b, "lazy").as_deref(), Some("Alice"));
    assert_eq!(
        who(&b, "second").as_deref(),
        Some("Alice"),
        "moved: its writer kept"
    );
    // (the runs cover the text, in order)
    assert_eq!(b.runs.first().map(|r| r.0), Some(0));
    assert_eq!(b.runs.last().map(|r| r.1), Some(text.len()));
    assert!(b.runs.windows(2).all(|w| w[0].1 == w[1].0));
    // (spacing counted: Carol's spaces are hers)
    let counted = repo
        .blame_with(
            "w.tex",
            Some(&text),
            &BlameOpts {
                ignore_space: false,
                ..words.clone()
            },
        )
        .unwrap();
    let at = text.windows(2).position(|w| w == b"  ").unwrap();
    let k = counted
        .runs
        .iter()
        .find(|r| r.0 <= at && at < r.1)
        .and_then(|r| r.2)
        .unwrap();
    assert_eq!(counted.commits[k].author, "Carol");
    // (by lines: moved, each keeps its writer; the mover credited, the line the diff moved is Dana's)
    let lines = |o: &BlameOpts| -> Vec<String> {
        let b = repo.blame_with("w.tex", Some(&text), o).unwrap();
        b.lines
            .iter()
            .map(|l| b.commits[l.unwrap()].author.clone())
            .collect()
    };
    let by_line = BlameOpts {
        words: false,
        ignore_space: true,
        moved: true,
        skip: vec![],
    };
    assert_eq!(lines(&by_line), ["Alice", "Bob"]);
    // (of two lines swapped, the diff keeps one in place: the other is moved, the mover's)
    assert_eq!(
        lines(&BlameOpts {
            moved: false,
            ..by_line.clone()
        })
        .iter()
        .filter(|w| *w == "Dana")
        .count(),
        1
    );
    // (Bob's commit skipped: his word is Alice's again)
    let bob = b.commits.iter().find(|c| c.author == "Bob").unwrap().id;
    let skipped = repo
        .blame_with(
            "w.tex",
            Some(&text),
            &BlameOpts {
                skip: vec![bob],
                ..words.clone()
            },
        )
        .unwrap();
    assert_eq!(who(&skipped, "slow").as_deref(), Some("Alice"));
}
