//! A panic in an app is written to `~/.niminal/crash.log`, with the app's name and version.

#[test]
fn a_panic_is_recorded_with_the_app_that_had_it() {
    let home = std::env::temp_dir().join(format!("niminal-crash-log-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    // this is the only test in this program, so nothing else reads HOME while it changes
    unsafe { std::env::set_var("HOME", &home) };
    niminal_ws::launch::install_crash_log("niminal-test-app", "9.9.9");
    let _ = std::thread::Builder::new()
        .name("worker".into())
        .spawn(|| panic!("something went wrong"))
        .unwrap()
        .join();
    let log = std::fs::read_to_string(home.join(".niminal/crash.log")).expect("crash.log was written");
    assert!(log.contains("niminal-test-app 9.9.9 panicked in thread `worker`"), "{log}");
    assert!(log.contains("something went wrong"), "{log}");
    let _ = std::fs::remove_dir_all(&home);
}
