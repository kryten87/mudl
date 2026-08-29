//! Black-box process-spawn tests for the `mudl` binary (Phase 8.2 of
//! `docs/IMPLEMENTATION-PLAN.md`) — the "thin integration layer" the plan
//! calls for, exercising the real compiled binary end-to-end rather than
//! `run()` directly (that's covered by `main.rs`'s own unit tests).

use std::io::Write;
use std::process::{Command, Stdio};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mudl-cli")
}

fn write_temp_md(name: &str, contents: &str) -> std::path::PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("mudl-cli-test-{}-{name}", std::process::id()));
    std::fs::write(&path, contents).unwrap();
    path
}

#[test]
fn renders_a_file_with_html_up() {
    let path = write_temp_md("up.md", "# Hello\n");
    let output = Command::new(bin()).arg("-u").arg(&path).output().unwrap();
    std::fs::remove_file(&path).ok();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("<h1"));
    assert!(stdout.contains("Hello"));
}

#[test]
fn renders_stdin_when_no_files_given() {
    let mut child = Command::new(bin())
        .arg("-d")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"raw line\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("raw line"));
}

#[test]
fn missing_file_exits_with_code_two() {
    let output = Command::new(bin())
        .arg("-u")
        .arg("/no/such/file.md")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn unknown_flag_exits_with_code_one() {
    let output = Command::new(bin()).arg("--nonsense").output().unwrap();

    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn help_exits_zero() {
    let output = Command::new(bin()).arg("--help").output().unwrap();

    assert!(output.status.success());
}

#[test]
fn multiple_files_are_each_rendered() {
    let path_a = write_temp_md("multi-a.md", "# A\n");
    let path_b = write_temp_md("multi-b.md", "# B\n");
    let output = Command::new(bin())
        .arg("-u")
        .arg(&path_a)
        .arg(&path_b)
        .output()
        .unwrap();
    std::fs::remove_file(&path_a).ok();
    std::fs::remove_file(&path_b).ok();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains(">A<"));
    assert!(stdout.contains(">B<"));
}

#[test]
fn standalone_inlines_local_image_as_data_uri() {
    let mut img_path = std::env::temp_dir();
    img_path.push(format!("mudl-cli-test-{}-pixel.png", std::process::id()));
    // A minimal 1x1 transparent PNG.
    let png_bytes: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];
    std::fs::write(&img_path, png_bytes).unwrap();
    let img_name = img_path.file_name().unwrap().to_str().unwrap();

    let md_path = write_temp_md("standalone.md", &format!("![alt]({img_name})\n"));
    let output = Command::new(bin())
        .arg("-u")
        .arg("--standalone")
        .arg(&md_path)
        .output()
        .unwrap();
    std::fs::remove_file(&md_path).ok();
    std::fs::remove_file(&img_path).ok();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("data:image/png;base64,"));
}
