use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn gain_subcommand_exits_zero() {
    let mut cmd = Command::cargo_bin("panda").unwrap();
    cmd.args(["gain"]).assert().success();
}

#[test]
fn gain_shows_summary_text() {
    let mut cmd = Command::cargo_bin("panda").unwrap();
    cmd.args(["gain"])
        .assert()
        .success()
        .stdout(predicate::str::contains("PandaFilter Token Savings"));
}

#[test]
fn gain_no_history_shows_zero() {
    let mut cmd = Command::cargo_bin("panda").unwrap();
    cmd.args(["gain"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Tokens saved:").or(predicate::str::contains("Runs:")));
}

#[test]
fn gain_history_flag_works() {
    let mut cmd = Command::cargo_bin("panda").unwrap();
    cmd.args(["gain", "--history"])
        .assert()
        .success()
        .stdout(predicate::str::contains("PandaFilter Daily History"));
}

#[test]
fn gain_history_days_flag() {
    let mut cmd = Command::cargo_bin("panda").unwrap();
    cmd.args(["gain", "--history", "--days", "7"])
        .assert()
        .success()
        .stdout(predicate::str::contains("last 7 days"));
}
