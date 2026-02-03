use std::time::Duration;

use super::timer_check_interval;

#[test]
fn timer_check_interval_default() {
    // Ensure no env var is set for this test
    std::env::remove_var("OJ_TIMER_CHECK_MS");
    assert_eq!(timer_check_interval(), Duration::from_secs(1));
}

#[test]
fn timer_check_interval_from_env() {
    std::env::set_var("OJ_TIMER_CHECK_MS", "500");
    assert_eq!(timer_check_interval(), Duration::from_millis(500));
    std::env::remove_var("OJ_TIMER_CHECK_MS");
}

#[test]
fn timer_check_interval_invalid_env_falls_back_to_default() {
    std::env::set_var("OJ_TIMER_CHECK_MS", "not_a_number");
    assert_eq!(timer_check_interval(), Duration::from_secs(1));
    std::env::remove_var("OJ_TIMER_CHECK_MS");
}
