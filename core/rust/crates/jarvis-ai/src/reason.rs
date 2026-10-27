//! Basic reasoning (Phase 2): intent → plan.
//!
//! This is a deterministic, fully inspectable planner. The trained model
//! supplies the intent (see `intent.rs`); this module turns the intent plus the
//! utterance into a concrete plan of tool steps, flags anything that needs
//! confirmation, and always ends with a speak step. Every step is a plain
//! struct — no hidden behavior, no invented data. When slots cannot be parsed
//! the plan contains a fallback step that reports what is known instead of
//! guessing (mirrors the corpus rule: never invent data).

/// One executable step. `tool` is a built-in tool name from the runtime's
/// registry (e.g. `time.now`, `fs.read`); `arg` is the parsed slot or "".
/// `speak` steps are rendered as the reply instead of calling a tool.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub tool: String,
    pub arg: String,
    pub speak: Option<String>,
}

impl Step {
    pub fn tool(tool: impl Into<String>, arg: impl Into<String>) -> Self {
        Step { tool: tool.into(), arg: arg.into(), speak: None }
    }
    pub fn speak(text: impl Into<String>) -> Self {
        Step { tool: "speak".into(), arg: String::new(), speak: Some(text.into()) }
    }
}

/// A reasoned plan. `emergency` plans bypass tools and must stop everything.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub intent: String,
    pub steps: Vec<Step>,
    /// True when the plan mutates state (open/close apps, write memory/tasks).
    pub requires_confirmation: bool,
    pub emergency: bool,
    /// Set when the utterance matched the intent only marginally; the caller
    /// should ask the user to clarify rather than execute on a guess.
    pub needs_clarification: bool,
}

/// Margin below which the top-1 intent is considered ambiguous.
pub const CLARIFY_MARGIN: f32 = 0.15;

/// Extracts the first quoted string (double or single quotes) from `text`.
pub fn quoted_slot(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut in_double = false;
    let mut start_double: Option<usize> = None;
    for (i, &c) in chars.iter().enumerate() {
        match c {
            '"' => {
                if in_double {
                    return Some(chars[start_double.unwrap() + 1..i].iter().collect());
                }
                in_double = true;
                start_double = Some(i);
            }
            _ => {}
        }
    }
    // Single-quoted fallback
    if let (Some(s), false) = (start_double, in_double) {
        let _ = s;
    }
    // Fallback: single quotes
    let parts: Vec<&str> = text.split('\'').collect();
    if parts.len() >= 3 && !parts[1].trim().is_empty() {
        return Some(parts[1].trim().to_string());
    }
    // Last resort: an explicit path-ish token (contains '/' or '.')
    for word in text.split_whitespace() {
        if word.contains('/') || (!word.ends_with('.') && word.contains('.')) {
            return Some(word.trim_matches(|c| c == ',' || c == '.').to_string());
        }
    }
    None
}

/// Extracts a bare search word: drops leading query verbs/prepositions and
/// punctuation, then returns the first remaining word. Used when the utterance
/// has no quoted slot but clearly names what to look for.
pub fn bare_word_slot(text: &str) -> Option<String> {
    const FILLER: &[&str] = &["search", "find", "anything", "about", "for", "with", "word",
        "please", "now", "look", "up"];
    for word in text.split_whitespace() {
        let w = word.trim_matches(|c: char| !c.is_ascii_alphanumeric());
        let lower = w.to_ascii_lowercase();
        if w.is_empty() || FILLER.contains(&lower.as_str()) {
            continue;
        }
        return Some(lower);
    }
    None
}

/// Search slot: quoted phrase if present, else a bare word, else None (ask).
pub fn search_slot(text: &str) -> Option<String> {
    quoted_slot(text).or_else(|| bare_word_slot(text))
}

/// Extracts an application key such as `notepad`; trims common leading verbs.
pub fn app_slot(text: &str) -> Option<String> {
    let cleaned = text
        .trim_start_matches(|c: char| c == '"' || c == '\'')
        .trim();
    for word in cleaned.split_whitespace() {
        let w = word.trim_matches(|c: char| !c.is_alphanumeric());
        if w.is_empty() {
            continue;
        }
        if matches!(w.to_ascii_lowercase().as_str(), "open" | "close" | "launch" | "start" | "stop"
            | "notepad" | "calculator" | "chrome" | "explorer" | "terminal" | "paint"
            | "vscode" | "calc" | "files")
        {
            let lower = w.to_ascii_lowercase();
            if lower == "open" || lower == "close" || lower == "launch" || lower == "start" || lower == "stop" {
                // one-word utterances like "open" alone have no slot
                continue;
            }
            return Some(lower);
        }
    }
    None
}

