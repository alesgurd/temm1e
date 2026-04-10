//! Tem-Code v5.0 A/B Benchmark — Integration Test Entry Point
//!
//! Compares OLD toolset vs NEW toolset across token usage, efficiency, and safety.
//! Run with: cargo test --test tem_code_ab_test -- --nocapture

mod tem_code_ab;

use tem_code_ab::benchmark::*;
use tem_code_ab::metrics::*;
use tem_code_ab::scenarios::*;

#[tokio::test]
async fn ab_benchmark_impossible_refactor() {
    let tmp = tempfile::tempdir().unwrap();
    let tasks = create_impossible_refactor(tmp.path()).await;

    let old = simulate_old_toolset(tmp.path(), &tasks).await;
    let new = simulate_new_toolset(tmp.path(), &tasks).await;

    let report = compare_results(&old, &new);

    println!("\n{}", report);

    // ── Assertions ──────────────────────────────────────────────

    // 1. Token savings: NEW uses fewer tokens
    assert!(
        report.token_savings_pct > 0.0,
        "NEW toolset should save tokens (got {:.1}%)",
        report.token_savings_pct
    );

    // 2. Efficiency: NEW completes more tasks per token
    assert!(
        report.efficiency_delta > 0.0,
        "NEW should be more token-efficient (delta: {:.4})",
        report.efficiency_delta
    );

    // 3. Safety: NEW has fewer violations
    assert!(
        report.new_violations < report.old_violations,
        "NEW ({}) should have fewer violations than OLD ({})",
        report.new_violations,
        report.old_violations
    );

    // 4. Accuracy: NEW matches or exceeds OLD
    assert!(
        report.accuracy_delta >= 0.0,
        "NEW accuracy should be >= OLD (delta: {:.1}pp)",
        report.accuracy_delta
    );

    // 5. NEW should complete all tasks
    assert_eq!(
        new.tasks_completed, new.tasks_total,
        "NEW should complete all {} tasks (got {})",
        new.tasks_total, new.tasks_completed
    );

    // 6. OLD should fail at least the safety tasks
    assert!(
        old.tasks_completed < old.tasks_total,
        "OLD should fail some tasks (completed {}/{})",
        old.tasks_completed,
        old.tasks_total
    );
}
