// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use super::*;

// =============================================================================
// escape_for_shell tests
// =============================================================================

#[test]
fn escape_for_shell_no_special_chars() {
    assert_eq!(escape_for_shell("hello world"), "hello world");
}

#[test]
fn escape_for_shell_escapes_backslash() {
    assert_eq!(escape_for_shell(r"path\to\file"), r"path\\to\\file");
}

#[test]
fn escape_for_shell_escapes_dollar_sign() {
    assert_eq!(escape_for_shell("$HOME"), "\\$HOME");
}

#[test]
fn escape_for_shell_escapes_backtick() {
    assert_eq!(
        escape_for_shell("Write to `file.txt`"),
        "Write to \\`file.txt\\`"
    );
}

#[test]
fn escape_for_shell_escapes_double_quote() {
    assert_eq!(escape_for_shell(r#"say "hello""#), r#"say \"hello\""#);
}

#[test]
fn escape_for_shell_escapes_all_special_chars() {
    assert_eq!(
        escape_for_shell(r#"$VAR `cmd` "quote" \slash"#),
        r#"\$VAR \`cmd\` \"quote\" \\slash"#
    );
}

#[test]
fn escape_for_shell_empty_string() {
    assert_eq!(escape_for_shell(""), "");
}

#[test]
fn escape_for_shell_preserves_single_quotes() {
    // Single quotes have no special meaning inside double quotes
    assert_eq!(escape_for_shell("it's a test"), "it's a test");
}

#[test]
fn escape_for_shell_preserves_spaces_and_newlines() {
    assert_eq!(
        escape_for_shell("Normal text with newlines\nand tabs\t"),
        "Normal text with newlines\nand tabs\t"
    );
}

// =============================================================================
// interpolate_shell tests
// =============================================================================

#[test]
fn interpolate_shell_escapes_special_chars_in_double_quotes() {
    let vars: HashMap<String, String> = [(
        "title".to_string(),
        r#"fix: handle "$HOME" path"#.to_string(),
    )]
    .into_iter()
    .collect();
    assert_eq!(
        interpolate_shell(r#"git commit -m "${title}""#, &vars),
        r#"git commit -m "fix: handle \"\$HOME\" path""#
    );
}

#[test]
fn interpolate_shell_escapes_backticks() {
    let vars: HashMap<String, String> =
        [("title".to_string(), "fix: update `config.rs`".to_string())]
            .into_iter()
            .collect();
    assert_eq!(
        interpolate_shell(r#"git commit -m "${title}""#, &vars),
        r#"git commit -m "fix: update \`config.rs\`""#
    );
}

#[test]
fn interpolate_shell_preserves_single_quotes_in_value() {
    let vars: HashMap<String, String> = [("msg".to_string(), "it's a test".to_string())]
        .into_iter()
        .collect();
    // Single quotes in a value are harmless inside double-quoted shell context
    assert_eq!(
        interpolate_shell(r#"echo "${msg}""#, &vars),
        r#"echo "it's a test""#
    );
}

#[test]
fn interpolate_shell_unknown_left_alone() {
    let vars: HashMap<String, String> = HashMap::new();
    assert_eq!(
        interpolate_shell("echo '${unknown}'", &vars),
        "echo '${unknown}'"
    );
}

#[test]
fn interpolate_plain_does_not_escape() {
    let vars: HashMap<String, String> = [("msg".to_string(), r#"$HOME `pwd` "hello""#.to_string())]
        .into_iter()
        .collect();
    // Regular interpolate should NOT escape
    assert_eq!(interpolate("${msg}", &vars), r#"$HOME `pwd` "hello""#);
}

#[test]
fn interpolate_shell_realistic_submit_step() {
    // Simulate a submit step where local.title contains user-provided text
    let vars: HashMap<String, String> = [
        (
            "local.title".to_string(),
            "fix: handle `$PATH` and \"quotes\"".to_string(),
        ),
        ("local.branch".to_string(), "fix/bug-123".to_string()),
    ]
    .into_iter()
    .collect();
    let template = r#"git commit -m "${local.title}" && git push origin "${local.branch}""#;
    let result = interpolate_shell(template, &vars);
    assert_eq!(
        result,
        r#"git commit -m "fix: handle \`\$PATH\` and \"quotes\"" && git push origin "fix/bug-123""#
    );
}

// =============================================================================
// interpolate tests
// =============================================================================

#[test]
fn interpolate_simple() {
    let vars: HashMap<String, String> = [("name".to_string(), "test".to_string())]
        .into_iter()
        .collect();
    assert_eq!(interpolate("Hello ${name}!", &vars), "Hello test!");
}

#[test]
fn interpolate_multiple() {
    let vars: HashMap<String, String> = [
        ("a".to_string(), "1".to_string()),
        ("b".to_string(), "2".to_string()),
    ]
    .into_iter()
    .collect();
    assert_eq!(interpolate("${a} + ${b} = ${a}${b}", &vars), "1 + 2 = 12");
}

#[test]
fn interpolate_unknown_left_alone() {
    let vars: HashMap<String, String> = HashMap::new();
    assert_eq!(interpolate("Hello ${unknown}!", &vars), "Hello ${unknown}!");
}

#[test]
fn interpolate_no_vars() {
    let vars: HashMap<String, String> = HashMap::new();
    assert_eq!(interpolate("No variables here", &vars), "No variables here");
}

#[test]
fn interpolate_empty_braces_not_matched() {
    let vars: HashMap<String, String> = HashMap::new();
    // Empty ${} should not match the template var pattern and pass through unchanged
    assert_eq!(interpolate("${}", &vars), "${}");
    // Incomplete ${ should also pass through unchanged
    assert_eq!(interpolate("${", &vars), "${");
}

#[test]
fn interpolate_env_var_with_default_uses_env() {
    // Set an env var for this test
    std::env::set_var("TEMPLATE_TEST_VAR", "from_env");
    let vars: HashMap<String, String> = HashMap::new();
    assert_eq!(
        interpolate("${TEMPLATE_TEST_VAR:-default}", &vars),
        "from_env"
    );
    std::env::remove_var("TEMPLATE_TEST_VAR");
}

#[test]
fn interpolate_env_var_with_default_uses_default() {
    // Ensure env var is not set
    std::env::remove_var("TEMPLATE_UNSET_VAR");
    let vars: HashMap<String, String> = HashMap::new();
    assert_eq!(
        interpolate("${TEMPLATE_UNSET_VAR:-fallback}", &vars),
        "fallback"
    );
}

#[test]
fn interpolate_env_and_template_vars() {
    std::env::set_var("TEMPLATE_CMD_VAR", "custom_cmd");
    let vars: HashMap<String, String> = [("name".to_string(), "test".to_string())]
        .into_iter()
        .collect();
    assert_eq!(
        interpolate("${TEMPLATE_CMD_VAR:-default} --name ${name}", &vars),
        "custom_cmd --name test"
    );
    std::env::remove_var("TEMPLATE_CMD_VAR");
}

#[test]
fn interpolate_dotted_key() {
    let vars: HashMap<String, String> = [
        ("input.name".to_string(), "my-feature".to_string()),
        ("input.prompt".to_string(), "Add tests".to_string()),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        interpolate("Feature: ${input.name}, Task: ${input.prompt}", &vars),
        "Feature: my-feature, Task: Add tests"
    );
}

#[test]
fn interpolate_dotted_key_with_hyphen() {
    let vars: HashMap<String, String> = [("input.feature-name".to_string(), "auth".to_string())]
        .into_iter()
        .collect();
    assert_eq!(
        interpolate("Testing ${input.feature-name}", &vars),
        "Testing auth"
    );
}

#[test]
fn interpolate_mixed_simple_and_dotted() {
    let vars: HashMap<String, String> = [
        ("prompt".to_string(), "rendered prompt text".to_string()),
        ("input.prompt".to_string(), "user input".to_string()),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        interpolate("Command: ${prompt}, Input: ${input.prompt}", &vars),
        "Command: rendered prompt text, Input: user input"
    );
}
