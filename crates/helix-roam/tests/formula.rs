//! Org table formulas.

use helix_roam::formula::{iterate, recalculate, recalculate_all};

fn recalc(text: &str) -> String {
    recalculate(text, 0).unwrap().text
}

#[test]
fn a_column_formula_fills_every_row_below_the_header() {
    let text = "\
| Item | Qty | Price | Total |
|------+-----+-------+-------|
| tea  | 2   | 1.5   |       |
| milk | 3   | 2     |       |
#+TBLFM: $4=$2*$3
";
    assert_eq!(
        recalc(text),
        "\
| Item | Qty | Price | Total |
|------+-----+-------+-------|
| tea  |   2 |   1.5 |     3 |
| milk |   3 |     2 |     6 |
#+TBLFM: $4=$2*$3
"
    );
}

#[test]
fn a_field_formula_sums_between_separators_and_wins_over_the_column() {
    let text = "\
| Item | Total |
|------+-------|
| tea  | 3     |
| milk | 6     |
|------+-------|
| Sum  |       |
#+TBLFM: @>$2=vsum(@I..@II)
";
    let out = recalculate(text, 0).unwrap();
    assert!(out.text.contains("| Sum  |     9 |"), "{}", out.text);
    assert_eq!(out.errors, 0);
    assert_eq!(out.formulas, 1);

    let both = text.replace("#+TBLFM: ", "#+TBLFM: $2=$2*10::");
    let out = recalc(&both);
    assert!(out.contains("| tea  |    30 |"), "{out}");
    // The column formula wrote 0 into the Sum row; the field formula then
    // replaced it with the sum of what the column formula had just written.
    assert!(out.contains("| Sum  |    90 |"), "{out}");
}

#[test]
fn a_running_total_reads_the_row_above_once_it_is_done() {
    let text = "\
| Day | Amount | Balance |
|-----+--------+---------|
| 1   | 10     | 10      |
| 2   | 5      |         |
| 3   | -2     |         |
#+TBLFM: @3$3..@>$3=@-1$3+$2
";
    let out = recalc(text);
    assert!(out.contains("|   2 |      5 |      15 |"), "{out}");
    assert!(out.contains("|   3 |     -2 |      13 |"), "{out}");
}

#[test]
fn references_first_last_relative_and_hline_offsets() {
    let text = "\
| a | b |
|---+---|
| 1 | 2 |
| 3 | 4 |
| 5 | 6 |
|   |   |
#+TBLFM: @>$1=@<<$1+@>>$<::@>$2=@I+1$>*@-1$-1::@2$2=$>
";
    let out = recalc(text);
    // @<< is the second row of cells (1, 2): the header is the first. @>>
    // is the one before last (5, 6), @I+1 the first under the separator.
    assert!(out.contains("| 6 | 10 |"), "{out}");
}

#[test]
fn vector_functions_skip_empty_fields() {
    let text = "\
| x | mean | count | min | max | median | prod |
|---+------+-------+-----+-----+--------+------|
| 4 |      |       |     |     |        |      |
|   |      |       |     |     |        |      |
| 2 |      |       |     |     |        |      |
| 9 |      |       |     |     |        |      |
#+TBLFM: @2$2=vmean(@2$1..@>$1)::@2$3=vcount(@2$1..@>$1)::@2$4=vmin(@2$1..@>$1)::@2$5=vmax(@2$1..@>$1)::@2$6=vmedian(@2$1..@>$1)::@2$7=vprod(@2$1..@>$1)
";
    let out = recalc(text);
    assert!(
        out.contains("| 4 |    5 |     3 |   2 |   9 |      4 |   72 |"),
        "{out}"
    );
}

#[test]
fn formats_and_arithmetic() {
    let text = "\
| a | b | c | d | e | f | g |
|---+---+---+---+---+---+---|
| 1 | 3 |   |   |   |   |   |
#+TBLFM: $3=$1/$2::$4=$1/$2;%.2f::$5=$1/$2;f3::$6=-2^2+7%3::$7=round($1/$2, 1)*10;%d
";
    let out = recalc(text);
    assert!(
        out.contains("| 1 | 3 | 0.33333333 | 0.33 | 0.333 | -3 | 3 |"),
        "{out}"
    );
}

#[test]
fn text_is_a_variable_or_an_error_unless_the_n_mode_says_it_is_zero() {
    let text = "\
| a     | b |
|-------+---|
| 1     |   |
| two   |   |
| n/a?  |   |
| 3 eggs |  |
#+TBLFM: $2=$1*2
";
    let out = recalculate(text, 0).unwrap();
    // A word is a variable to Calc, as it is in Org.
    assert!(out.text.contains("| two    | 2 two  |"), "{}", out.text);
    assert!(out.text.contains("| n/a?   | #ERROR |"), "{}", out.text);
    assert!(out.text.contains("| 3 eggs | 6 eggs |"), "{}", out.text);
    assert_eq!(out.errors, 1);

    let lenient = recalculate(&text.replace("$1*2", "$1*2;N"), 0).unwrap();
    assert!(lenient.text.contains("| two    | 0 |"), "{}", lenient.text);
    assert!(lenient.text.contains("| 3 eggs | 6 |"), "{}", lenient.text);
    assert_eq!(lenient.errors, 0);
}

