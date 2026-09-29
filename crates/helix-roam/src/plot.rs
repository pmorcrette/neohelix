//! Plotting a table through gnuplot, Org's `org-plot`.
//!
//! The `#+PLOT:` lines above a table say how:
//!
//! ```text
//! #+PLOT: title:"Visits" ind:1 deps:(2 3) type:2d with:lines file:"visits.svg"
//! #+PLOT: set:"yrange [0:]"
//! | Day | Home | Blog |
//! |-----+------+------|
//! | Mon |   12 |    3 |
//! ```
//!
//! This builds the data and the gnuplot script; running gnuplot is the
//! editor's. Without a `file:`, the plot is drawn in text (gnuplot's `dumb`
//! terminal) for the editor to show, where Org would open a window.

use std::path::{Path, PathBuf};

use crate::table::{parse_table, Row};

/// What plotting a table needs: its data, and the script that reads it
/// from `{data}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plot {
    /// Tab-separated, one row per line.
    pub data: String,
    /// The script, `{data}` standing for the data file's path.
    pub script: String,
    /// Where the plot is written, or `None` to draw it in text.
    pub file: Option<PathBuf>,
}

/// The `key:value` options of `#+PLOT:` lines.
#[derive(Debug, Clone, Default)]
struct Options {
    title: Option<String>,
    ind: Option<usize>,
    deps: Vec<usize>,
    kind: Option<String>,
    with: Option<String>,
    file: Option<String>,
    labels: Vec<String>,
    sets: Vec<String>,
    line: Option<String>,
    timefmt: Option<String>,
    xlabel: Option<String>,
    ylabel: Option<String>,
    map: bool,
}

/// The value after `key:`: a quoted string, a parenthesised list or a word.
fn values(line: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let mut at = 0;
    while at < chars.len() {
        while at < chars.len() && chars[at].is_whitespace() {
            at += 1;
        }
        let key_start = at;
        while at < chars.len() && chars[at] != ':' && !chars[at].is_whitespace() {
            at += 1;
        }
        if at >= chars.len() || chars[at] != ':' {
            continue;
        }
        let key: String = chars[key_start..at].iter().collect();
        at += 1;
        let mut value = String::new();
        match chars.get(at) {
            Some('"') => {
                at += 1;
                while at < chars.len() && chars[at] != '"' {
                    if chars[at] == '\\' && at + 1 < chars.len() {
                        at += 1;
                    }
                    value.push(chars[at]);
                    at += 1;
                }
                at += 1;
            }
            Some('(') => {
                let mut depth = 0;
                while at < chars.len() {
                    value.push(chars[at]);
                    match chars[at] {
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                at += 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                    at += 1;
                }
            }
            _ => {
                while at < chars.len() && !chars[at].is_whitespace() {
                    value.push(chars[at]);
                    at += 1;
                }
            }
        }
        out.push((key, value));
    }
    out
}

/// The strings of a list `("a" "b")`, or its words `(2 3)`.
fn list_items(value: &str) -> Vec<String> {
    let inner = value.trim().trim_start_matches('(').trim_end_matches(')');
    let mut items = Vec::new();
    let mut chars = inner.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else if c == '"' {
            chars.next();
            let item: String = chars.by_ref().take_while(|&c| c != '"').collect();
            items.push(item);
        } else {
            let mut item = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                item.push(c);
                chars.next();
            }
            items.push(item);
        }
    }
    items
}

fn options(lines: &[&str]) -> Result<Options, String> {
    let mut options = Options::default();
    let column = |key: &str, value: &str| -> Result<usize, String> {
        value
            .parse::<usize>()
            .ok()
            .filter(|&n| n > 0)
            .ok_or(format!("{key}:{value} is not a column number"))
    };
    for line in lines {
        for (key, value) in values(line) {
            match key.as_str() {
                "title" => options.title = Some(value),
                "ind" => options.ind = Some(column("ind", &value)?),
                "deps" => {
                    options.deps = list_items(&value)
                        .iter()
                        .map(|n| column("deps", n))
                        .collect::<Result<_, _>>()?
                }
                "type" => options.kind = Some(value),
                "with" => options.with = Some(value),
                "file" => options.file = Some(value),
                "labels" => options.labels = list_items(&value),
                "set" => options.sets.push(value),
                "line" => options.line = Some(value),
                "timefmt" => options.timefmt = Some(value),
                "xlabel" => options.xlabel = Some(value),
                "ylabel" => options.ylabel = Some(value),
                "map" => options.map = value != "nil",
                "script" => return Err("script: is not supported; use set: lines".to_string()),
                // Radar and the like have options this does not draw.
                _ => {}
            }
        }
    }
    Ok(options)
}

