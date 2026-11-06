//! What `nata sql` prints: the result as CSV (the exact text `nata-check` compares: `nata_parquet::csv`) or as an aligned table, streaming, and the
//! errors with their class, message, position and a caret under the position. Exit codes: `exit_code`.

use std::io::Write;

use arrow_array::{Array, RecordBatch};
use nata_parquet::csv;
use nata_parquet::{ErrorKind, ScanError};
use nata_sql::types::SqlType;
use nata_sql::{ErrorClass, SqlError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Csv,
    Table,
}

fn io(e: &std::io::Error) -> ScanError {
    ScanError::new(ErrorKind::Io(nata_parquet::error::redact(&e.to_string())))
}

/// Rows buffered to decide the widths of the aligned table. Later rows use the same widths (a wider cell is printed in full, unaligned).
const TABLE_PAGE_ROWS: usize = 1000;

/// Writes the result batch by batch. CSV: the header with the first batch (or at `finish` for an empty result), rows as they come. Table: the first
/// page of rows (at most 1,000) is aligned by its widest cells and the header; the footer `(N rows)` comes with `finish`.
pub struct ResultWriter<W: Write> {
    out: W,
    format: Format,
    names: Vec<String>,
    numeric: Vec<bool>,
    header_done: bool,
    page: Vec<Vec<String>>,
    widths: Option<Vec<usize>>,
    pub rows: u64,
}

impl<W: Write> ResultWriter<W> {
    pub fn new(out: W, format: Format, names: Vec<String>, types: &[SqlType]) -> ResultWriter<W> {
        let numeric = types
            .iter()
            .map(|t| t.is_exact_numeric() || t.is_float())
            .collect();
        ResultWriter {
            out,
            format,
            names,
            numeric,
            header_done: false,
            page: Vec::new(),
            widths: None,
            rows: 0,
        }
    }

    /// Rows written (or buffered in the first page of a table).
    pub fn rows_written(&self) -> u64 {
        self.rows
    }

    pub fn write_batch(&mut self, batch: &RecordBatch) -> Result<(), ScanError> {
        match self.format {
            Format::Csv => {
                if !self.header_done {
                    csv::write_header(&mut self.out, &self.names).map_err(|e| io(&e))?;
                    self.header_done = true;
                }
                csv::write_batch(&mut self.out, batch)?;
            }
            Format::Table => {
                for row in 0..batch.num_rows() {
                    let mut cells = Vec::with_capacity(batch.num_columns());
                    for col in batch.columns() {
                        cells.push(
                            csv::cell_text(col.as_ref() as &dyn Array, row)?
                                .unwrap_or_else(|| "NULL".to_string()),
                        );
                    }
                    self.page.push(cells);
                    if self.page.len() >= TABLE_PAGE_ROWS || self.widths.is_some() {
                        self.flush_page()?;
                    }
                }
            }
        }
        self.rows = self
            .rows
            .saturating_add(u64::try_from(batch.num_rows()).unwrap_or(u64::MAX));
        Ok(())
    }

    fn width(s: &str) -> usize {
        s.chars().count()
    }

    fn flush_page(&mut self) -> Result<(), ScanError> {
        if self.widths.is_none() {
            let mut w: Vec<usize> = self.names.iter().map(|n| Self::width(n)).collect();
            for row in &self.page {
                for (i, c) in row.iter().enumerate() {
                    if let Some(x) = w.get_mut(i) {
                        *x = (*x).max(Self::width(c));
                    }
                }
            }
            let mut text = String::new();
            let header: Vec<String> = self
                .names
                .iter()
                .enumerate()
                .map(|(i, n)| pad(n, w[i], false))
                .collect();
            text.push_str(&format!(" {} \n", header.join(" | ")));
            let rule: Vec<String> = w.iter().map(|x| "-".repeat(*x + 2)).collect();
            text.push_str(&rule.join("+"));
            text.push('\n');
            self.out.write_all(text.as_bytes()).map_err(|e| io(&e))?;
            self.widths = Some(w);
        }
        let w = self.widths.clone().unwrap_or_default();
        let mut text = String::new();
        for row in self.page.drain(..) {
            let cells: Vec<String> = row
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    pad(
                        c,
                        w.get(i).copied().unwrap_or(0),
                        self.numeric.get(i).copied().unwrap_or(false),
                    )
                })
                .collect();
            text.push_str(&format!(" {} \n", cells.join(" | ")));
        }
        self.out.write_all(text.as_bytes()).map_err(|e| io(&e))
    }

    /// The end of a complete result: the header of an empty CSV result, the buffered rows and the footer of a table; flushes.
    pub fn finish(&mut self) -> Result<(), ScanError> {
        match self.format {
            Format::Csv => {
                if !self.header_done {
                    csv::write_header(&mut self.out, &self.names).map_err(|e| io(&e))?;
                    self.header_done = true;
                }
            }
            Format::Table => {
                if self.widths.is_none() || !self.page.is_empty() {
                    self.flush_page()?;
                }
                let s = if self.rows == 1 { "" } else { "s" };
                writeln!(self.out, "({} row{s})", self.rows).map_err(|e| io(&e))?;
            }
        }
        self.out.flush().map_err(|e| io(&e))
    }

    /// Flush what was written so far (before an error message goes to stderr, so that the two come in the order they happened). A table's buffered
    /// first page is written too: those rows were produced and are part of what the error interrupted.
    pub fn flush_partial(&mut self) -> Result<(), ScanError> {
        if self.format == Format::Table && !self.page.is_empty() {
            self.flush_page()?;
        }
        self.out.flush().map_err(|e| io(&e))
    }
}

