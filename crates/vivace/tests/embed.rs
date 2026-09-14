//! L'entrée bibliothèque : un programme hôte appelle `vivace::run` avec
//! une ligne de commande et obtient le code retour du binaire.

#[test]
fn run_returns_the_binary_exit_codes() {
    assert_eq!(vivace::run(["vivace", "--version"]), 0);
    assert_eq!(vivace::run(["vivace", "--help"]), 0);
    assert_eq!(vivace::run(["vivace", "bogus"]), 2);
    assert_eq!(vivace::run(["vivace", "install", "--no-such-flag"]), 2);
    let missing = tempfile::tempdir().expect("tmp");
    let dir = missing.path().join("absent");
    assert_eq!(
        vivace::run([
            "vivace",
            "install",
            "--no-fallback",
            "--working-dir",
            dir.to_str().expect("utf8")
        ]),
        1
    );
}
