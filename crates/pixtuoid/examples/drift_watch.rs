//! The upstream drift watch upstream-drift.yml runs (`just drift-watch`): prints
//! the report, whose H1 is the tracking issue's title, and exits 0 with no
//! findings, 1 on an actionable one, 2 on transient fetch trouble alone.

use std::io::Write;
use std::process::ExitCode;

use pixtuoid::dev::drift_watch::{Report, run};

fn main() -> ExitCode {
    let mut report = Report::default();
    run(&mut report);
    let _ = writeln!(std::io::stdout(), "{}", report.render());
    ExitCode::from(report.exit_code())
}
