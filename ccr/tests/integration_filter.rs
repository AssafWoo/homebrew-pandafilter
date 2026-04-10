use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn filter_subcommand_reads_stdin_writes_stdout() {
    let mut cmd = Command::cargo_bin("panda").unwrap();
    cmd.args(["filter"])
        .write_stdin("hello world\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("hello world"));
}

#[test]
fn filter_with_command_flag() {
    let mut cmd = Command::cargo_bin("panda").unwrap();
    cmd.args(["filter", "--command", "cargo"])
        .write_stdin("   Compiling foo v1.0\n   Compiling bar v1.0\nerror[E0001]: bad\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("error[E0001]"));
}

#[test]
fn filter_empty_input() {
    let mut cmd = Command::cargo_bin("panda").unwrap();
    cmd.args(["filter"])
        .write_stdin("")
        .assert()
        .success();
}

#[test]
fn filter_ansi_stripped_from_real_git_output() {
    let input = "\x1b[32m+added line\x1b[0m\n\x1b[31m-removed line\x1b[0m\n";
    let mut cmd = Command::cargo_bin("panda").unwrap();
    cmd.args(["filter", "--command", "git"])
        .write_stdin(input)
        .assert()
        .success()
        .stdout(predicate::str::contains("+added line"))
        .stdout(predicate::str::contains("-removed line"))
        .stdout(predicate::str::contains("\x1b").not());
}
