use super::*;

#[test]
fn parse_minimal_spec() {
    let yaml = r#"
waves:
  - id: W1
    name: Test Wave
    tasks:
      - id: W1-T1
        title: Do something
        effort: 2
        model: claude-sonnet-4-6
        verify:
          - "echo ok"
"#;
    let spec = parse_spec(yaml).unwrap();
    assert_eq!(spec.waves.len(), 1);
    assert_eq!(spec.waves[0].tasks.len(), 1);
    assert_eq!(spec.waves[0].tasks[0].effort, Some(2));
}

#[test]
fn parse_spec_with_do_field() {
    let yaml = r#"
waves:
  - id: W0
    tasks:
      - id: W0-T1
        do: "Run the analysis"
        effort: 1
"#;
    let spec = parse_spec(yaml).unwrap();
    let task = &spec.waves[0].tasks[0];
    assert_eq!(task.title.as_deref(), Some("Run the analysis"));
}