fn pad(s: &str, width: usize, right: bool) -> String {
    let n = s.chars().count();
    let fill = " ".repeat(width.saturating_sub(n));
    if right {
        format!("{fill}{s}")
    } else {
        format!("{s}{fill}")
    }
}

// ------------------------------------------------------------------------------------------------------------------------------ errors

/// Exit codes of `nata sql` (as `nata scan`): 1 a data or I/O error, 2 `not supported yet`, a bind or syntax error or a wrong use of the command,
/// 130 an interrupt; 0 is success.
pub fn exit_code_sql(e: &SqlError) -> u8 {
    match e.class {
        ErrorClass::Execution => 1,
        ErrorClass::Syntax | ErrorClass::Bind | ErrorClass::NotSupported => 2,
    }
}

/// The exit code of an error of the run.
pub fn exit_code_scan(e: &ScanError) -> u8 {
    match e.kind {
        ErrorKind::NotSupported(_) | ErrorKind::Usage(_) | ErrorKind::Filter(_) => 2,
        ErrorKind::Cancelled(_) => 130,
        _ => 1,
    }
}

const CONTEXT: usize = 100;

/// The line of `sql` at `line` (1-based) with a caret under `col` (1-based, in characters), as
/// ```text
///   --> line 1, column 8
///    |
///  1 | SELECT x FROM 'f'
///    |        ^
/// ```
/// A long line is cut around the column.
pub fn caret_block(sql: &str, line: u32, col: u32) -> String {
    let text = sql
        .split('\n')
        .nth(usize::try_from(line.saturating_sub(1)).unwrap_or(0))
        .unwrap_or("");
    let text = text.strip_suffix('\r').unwrap_or(text);
    let chars: Vec<char> = text.chars().collect();
    let at = usize::try_from(col.saturating_sub(1))
        .unwrap_or(0)
        .min(chars.len());
    let (from, to) = (at.saturating_sub(CONTEXT), (at + CONTEXT).min(chars.len()));
    let shown: String = chars[from..to]
        .iter()
        .map(|c| if c.is_control() { ' ' } else { *c })
        .collect();
    let (pre, post) = (
        if from > 0 { "..." } else { "" },
        if to < chars.len() { "..." } else { "" },
    );
    let num = line.to_string();
    let gutter = " ".repeat(num.len());
    format!(
        "{gutter} --> line {line}, column {col}\n{gutter} |\n{num} | {pre}{shown}{post}\n{gutter} | {}^\n",
        " ".repeat(pre.len() + (at - from))
    )
}

/// An error of the front end: `error: <class>: <message>` and the caret block.
pub fn render_sql_error(sql: &str, e: &SqlError) -> String {
    let mut head = match e.class {
        ErrorClass::NotSupported => format!("error: not supported yet: {}", e.message),
        c => format!("error: {}: {}", c.name(), e.message),
    };
    if let Some(s) = &e.suggestion {
        head.push_str(&format!(" (did you mean `{s}`?)"));
    }
    head.push('\n');
    if e.pos.is_none() {
        return head;
    }
    head + &caret_block(sql, e.pos.line, e.pos.col)
}

/// An error of the run: the scan's text (it names the file and the row group); an error of an expression also gets the caret block.
/// `rows_printed` > 0 adds that the result is incomplete.
pub fn render_scan_error(sql: &str, e: &ScanError, rows_printed: u64) -> String {
    let mut out = format!("error: {e}\n");
    if let ErrorKind::Eval { line, column, .. } = &e.kind {
        out.push_str(&caret_block(sql, *line, *column));
    }
    if rows_printed > 0 {
        let s = if rows_printed == 1 { "" } else { "s" };
        out.push_str(&format!(
            "error: the query failed after {rows_printed} row{s} had been printed: the output above is INCOMPLETE and must not be used as the result\n"
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_caret_stands_under_the_column() {
        let b = caret_block("SELECT x FROM 'f'", 1, 8);
        assert!(b.contains("1 | SELECT x FROM 'f'"), "{b}");
        let lines: Vec<&str> = b.lines().collect();
        assert_eq!(lines[3], "  |        ^", "{b}");
        let b = caret_block("SELECT 1\nFROM 'f'\nWHERE x", 3, 7);
        assert!(b.contains("3 | WHERE x"), "{b}");
        assert!(b.lines().any(|l| l == "  |       ^"), "{b}");
    }

    #[test]
    fn a_long_line_is_cut_around_the_column() {
        let sql = format!("SELECT {} FROM 'f'", "a, ".repeat(2000));
        let b = caret_block(&sql, 1, 3000);
        assert!(b.lines().all(|l| l.chars().count() < 260), "{b}");
        assert!(b.contains("..."));
        // column beyond the line (the end of the input)
        let b = caret_block("SELECT", 1, 7);
        assert!(b.contains("^"), "{b}");
    }
}