/// gnuplot's single-quoted string.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// The terminal for a file, from its extension.
fn terminal(file: &Path) -> Result<&'static str, String> {
    let extension = file
        .extension()
        .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    Ok(match extension.as_str() {
        "png" => "png",
        "svg" => "svg",
        "pdf" => "pdfcairo",
        "jpg" | "jpeg" => "jpeg",
        "gif" => "gif",
        "eps" => "postscript eps color",
        "txt" => "dumb",
        other => return Err(format!("no gnuplot terminal for .{other} files")),
    })
}

/// The plot of the table at `line`, from the `#+PLOT:` lines above it;
/// `org_dir` is where a relative `file:` goes. `columns` and `lines` size
/// the text drawing.
pub fn plan(text: &str, line: usize, org_dir: &Path, size: (u16, u16)) -> Result<Plot, String> {
    let table = parse_table(text, line).ok_or("No table at the cursor")?;
    let lines: Vec<&str> = text.lines().collect();
    let mut plot_lines = Vec::new();
    let mut above = table.start;
    while above > 0 && lines[above - 1].trim_start().starts_with("#+") {
        above -= 1;
        let keyword = lines[above].trim_start();
        if keyword.len() >= 7 && keyword[..7].eq_ignore_ascii_case("#+PLOT:") {
            plot_lines.push(&keyword[7..]);
        }
    }
    plot_lines.reverse();
    let options = options(&plot_lines)?;

    // The header is the rows above the first separator, when rows follow.
    let columns = table.columns();
    let first_hline = table
        .rows
        .iter()
        .position(|row| matches!(row, Row::Separator));
    let header: Option<&Vec<String>> = first_hline.and_then(|at| {
        table.rows[..at].iter().rev().find_map(|row| match row {
            Row::Cells(cells) => Some(cells),
            Row::Separator => None,
        })
    });
    let body: Vec<Vec<String>> = table.rows[first_hline.map_or(0, |at| at + 1)..]
        .iter()
        .filter_map(|row| match row {
            Row::Cells(cells) => {
                let mut cells = cells.clone();
                cells.resize(columns, String::new());
                Some(cells)
            }
            Row::Separator => None,
        })
        .collect();
    if body.is_empty() {
        return Err("The table has no rows to plot".to_string());
    }
    for &column in options.deps.iter().chain(options.ind.iter()) {
        if column > columns {
            return Err(format!("the table has no column {column}"));
        }
    }

    let numeric = |column: usize| {
        body.iter().all(|row| {
            let cell = row[column - 1].trim();
            cell.is_empty() || cell.parse::<f64>().is_ok()
        })
    };
    let title_of = |column: usize, nth: usize| -> String {
        options
            .labels
            .get(nth)
            .cloned()
            .or_else(|| header.and_then(|header| header.get(column - 1).cloned()))
            .unwrap_or_else(|| format!("Column {column}"))
    };

    let data: String = body
        .iter()
        .map(|row| {
            let mut line = row
                .iter()
                .map(|cell| cell.replace('\t', " "))
                .collect::<Vec<_>>()
                .join("\t");
            line.push('\n');
            line
        })
        .collect();

    let file = options.file.as_ref().map(|file| org_dir.join(file));
    let mut script = Vec::new();
    match &file {
        Some(file) => {
            script.push(format!("set terminal {}", terminal(file)?));
            script.push(format!("set output {}", quote(&file.to_string_lossy())));
        }
        None => script.push(format!("set terminal dumb size {},{}", size.0, size.1)),
    }
    if let Some(title) = &options.title {
        script.push(format!("set title {}", quote(title)));
    }
    if let Some(label) = &options.xlabel {
        script.push(format!("set xlabel {}", quote(label)));
    }
    if let Some(label) = &options.ylabel {
        script.push(format!("set ylabel {}", quote(label)));
    }
    script.push("set datafile separator \"\\t\"".to_string());
    if let Some(timefmt) = &options.timefmt {
        script.push("set xdata time".to_string());
        script.push(format!("set timefmt {}", quote(timefmt)));
    }
    let with = options.with.clone().unwrap_or("lines".to_string());
    script.push(format!("set style data {with}"));
    if with.starts_with("histogram") {
        script.push("set style fill solid border -1".to_string());
    }
    for set in &options.sets {
        script.push(format!("set {set}"));
    }

    match options.kind.as_deref().unwrap_or("2d") {
        "2d" => {
            let ind = options.ind;
            let deps: Vec<usize> = if options.deps.is_empty() {
                (1..=columns)
                    .filter(|&c| Some(c) != ind && numeric(c))
                    .collect()
            } else {
                options.deps.clone()
            };
            if deps.is_empty() {
                return Err("No column of numbers to plot: give deps:".to_string());
            }
            let text_ind = ind.filter(|&ind| !numeric(ind) || options.timefmt.is_some());
            let histogram = with.starts_with("histogram");
            let parts: Vec<String> = deps
                .iter()
                .enumerate()
                .map(|(nth, &dep)| {
                    let using = match (ind, text_ind) {
                        (Some(ind), _) if options.timefmt.is_some() => format!("{ind}:{dep}"),
                        (Some(ind), Some(_)) if histogram => format!("{dep}:xtic({ind})"),
                        (Some(ind), Some(_)) => format!("0:{dep}:xticlabels({ind})"),
                        (Some(ind), None) if histogram => format!("{dep}:xtic({ind})"),
                        (Some(ind), None) => format!("{ind}:{dep}"),
                        (None, _) => format!("{dep}"),
                    };
                    let file = if nth == 0 { "'{data}'" } else { "''" };
                    format!("{file} using {using} title {}", quote(&title_of(dep, nth)))
                })
                .collect();
            let mut plot = format!("plot {}", parts.join(", "));
            if let Some(line) = &options.line {
                plot = format!("plot {line}");
            }
            script.push(plot);
        }
        kind @ ("3d" | "grid") => {
            // A matrix of heights: the table's numbers, the `ind` column
            // (row labels) left out.
            let matrix: String = body
                .iter()
                .map(|row| {
                    let mut line = row
                        .iter()
                        .enumerate()
                        .filter(|(at, _)| Some(at + 1) != options.ind)
                        .map(|(_, cell)| {
                            let cell = cell.trim();
                            if cell.is_empty() {
                                "0".to_string()
                            } else {
                                cell.to_string()
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\t");
                    line.push('\n');
                    line
                })
                .collect();
            if kind == "grid" || options.map {
                script.push("set view map".to_string());
                script.push("splot '{data}' matrix with image notitle".to_string());
            } else {
                script.push(format!("splot '{{data}}' matrix with {with} notitle"));
            }
            return Ok(Plot {
                data: matrix,
                script: join(script),
                file,
            });
        }
        other => return Err(format!("type:{other} is not supported (2d, 3d or grid)")),
    }

    Ok(Plot {
        data,
        script: join(script),
        file,
    })
}

fn join(lines: Vec<String>) -> String {
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "\
#+NAME: visits
#+PLOT: title:\"Visits\" ind:1 deps:(2 3) with:histograms file:\"out/v.svg\"
#+PLOT: set:\"yrange [0:]\"
| Day | Home | Blog |
|-----+------+------|
| Mon |   12 |    3 |
| Tue |    9 |      |
";

    #[test]
    fn a_histogram_into_a_file() {
        let plot = plan(DOC, 4, Path::new("/n"), (80, 25)).unwrap();
        assert_eq!(plot.file.as_deref(), Some(Path::new("/n/out/v.svg")));
        assert_eq!(plot.data, "Mon\t12\t3\nTue\t9\t\n");
        assert_eq!(
            plot.script,
            "set terminal svg\nset output '/n/out/v.svg'\nset title 'Visits'\n\
             set datafile separator \"\\t\"\nset style data histograms\n\
             set style fill solid border -1\nset yrange [0:]\n\
             plot '{data}' using 2:xtic(1) title 'Home', '' using 3:xtic(1) title 'Blog'\n"
        );
    }

    #[test]
    fn lines_in_text_by_default() {
        let text = "| x | y |\n|---+---|\n| 1 | 2 |\n| 2 | 4 |\n";
        let plot = plan(text, 0, Path::new("/n"), (60, 20)).unwrap();
        assert!(plot.file.is_none());
        assert!(plot.script.starts_with("set terminal dumb size 60,20\n"));
        assert!(
            plot.script
                .ends_with("plot '{data}' using 1 title 'x', '' using 2 title 'y'\n"),
            "{}",
            plot.script
        );
        let with_ind = format!("#+PLOT: ind:1\n{text}");
        let plot = plan(&with_ind, 1, Path::new("/n"), (60, 20)).unwrap();
        assert!(plot.script.ends_with("plot '{data}' using 1:2 title 'y'\n"));
        assert!(plan("| a |\n| b |\n", 0, Path::new("/n"), (60, 20)).is_err());
        let three = format!("#+PLOT: type:grid\n{text}");
        let plot = plan(&three, 1, Path::new("/n"), (60, 20)).unwrap();
        assert!(plot
            .script
            .ends_with("splot '{data}' matrix with image notitle\n"));
    }
}
