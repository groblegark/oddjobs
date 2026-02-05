// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use super::super::*;

// =============================================================================
// PrimeDef Tests
// =============================================================================

#[test]
fn prime_deserialize_string_form() {
    #[derive(Debug, Deserialize)]
    struct TestConfig {
        prime: PrimeDef,
    }

    let toml = r#"
        prime = "echo hello\ngit status"
    "#;
    let config: TestConfig = toml::from_str(toml).unwrap();
    assert!(matches!(config.prime, PrimeDef::Script(_)));
    if let PrimeDef::Script(s) = &config.prime {
        assert!(s.contains("echo hello"));
    }
}

#[test]
fn prime_deserialize_array_form() {
    #[derive(Debug, Deserialize)]
    struct TestConfig {
        prime: PrimeDef,
    }

    let toml = r#"
        prime = ["echo hello", "git status"]
    "#;
    let config: TestConfig = toml::from_str(toml).unwrap();
    assert!(matches!(config.prime, PrimeDef::Commands(_)));
    if let PrimeDef::Commands(cmds) = &config.prime {
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0], "echo hello");
        assert_eq!(cmds[1], "git status");
    }
}

#[test]
fn prime_render_script_interpolates() {
    let prime = PrimeDef::Script("echo ${name} in ${workspace}".to_string());
    let vars: HashMap<String, String> = [
        ("name".to_string(), "test-feature".to_string()),
        ("workspace".to_string(), "/tmp/ws".to_string()),
    ]
    .into_iter()
    .collect();

    let result = prime.render(&vars);
    assert_eq!(result, "echo test-feature in /tmp/ws");
}

#[test]
fn prime_render_commands_interpolates() {
    let prime = PrimeDef::Commands(vec![
        "echo ${name}".to_string(),
        "git -C ${workspace} status".to_string(),
    ]);
    let vars: HashMap<String, String> = [
        ("name".to_string(), "test-feature".to_string()),
        ("workspace".to_string(), "/tmp/ws".to_string()),
    ]
    .into_iter()
    .collect();

    let result = prime.render(&vars);
    assert_eq!(result, "echo test-feature\ngit -C /tmp/ws status");
}

#[test]
fn prime_optional_defaults_to_none() {
    let toml = r#"
        name = "worker"
        run = "claude"
    "#;
    let agent: AgentDef = toml::from_str(toml).unwrap();
    assert!(agent.prime.is_none());
}

// =============================================================================
// PrimeDef PerSource Tests
// =============================================================================

#[test]
fn prime_deserialize_per_source_form() {
    let json = r#"{
        "startup": ["echo startup"],
        "resume": "echo resume"
    }"#;
    let prime: PrimeDef = serde_json::from_str(json).unwrap();
    match &prime {
        PrimeDef::PerSource(map) => {
            assert_eq!(map.len(), 2);
            assert!(matches!(map.get("startup"), Some(PrimeDef::Commands(_))));
            assert!(matches!(map.get("resume"), Some(PrimeDef::Script(_))));
        }
        other => panic!("expected PerSource, got {:?}", other),
    }
}

#[test]
fn prime_render_per_source() {
    let mut map = HashMap::new();
    map.insert(
        "startup".to_string(),
        PrimeDef::Commands(vec!["echo ${name}".to_string(), "git status".to_string()]),
    );
    map.insert(
        "resume".to_string(),
        PrimeDef::Script("echo resuming ${name}".to_string()),
    );
    let prime = PrimeDef::PerSource(map);

    let vars: HashMap<String, String> = [("name".to_string(), "test".to_string())]
        .into_iter()
        .collect();
    let rendered = prime.render_per_source(&vars);

    assert_eq!(rendered.len(), 2);
    assert_eq!(rendered["startup"], "echo test\ngit status");
    assert_eq!(rendered["resume"], "echo resuming test");
}

#[test]
fn prime_render_script_as_per_source() {
    let prime = PrimeDef::Script("echo hello".to_string());
    let vars = HashMap::new();
    let rendered = prime.render_per_source(&vars);

    assert_eq!(rendered.len(), 1);
    assert_eq!(rendered[""], "echo hello");
}

#[test]
fn prime_render_commands_as_per_source() {
    let prime = PrimeDef::Commands(vec!["echo a".to_string(), "echo b".to_string()]);
    let vars = HashMap::new();
    let rendered = prime.render_per_source(&vars);

    assert_eq!(rendered.len(), 1);
    assert_eq!(rendered[""], "echo a\necho b");
}

#[test]
fn prime_per_source_rejects_nested() {
    let json = r#"{
        "startup": { "resume": "echo nested" }
    }"#;
    let result: Result<PrimeDef, _> = serde_json::from_str(json);
    assert!(
        result.is_err(),
        "nested PerSource should fail deserialization"
    );
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("nested per-source prime"),
        "error should mention nesting: {}",
        err
    );
}

#[test]
#[should_panic(expected = "render() not valid for PerSource")]
fn prime_per_source_render_panics() {
    let mut map = HashMap::new();
    map.insert(
        "startup".to_string(),
        PrimeDef::Script("echo hi".to_string()),
    );
    let prime = PrimeDef::PerSource(map);
    let vars = HashMap::new();
    prime.render(&vars);
}
