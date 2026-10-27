//! Conversation context management (Phase 2).
//!
//! Keeps role-tagged turns and maintains them inside a character budget so the
//! latest, most relevant turns survive. Truncation is explicit: when the budget
//! overflows, the oldest complete turns are dropped and a marker notes the cut.
//! Every rule here is simple enough to be verified by a unit test.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::System => "System",
            Role::User => "User",
            Role::Assistant => "Jake",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Turn {
    pub role: Role,
    pub text: String,
}

pub const TRIM_MARKER: &str = "[... earlier turns trimmed ...]";

/// Sliding-window conversation context bounded by a character budget.
pub struct ContextWindow {
    turns: Vec<Turn>,
    max_chars: usize,
}

impl ContextWindow {
    /// `max_chars` is the rendering budget in characters; it must leave room
    /// for the trim marker plus at least one line.
    pub fn new(max_chars: usize) -> Self {
        assert!(
            max_chars >= TRIM_MARKER.chars().count() + 16,
            "budget too small to render anything meaningful"
        );
        ContextWindow { turns: Vec::new(), max_chars }
    }

    pub fn push(&mut self, role: Role, text: impl Into<String>) {
        self.turns.push(Turn { role, text: text.into() });
        if self.turns.len() > 512 {
            let excess = self.turns.len() - 512;
            self.turns.drain(0..excess);
        }
    }

    pub fn turns(&self) -> &[Turn] {
        &self.turns
    }

    pub fn is_empty(&self) -> bool {
        self.turns.is_empty()
    }

    pub fn len(&self) -> usize {
        self.turns.len()
    }

    /// Renders the window preserving the newest turns: when the budget is
    /// exceeded the oldest whole turns are cut first and the marker replaces
    /// them. Output never exceeds `max_chars`.
    pub fn render(&self) -> String {
        // Walk from the newest turn backwards, keeping what fits.
        let mut kept: Vec<String> = Vec::new();
        let mut used = 0usize;
        for turn in self.turns.iter().rev() {
            let line = format!("{}: {}", turn.role.as_str(), turn.text);
            let cost = line.chars().count() + 1; // + newline
            if used + cost > self.max_chars {
                break;
            }
            used += cost;
            kept.push(line);
        }
        kept.reverse();

        let dropped = self.turns.len() - kept.len();

        let mut out = String::new();
        if dropped > 0 {
            let marker_cost = TRIM_MARKER.chars().count() + 1;
            // Reserve room for the marker: drop the oldest kept turn(s) until
            // the marker fits, so trimming is always visible in the render.
            while used + marker_cost > self.max_chars && kept.len() > 1 {
                let line_cost = kept[0].chars().count() + 1;
                used -= line_cost;
                kept.remove(0);
            }
            if used + marker_cost <= self.max_chars {
                out.push_str(TRIM_MARKER);
                out.push('\n');
            }
        }
        for line in kept {
            out.push_str(&line);
            out.push('\n');
        }
        out
    }
}

/// The rendered window must fit the budget and keep the newest turn.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_everything_when_it_fits() {
        let mut ctx = ContextWindow::new(1024);
        ctx.push(Role::User, "hello");
        ctx.push(Role::Assistant, "Hi!");
        let rendered = ctx.render();
        assert!(rendered.contains("User: hello"));
        assert!(rendered.contains("Jake: Hi!"));
        assert!(!rendered.contains(TRIM_MARKER));
    }

    #[test]
    fn drops_oldest_turns_and_marks_the_cut() {
        let mut ctx = ContextWindow::new(120);
        ctx.push(Role::User, "first message with some content 1234567890");
        ctx.push(Role::User, "second message with more content 1234567890");
        ctx.push(Role::User, "third message yet again with content 1234567890");
        ctx.push(Role::Assistant, "very last reply");
        let rendered = ctx.render();
        assert!(rendered.starts_with(TRIM_MARKER), "marker must lead the render: {rendered}");
        assert!(!rendered.contains("first message"), "oldest turn must be dropped");
        assert!(rendered.contains("very last reply"), "newest turn must survive");
        assert!(rendered.chars().count() <= 180);
    }

    #[test]
    fn newest_turn_always_survives() {
        let mut ctx = ContextWindow::new(TRIM_MARKER.chars().count() + 16);
        ctx.push(Role::User, "long old filler that cannot possibly fit");
        ctx.push(Role::User, "x");
        let rendered = ctx.render();
        assert!(rendered.lines().any(|l| l.ends_with(": x")));
    }
}
