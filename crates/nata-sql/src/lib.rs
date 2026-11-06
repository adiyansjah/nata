//! The SQL front end of slice 4.
pub mod ast;
pub mod bind;
pub mod error;
pub mod glob;
pub mod keywords;
pub mod parse;
pub mod plan;
pub mod source;
pub mod texpr;
pub mod types;

pub use error::{ErrorClass, Pos, Result, SqlError};
pub use plan::{LogicalPlan, Planned};
pub use source::{BindOptions, FsSource, SourceProvider};

/// Parse, bind and plan one statement: `SELECT ... FROM '<file or glob>' [WHERE ...] [LIMIT n [OFFSET m]]`, or `EXPLAIN [ANALYZE]` of it.
///
/// Everything runs on a thread with a 64 MiB stack (the parser needs it for the nesting the dialect allows). Errors carry their class and the line and
/// column of the construct (`docs/dialect.md` section 8). The returned plan is ready for step 3: [`LogicalPlan::scan`] has the files, the columns and the
/// pushed conjuncts (see [`plan::Scan::scan_options`]), the operators above it hold typed expressions over the scan's columns.
pub fn plan_sql(sql: &str, source: &dyn SourceProvider) -> Result<Planned> {
    plan_sql_with(sql, source, BindOptions::default())
}

/// As [`plan_sql`]; `options.all_footers` reads the footer of every file of a glob before the query starts (the behaviour before register item P10).
pub fn plan_sql_with(
    sql: &str,
    source: &dyn SourceProvider,
    options: BindOptions,
) -> Result<Planned> {
    parse::on_big_stack(|| {
        let q = parse::parse_query_here(sql)?;
        let bound = bind::bind_with(&q, source, options)?;
        plan::plan(bound)
    })?
}

#[cfg(test)]
mod parse_tests;
#[cfg(test)]
mod testutil;
