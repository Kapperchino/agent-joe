use super::*;

fn assertions() -> String {
    let output = std::process::Command::new("/usr/bin/pmset")
        .args(["-g", "assertions"])
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn idle_sleep_assertions_are_idempotent_independent_and_released_on_drop() {
    let first_name = format!("Joe sleep test {}", uuid::Uuid::new_v4());
    let second_name = format!("Joe sleep test {}", uuid::Uuid::new_v4());
    let mut first = SleepInhibitor::new(first_name.clone());
    let mut second = SleepInhibitor::new(second_name.clone());
    assert!(!assertions().contains(&first_name));

    first.update(IdleSleep::Prevented).unwrap();
    first.update(IdleSleep::Prevented).unwrap();
    second.update(IdleSleep::Prevented).unwrap();
    let active = assertions();
    assert_eq!(active.matches(&first_name).count(), 1);
    assert_eq!(active.matches(&second_name).count(), 1);
    assert!(
        active.lines().any(|line| {
            line.contains(&first_name) && line.contains("PreventUserIdleSystemSleep")
        })
    );

    first.update(IdleSleep::Allowed).unwrap();
    let active = assertions();
    assert!(!active.contains(&first_name));
    assert!(active.contains(&second_name));
    first.update(IdleSleep::Prevented).unwrap();
    assert!(assertions().contains(&first_name));

    drop(first);
    drop(second);
    let inactive = assertions();
    assert!(!inactive.contains(&first_name));
    assert!(!inactive.contains(&second_name));
}
