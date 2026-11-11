//! The real binary refuses to start while another process holds the config's lock file, and
//! the one-shot `--print-config` does not need the lock. The lock is held by this test
//! process, so nothing here ever sends a packet.

use std::fs::OpenOptions;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_rtm-sender");

/// Run the sender with a hard timeout (it is killed and the test fails if it hangs).
fn run(args: &[&str], config: &Path) -> Output {
    let mut child = Command::new(BIN)
        .arg("--config")
        .arg(config)
        .args(["--no-gui", "--no-broadcast"])
        .args(args)
        .env("RTM_LOG", "info")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawning rtm-sender");
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("rtm-sender did not exit (lock not honoured?)");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn second_instance_exits_with_code_3() {
    let tmp = tempfile::tempdir().unwrap();
    let config = tmp.path().join("sender.toml");
    // Sends nowhere even if the guard were broken.
    std::fs::write(
        &config,
        "[general]\nid = \"lock-test\"\n[network]\nudp_broadcast = false\n",
    )
    .unwrap();

    let lock = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(tmp.path().join("sender.toml.lock"))
        .unwrap();
    lock.try_lock().expect("test takes the lock first");

    let out = run(&[], &config);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "stderr: {stderr}");
    assert!(stderr.contains("already running"), "stderr: {stderr}");

    // One-shot modes do not take the lock.
    let out = run(&["--print-config"], &config);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("lock-test"));
    drop(lock);
}
