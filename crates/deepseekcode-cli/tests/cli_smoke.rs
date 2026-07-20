#[test]
fn cli_help_exits_successfully() {
    use std::process::Command;
    let status = Command::new(env!("CARGO_BIN_EXE_deepseekcode"))
        .arg("--help")
        .status()
        .expect("spawn cli");
    assert!(status.success());
}