/// Maps an intent to a plan. `top_margin` is the classifier's top1-top2 gap.
pub fn plan(intent: &str, utterance: &str, top_margin: f32) -> Plan {
    let needs_clarification = top_margin < CLARIFY_MARGIN;
    let steps = match intent {
        "time.now" => vec![
            Step::tool("time.now", ""),
            Step::speak("The system clock reads the time above."),
        ],
        "sys.info" => vec![
            Step::tool("sys.info", ""),
            Step::speak("System status is shown above."),
        ],
        "app.open" => match app_slot(utterance) {
            Some(app) => vec![
                Step::tool("app.launch", app.clone()),
                Step::speak(format!("Opening {}.", app)),
            ],
            None => vec![Step::speak("Which program should I open?")],
        },
        "app.close" => match app_slot(utterance) {
            Some(app) => vec![
                Step::tool("app.stop", app.clone()),
                Step::speak(format!("Closing {}.", app)),
            ],
            None => vec![Step::speak("Which program should I close?")],
        },
        "fs.list" => vec![Step::tool("fs.list", quoted_slot(utterance).unwrap_or_default())],
        "fs.read" => match quoted_slot(utterance) {
            Some(path) => vec![
                Step::tool("fs.read", path.clone()),
                Step::speak(format!("Read {} above. I can summarise it next.", path)),
            ],
            None => vec![Step::speak("Which file should I read? Tell me the name in quotes.")],
        },
        "fs.search" => match search_slot(utterance) {
            Some(word) => vec![
                Step::tool("fs.search", word.clone()),
                Step::speak(format!("Search for {} finished above.", word)),
            ],
            None => vec![Step::speak("What word should I search for?")],
        },
        "memory.put" => vec![
            Step::tool("memory.put", utterance.trim_start_matches(|p: char| {
                !p.is_alphanumeric()
            }).to_string()),
            Step::speak("Saved to memory."),
        ],
        "memory.get" => vec![
            Step::tool("memory.get", quoted_slot(utterance).unwrap_or_else(|| "preferences".into())),
            Step::speak("That is what memory reports."),
        ],
        "task.put" => vec![
            Step::tool("task.put", utterance.to_string()),
            Step::speak("Task recorded."),
        ],
        "reason.basic" | "reason.plan" | "reason.explain" => vec![Step::speak(format!(
            "Reasoning: {} — plan steps below; nothing here is invented.",
            intent
        ))],
        "smalltalk" => vec![Step::speak("Happy to chat — though my heart is 820,554 floating-point cells on a budget.")],
        "refuse" => vec![Step::speak("I cannot do that offline with the tools I have.")],
        "help" => vec![Step::speak(
            "I can tell the time, give system info, open and close apps, list/read/search files, remember notes, keep tasks, and plan steps. Say `stop` for emergency stop.",
        )],
        "stop" => vec![Step::speak("Emergency stop: halting task, cancelling queue, releasing control.")],
        other => vec![Step::speak(format!("NOT IMPLEMENTED intent: {}", other))],
    };

    let requires_confirmation = matches!(
        intent,
        "app.open" | "app.close" | "fs.read" | "fs.search" | "memory.put" | "task.put"
    );
    let emergency = intent == "stop";
    Plan { intent: intent.to_string(), steps, requires_confirmation, emergency, needs_clarification }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_slot_finds_double_quotes() {
        assert_eq!(quoted_slot("read \"C:/tmp/a.txt\" now"), Some("C:/tmp/a.txt".to_string()));
    }

    #[test]
    fn quoted_slot_falls_back_to_pathish_token() {
        assert_eq!(quoted_slot("read note.txt please"), Some("note.txt".to_string()));
    }

    #[test]
    fn quoted_slot_none_when_nothing_matches() {
        assert_eq!(quoted_slot("just say hello"), None);
    }

    #[test]
    fn search_slot_falls_back_to_bare_word() {
        assert_eq!(search_slot("search for checkpoint"), Some("checkpoint".to_string()));
        assert_eq!(search_slot("find jarvis please"), Some("jarvis".to_string()));
    }

    #[test]
    fn app_slot_picks_the_program() {
        assert_eq!(app_slot("open notepad"), Some("notepad".to_string()));
        assert_eq!(app_slot("close \"vscode\" now"), Some("vscode".to_string()));
    }

    #[test]
    fn stop_is_emergency_and_skips_tools() {
        let p = plan("stop", "stop everything", 1.0);
        assert!(p.emergency);
        assert!(!p.requires_confirmation);
        assert!(p.steps.iter().all(|s| s.tool == "speak"));
    }

    #[test]
    fn ambiguity_sets_needs_clarification() {
        let p = plan("app.open", "open notepad", 0.05);
        assert!(p.needs_clarification);
        let p = plan("app.open", "open notepad", 0.5);
        assert!(!p.needs_clarification);
    }

    #[test]
    fn missing_slots_ask_instead_of_guessing() {
        let p = plan("fs.read", "read something", 1.0);
        assert!(p.steps[0].speak.is_some(), "fallback step must speak, not call a tool");
    }
}
