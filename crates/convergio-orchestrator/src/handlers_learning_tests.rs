use super::metrics::PlanMetrics;
use super::*;

#[test]
fn generate_learnings_all_done() {
    let m = PlanMetrics {
        tasks_total: 5,
        tasks_done: 5,
        tasks_failed: 0,
        tasks_cancelled: 0,
        waves_total: 2,
        cost_usd: 1.50,
        total_tokens: 50000,
        duration_minutes: Some(30.0),
        agents_used: 3,
        respawn_count: 0,
        tasks_without_evidence: 0,
        pre_review_verdict: Some("pass".into()),
        post_review_verdict: Some("pass".into()),
    };
    let result = generate_learnings(&m);
    let insights = result["insights"].as_array().unwrap();
    assert!(insights.iter().any(|i| i.as_str().unwrap().contains("5/5")));
    assert!(insights
        .iter()
        .any(|i| i.as_str().unwrap().contains("$1.50")));
    assert!(!insights
        .iter()
        .any(|i| i.as_str().unwrap().contains("failed")));
}

#[test]
fn generate_learnings_with_failures() {
    let m = PlanMetrics {
        tasks_total: 10,
        tasks_done: 7,
        tasks_failed: 2,
        tasks_cancelled: 1,
        waves_total: 3,
        cost_usd: 5.0,
        total_tokens: 100000,
        duration_minutes: Some(120.0),
        agents_used: 5,
        respawn_count: 3,
        tasks_without_evidence: 1,
        pre_review_verdict: Some("pass".into()),
        post_review_verdict: Some("fail".into()),
    };
    let result = generate_learnings(&m);
    let insights = result["insights"].as_array().unwrap();
    assert!(insights
        .iter()
        .any(|i| i.as_str().unwrap().contains("failed")));
    assert!(insights
        .iter()
        .any(|i| i.as_str().unwrap().contains("respawn")));
    assert!(insights
        .iter()
        .any(|i| i.as_str().unwrap().contains("evidence")));
}

#[test]
fn parse_review_verdicts_pre_review() {
    let json = Some(r#"{"type":"pre_review","verdict":"pass"}"#.to_string());
    let (pre, post) = parse_review_verdicts(&json);
    assert_eq!(pre, Some("pass".into()));
    assert_eq!(post, None);
}

#[test]
fn parse_review_verdicts_none() {
    let (pre, post) = parse_review_verdicts(&None);
    assert_eq!(pre, None);
    assert_eq!(post, None);
}