#[test]
fn division_by_zero_and_references_off_the_table_are_errors_in_the_field() {
    let text = "\
| a | b |
|---+---|
| 0 |   |
#+TBLFM: $2=1/$1::@2$1=@9$1
";
    let out = recalculate(text, 0).unwrap();
    assert!(out.text.contains("| #ERROR | #ERROR |"), "{}", out.text);
    assert_eq!(out.errors, 2);
}

#[test]
fn a_bad_formula_is_reported_rather_than_applied() {
    let base = "| a |\n|---|\n| 1 |\n#+TBLFM: ";
    for (formula, says) in [
        ("$1=1+'(+ 1 2)", "Emacs Lisp"),
        ("$1=frob($1)", "unknown function"),
        ("$1=($1", "missing `)`"),
        ("$-1=2", "absolute column"),
        ("@-1$1=2", "row and column"),
        ("$1=$price", "named columns"),
        ("$1=1;%x", "unsupported format"),
        ("$1", "needs `=`"),
    ] {
        let error = recalculate(&format!("{base}{formula}\n"), 0).unwrap_err();
        assert!(error.contains(says), "{formula}: {error}");
    }
}

#[test]
fn without_a_separator_every_row_is_computed() {
    let out = recalculate("| a | b |\n| 2 |   |\n#+TBLFM: $2=$1*10\n", 0).unwrap();
    assert!(out.text.contains("| a | 10 a |"), "{}", out.text);
    assert!(out.text.contains("| 2 | 20   |"), "{}", out.text);
}

#[test]
fn a_range_outside_a_function_is_an_error() {
    let out = recalculate("| 1 | 2 |   |\n#+TBLFM: $3=$1..$2\n", 0).unwrap();
    assert_eq!(out.errors, 1);
}

#[test]
fn recalculating_from_a_tblfm_line_uses_that_line() {
    let text = "\
| a | b |
|---+---|
| 2 |   |
#+TBLFM: $2=$1*10
#+TBLFM: $2=$1+10
";
    assert!(recalculate(text, 0).unwrap().text.contains("| 2 | 20 |"));
    assert!(recalculate(text, 3).unwrap().text.contains("| 2 | 20 |"));
    assert!(recalculate(text, 4).unwrap().text.contains("| 2 | 12 |"));
}

#[test]
fn a_table_without_formulas_or_no_table_is_reported() {
    assert_eq!(
        recalculate("| a |\n", 0).unwrap_err(),
        "The table has no #+TBLFM: line"
    );
    assert_eq!(
        recalculate("text\n", 0).unwrap_err(),
        "No Org table at the cursor"
    );
}

#[test]
fn iterating_settles_formulas_that_read_further_down() {
    let text = "\
| a | b |
|---+---|
| 1 |   |
| 2 |   |
#+TBLFM: @2$2=@3$2*2::@3$2=$1+1
";
    // One pass reads @3$2 before it is written.
    assert!(recalc(text).contains("| 1 | 0 |"));
    let settled = iterate(text, 0).unwrap().text;
    assert!(settled.contains("| 1 | 6 |"), "{settled}");
}

#[test]
fn every_table_in_the_buffer_is_recalculated() {
    let text = "\
* One
| 1 |   |
#+TBLFM: $2=$1+1

* Two
  | 5 |   |
  #+TBLFM: $2=$1*$1
| no formulas |
";
    let (out, tables, errors) = recalculate_all(text).unwrap();
    assert_eq!((tables, errors), (2, 0));
    assert!(out.contains("| 1 | 2 |"), "{out}");
    assert!(out.contains("  | 5 | 25 |"), "{out}");
}

#[test]
fn dates_subtract_to_days_and_move_by_them() {
    let text = "\
| Start            | End                    | Days | Next             |
|------------------+------------------------+------+------------------|
| <2026-09-01 Tue> | <2026-09-29 Tue>       |      |                  |
| [2026-01-30 Fri] | <2026-01-31 Sat 12:00> |      |                  |
#+TBLFM: $3=$2-$1::$4=$1+7::@3$4=incmonth($1, 1)
";
    let out = recalc(text);
    assert!(out.contains("|   28 | <2026-09-08 Tue> |"), "{out}");
    assert!(out.contains("|  1.5 | [2026-02-28 Sat] |"), "{out}");

    let parts = "| <2026-09-29 Tue 14:30> |   |   |   |   |\n#+TBLFM: $2=year($1)::$3=month($1)::$4=weekday($1)::$5=hour($1)\n";
    assert!(
        recalc(parts).contains("| 2026 | 9 | 2 | 14 |"),
        "{}",
        recalc(parts)
    );
    let literal = "|   |\n#+TBLFM: $1=<2026-12-25>-<2026-12-24>\n";
    assert!(recalc(literal).starts_with("| 1 |"), "{}", recalc(literal));
    let repeating = "| <2026-09-29 Tue +1w> |   |\n#+TBLFM: $2=$1+1\n";
    assert!(
        recalc(repeating).contains("| <2026-09-30 Wed> |"),
        "{}",
        recalc(repeating)
    );
    let built = "|   |\n#+TBLFM: $1=date(2026, 2, 30)\n";
    assert!(
        recalc(built).starts_with("| <2026-03-02 Mon> |"),
        "{}",
        recalc(built)
    );
}

