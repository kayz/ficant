use std::env;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=FICANT_CODE_COMMIT_SHA");
    println!("cargo:rerun-if-env-changed=FICANT_CODE_TREE_SHA");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/index");

    let commit = source_identity("FICANT_CODE_COMMIT_SHA", &["rev-parse", "HEAD"]);
    let tree = source_identity("FICANT_CODE_TREE_SHA", &["rev-parse", "HEAD^{tree}"]);
    require_git_sha(&commit, "commit");
    require_git_sha(&tree, "tree");
    println!("cargo:rustc-env=FICANT_COMPILED_GIT_COMMIT_SHA={commit}");
    println!("cargo:rustc-env=FICANT_COMPILED_GIT_TREE_SHA={tree}");
}

fn source_identity(setting: &str, git_args: &[&str]) -> String {
    env::var(setting).unwrap_or_else(|_| {
        let output = Command::new("git")
            .args(git_args)
            .output()
            .unwrap_or_else(|error| panic!("failed to execute git for {setting}: {error}"));
        assert!(
            output.status.success(),
            "git failed while resolving {setting}"
        );
        String::from_utf8(output.stdout)
            .expect("git identity must be UTF-8")
            .trim()
            .to_owned()
    })
}

fn require_git_sha(value: &str, label: &str) {
    assert!(
        value.len() == 40
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "compiled Git {label} must be one 40-character lowercase SHA"
    );
}
