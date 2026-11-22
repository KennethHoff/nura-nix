//! The text under every step header, in one table so it can be reviewed on one screen. Every step is
//! done by this tool (a `Does:` line), and the steps run in strict order. `Takes:` is a static typical
//! range, never a live estimate. `Does:` lines are `-` bullets.

pub struct Step {
    pub title: &'static str,
    pub takes: Option<&'static str>,
    pub text: &'static [&'static str],
    pub phone: Option<&'static str>,
}

const fn tool(
    title: &'static str,
    takes: Option<&'static str>,
    text: &'static [&'static str],
    phone: Option<&'static str>,
) -> Step {
    Step {
        title,
        takes,
        text,
        phone,
    }
}

const NOT_TOUCHED: &str = "untouched";

pub const CHECK_COMPUTER: Step = tool(
    "Check this computer",
    Some("under 1 min"),
    &["- programs this run needs start", "- free disk space"],
    Some(NOT_TOUCHED),
);

#[cfg(test)]
pub const ALL: [&Step; 1] = [&CHECK_COMPUTER];

const LABEL: usize = 12;

fn field(out: &mut Vec<String>, label: &str, lines: &[&str]) {
    for (i, line) in lines.iter().enumerate() {
        let name = if i == 0 { label } else { "" };
        out.push(format!("  {name:<LABEL$}{line}"));
    }
}

pub fn header(n: u32, total: u32, step: &Step) -> String {
    format!("== Step {n} of {total} · TOOL: {}", step.title)
}

/// The labelled lines under a header; a line appears only when it applies.
pub fn block(step: &Step) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(takes) = step.takes {
        field(&mut out, "Takes:", &[takes]);
    }
    field(&mut out, "Does:", step.text);
    if let Some(phone) = step.phone {
        field(&mut out, "Phone:", &[phone]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_block_aligns_its_labels_and_hangs_continuations() {
        assert_eq!(
            block(&CHECK_COMPUTER),
            [
                "  Takes:      under 1 min",
                "  Does:       - programs this run needs start",
                "              - free disk space",
                "  Phone:      untouched",
            ]
        );
    }

    #[test]
    fn every_step_says_what_the_tool_does() {
        for step in ALL {
            let lines = block(step);
            assert_eq!(
                lines.iter().filter(|l| l.starts_with("  Does:")).count(),
                1,
                "{}",
                step.title
            );
            assert!(
                !lines.iter().any(|l| l.starts_with("  Do:")),
                "{}",
                step.title
            );
        }
    }

    #[test]
    fn a_header_numbers_the_step_and_names_its_title() {
        assert_eq!(
            header(1, 14, &CHECK_COMPUTER),
            "== Step 1 of 14 · TOOL: Check this computer"
        );
    }
}
