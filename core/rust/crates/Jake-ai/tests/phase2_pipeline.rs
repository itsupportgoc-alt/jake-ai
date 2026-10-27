//! Phase 2 integration tests: context + intent + reasoning pipeline.
//!
//! Model-dependent tests load `models/Jake-strong` from the workspace root
//! and are skipped (with a message) when the checkpoint is absent so that the
//! test suite stays green before the first training run.

use jarvis_ai::checkpoint::load_checkpoint;
use jarvis_ai::context::{ContextWindow, Role, TRIM_MARKER};
use jarvis_ai::intent::IntentClassifier;
use jarvis_ai::reason::{plan, quoted_slot, CLARIFY_MARGIN};

fn project_root() -> String {
    // tests live in core/rust/crates/Jake-ai → workspace root is 4 levels up
    let manifest = env!("CARGO_MANIFEST_DIR");
    let mut p = std::path::PathBuf::from(manifest);
    for _ in 0..4 {
        p.pop();
    }
    p.to_string_lossy().to_string()
}

#[test]
fn classifier_picks_expected_intents() -> Result<(), String> {
    let dir = format!("{}/models/Jake-strong", project_root());
    let Ok((_tokenizer, model, _meta)) = load_checkpoint(&dir) else {
        println!("SKIP: no checkpoint at {dir} (train first)");
        return Ok(());
    };

    let classifier = IntentClassifier::new();
    let history = ContextWindow::new(1024);

    let cases: &[(&str, &str)] = &[
        ("what time is it", "time.now"),
        ("system info", "sys.info"),
        ("open notepad", "app.open"),
        ("read \"C:/Users/you/Documents/todo.txt\"", "fs.read"),
        ("search for jarvis", "fs.search"),
        ("jarvis stop", "stop"),
    ];

    let mut correct = 0usize;
    for (utterance, expected) in cases {
        let scores = classifier.classify(&_tokenizer, &model, &history, utterance);
        let top = &scores[0];
        println!("{utterance:?} -> {} ({:.4}) want {}", top.label, top.mean_logprob, expected);
        if top.label == *expected {
            correct += 1;
        }
    }
    println!("intent accuracy {correct}/{}", cases.len());
    assert!(
        correct >= cases.len() * 3 / 4,
        "classifier accuracy too low: {correct}/{}",
        cases.len()
    );
    Ok(())
}

/// The deterministic emergency guard must fire even on wording the model was
/// never trained on — the stop path never depends on the small LM.
#[test]
fn emergency_guard_overrides_untrained_phrasing() -> Result<(), String> {
    let dir = format!("{}/models/Jake-strong", project_root());
    let Ok((tokenizer, model, _meta)) = load_checkpoint(&dir) else {
        println!("SKIP: no checkpoint at {dir} (train first)");
        return Ok(());
    };

    let classifier = IntentClassifier::new();
    let history = ContextWindow::new(1024);
    for utterance in ["stop everything now", "halt the task", "cancel my queue"] {
        let scores = classifier.classify(&tokenizer, &model, &history, utterance);
        assert_eq!(scores[0].label, "stop", "guard failed for {utterance:?}");
    }
    Ok(())
}

#[test]
fn emergency_stop_plan_is_detached_from_tools() {
    let p = plan("stop", "stop everything", 1.0);
    assert!(p.emergency);
    assert!(p.steps.iter().all(|s| s.tool == "speak"));
}

#[test]
fn low_margin_flags_clarification() {
    let p = plan("fs.read", "read \"todo.txt\"", 0.01);
    assert!(p.needs_clarification);
    assert!(CLARIFY_MARGIN > 0.0);
}

#[test]
fn slots_parse_files_and_apps() {
    assert_eq!(quoted_slot("read \"notes.txt\""), Some("notes.txt".to_string()));
    assert_eq!(quoted_slot("open notepad"), None);
}

#[test]
fn context_window_trims_oldest_keeps_newest() {
    let mut ctx = ContextWindow::new(200);
    for i in 0..40 {
        ctx.push(Role::User, format!("message number {i} with plenty of text to fill space"));
        ctx.push(Role::Assistant, format!("reply number {i}"));
    }
    let rendered = ctx.render();
    assert!(rendered.starts_with(TRIM_MARKER));
    assert!(rendered.contains("message number 39"), "newest must survive");
    assert!(!rendered.contains("message number 0 "), "oldest must be gone");
}