#[test]
fn durations_in_the_time_modes() {
    let text = "\
| Start | End      | Took | Hours |
|-------+----------+------+-------|
| 09:15 | 11:45:30 |      |       |
#+TBLFM: $3=$2-$1;T::$4=$2-$1;t
";
    let out = recalc(text);
    assert!(out.contains("| 02:30:30 |  2.51 |"), "{out}");
    let minutes = recalc("| 1:30 | 0:45 |   |\n#+TBLFM: $3=$1+$2;U\n");
    assert!(minutes.contains("| 02:15 |"), "{minutes}");
}

#[test]
fn units_are_kept_and_converted() {
    let text = "\
| Distance | Time   | Speed | In mph | Total  |
|----------+--------+-------+--------+--------|
| 12 km    | 30 min |       |        |        |
| 500 m    | 2 min  |       |        |        |
#+TBLFM: $3=$1/$2::$4=uconvert($1/$2, mph);%.1f::@>$5=vsum(@I$1..@>$1)
";
    let out = recalc(text);
    assert!(out.contains("| 0.4 km / min"), "{out}");
    assert!(out.contains("| 14.9 mph"), "{out}");
    assert!(out.contains("| 12.5 km |"), "{out}");
    let base = recalc("| 1 kWh |   |\n#+TBLFM: $2=ubase($1)\n");
    assert!(base.contains("| 3600000 m^2 kg / s^2 |"), "{base}");
    let mixed = recalc("| 1 m | 1 s |   |\n#+TBLFM: $3=$1+$2\n");
    assert!(mixed.contains("#ERROR"), "{mixed}");
}

#[test]
fn symbols_are_collected_and_derived() {
    let text = "\
| Expr      | Square | Derivative | At 2 |
|-----------+--------+------------+------|
| x + 1     |        |            |      |
| 2 a - a   |        |            |      |
#+TBLFM: $2=$1^2::$3=deriv($1^2, x)::$4=subst($1^2, x, 2)
";
    let out = recalc(text);
    assert!(out.contains("| x^2 + 2 x + 1 | 2 x + 2"), "{out}");
    assert!(out.contains("| 9    |"), "{out}");
    assert!(out.contains("| a^2           | 0 "), "{out}");
}

#[test]
fn conditions_and_comparisons() {
    let text = "\
| Score | Result | Both |
|-------+--------+------|
|    72 |        |      |
|    40 |        |      |
#+TBLFM: $2=if($1 >= 50, 1, 0)::$3=$1 > 30 && $1 < 60
";
    let out = recalc(text);
    assert!(out.contains("|    72 |      1 |    0 |"), "{out}");
    assert!(out.contains("|    40 |      0 |    1 |"), "{out}");
    let angle = recalc("| 30 |   |   |\n#+TBLFM: $2=sin($1)::$3=cos(pi);R\n");
    assert!(angle.contains("| 0.5 | -1 |"), "{angle}");
    let numbers = recalc("|   |   |\n|   |   |\n#+TBLFM: $1=@#::$2=$#\n");
    assert!(numbers.contains("| 1 | 2 |\n| 2 | 2 |"), "{numbers}");
}

#[test]
fn emacs_lisp_formulas_on_strings_and_numbers() {
    let text = "\
| First | Last  | Name | Initials | Sum |
|-------+-------+------+----------+-----|
| ada   | byron |      |          |     |
#+TBLFM: $3='(concat (capitalize $1) \" \" (upcase $2))::$4='(concat (substring $1 0 1) (substring $2 0 1))::$5='(+ 1 2 3);%.1f
";
    let out = recalc(text);
    assert!(out.contains("| Ada BYRON | ab       | 6.0 |"), "{out}");
    let numbers = recalc("| 2 | 3 | 4 |   |\n#+TBLFM: $4='(apply '+ '($1..$3));N\n");
    assert!(numbers.contains("| 9 |"), "{numbers}");
    let literal = recalc("| 2 | 3 |   |\n#+TBLFM: $3='(* $1 $2);L\n");
    assert!(literal.contains("| 6 |"), "{literal}");
    let bad = recalculate("| a |   |\n#+TBLFM: $2='(shell-command $1)\n", 0).unwrap();
    assert_eq!(bad.errors, 1);
}
