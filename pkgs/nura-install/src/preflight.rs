//! The Preflight section: every read-only check of the run, in order, each with a mark. It does not
//! stop at the first failure: it runs every check it can, reports all the failures together and only
//! then stops. It changes nothing.
use crate::fail::{Fail, Result};
use crate::host;
use crate::render::{plural, Mark};
use crate::ui::{self, Job};
use std::path::Path;

const FIX_TOOLS: &str = "[ ] Install or fix the program named above  [ ] Run again.";
const FIX_DISK: &str = "[ ] Free space in the cache directory, or move it  [ ] Run again.";

struct Row {
    mark: Mark,
    label: String,
    detail: String,
    fix: &'static str,
}

pub struct Report {
    rows: Vec<Row>,
}

impl Report {
    pub fn new() -> Self {
        Report { rows: Vec::new() }
    }

    fn record(&mut self, mark: Mark, label: &str, detail: &str, fix: &'static str) {
        self.rows.push(Row {
            mark,
            label: label.to_string(),
            detail: detail.to_string(),
            fix,
        });
    }

    fn check<T>(
        &mut self,
        label: &str,
        fix: &'static str,
        work: impl FnOnce() -> Result<(T, String)>,
    ) -> Option<T> {
        self.checked(Job::new(label), label, fix, work)
    }

    /// A check whose wait is named up front, with what it waits for and how long.
    fn check_named<T>(
        &mut self,
        label: &str,
        naming: String,
        fix: &'static str,
        work: impl FnOnce() -> Result<(T, String)>,
    ) -> Option<T> {
        self.checked(Job::new(label).naming(naming), label, fix, work)
    }

    fn checked<T>(
        &mut self,
        job: Job,
        label: &str,
        fix: &'static str,
        work: impl FnOnce() -> Result<(T, String)>,
    ) -> Option<T> {
        let mut shown = String::new();
        let result = ui::run(job.explain(), || {
            let (value, detail) = work()?;
            shown = detail.clone();
            Ok((value, detail))
        });
        match result {
            Ok(value) => {
                self.record(Mark::Pass, label, &shown, fix);
                Some(value)
            }
            Err(e) => {
                self.record(Mark::Fail, label, &e.to_string(), fix);
                None
            }
        }
    }

    fn count(&self, mark: Mark) -> usize {
        self.rows.iter().filter(|r| r.mark == mark).count()
    }

    /// Ends the Preflight: silent success, or every failure with what to do, and a stop.
    pub fn conclude(&self, run_again: &str) -> Result<()> {
        let failed = self.count(Mark::Fail);
        if failed == 0 {
            ui::say(&format!(
                "  Preflight passed: {}",
                plural(self.rows.len(), "check")
            ));
            return Ok(());
        }
        ui::say(&format!("\nNot ready: {} failed.", plural(failed, "check")));
        ui::say("What each one means and what to do:");
        for row in self.rows.iter().filter(|row| row.mark == Mark::Fail) {
            let first = row.detail.lines().next().unwrap_or("");
            ui::say(&format!("  - {}: {first}\n      {}", row.label, row.fix));
        }
        Err(Fail::Stop(format!(
            "Preflight not ready: {} failed; nothing was changed. Next: fix what is listed above and run `{run_again}` again",
            plural(failed, "check")
        )))
    }
}

/// This computer: the programs and the disk space.
pub fn computer(r: &mut Report, cache: &Path) {
    r.check_named("tools", host::tools_naming(), FIX_TOOLS, host::check_tools);
    r.check("disk space", FIX_DISK, || host::check_disk(cache));
}
