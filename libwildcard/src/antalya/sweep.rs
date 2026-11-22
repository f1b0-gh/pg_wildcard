// SPDX-License-Identifier: Apache-2.0
//! Bounded constant-regex sweeps. AST estimates concern the parsed AST;
//! ClickHouse's separate expanded-AST and execution limits still apply.
use super::prefilter::common_required_needles;
use super::{ChError, SweepItem, ViewColumns, candidate_predicate, ident, predicate, quote};
use crate::core::verify::Kind;

/// Ten percent below ClickHouse's default `max_ast_elements = 50000`.
pub const DEFAULT_SWEEP_MAX_AST_NODES: usize = 45_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SweepOptions {
    /// Maximum complete rendered query size, including diagnostic comments.
    pub max_bytes: usize,
    /// Maximum conservative parsed-AST estimate per query.
    pub max_ast_nodes: usize,
}

impl Default for SweepOptions {
    fn default() -> Self {
        Self {
            max_bytes: 200_000,
            max_ast_nodes: DEFAULT_SWEEP_MAX_AST_NODES,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SweepStrategy {
    /// Constant candidate predicates and exact constant-regex rechecks.
    /// Individual candidates may be omitted to fit the configured limits.
    ConstantCandidates,
}

impl SweepStrategy {
    pub fn name(self) -> &'static str {
        "constant_candidates"
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SweepPlan {
    pub queries: Vec<String>,
    pub strategy: SweepStrategy,
    /// Number of generated source queries; each contains one source reference.
    /// Physical reads remain an engine-plan property, especially for source views.
    pub source_scan_count: usize,
    /// Conservative parsed-AST estimate for each corresponding query.
    pub estimated_ast: Vec<usize>,
    /// Items with no candidate, including unsupported plans and budget widening.
    pub exact_only_items: usize,
}

/// Compatibility wrapper using the default AST budget. An empty sweep emits
/// no queries, even with a zero byte limit, preserving the original contract.
pub fn sweep_queries(
    source: &str,
    select: &[&str],
    cols: &ViewColumns,
    items: &[SweepItem],
    filter: Option<&str>,
    max_bytes: usize,
) -> Result<Vec<String>, ChError> {
    if items.is_empty() {
        ident(source)?;
        for column in select {
            ident(column)?;
        }
        return Ok(Vec::new());
    }
    sweep_queries_with_options(
        source,
        select,
        cols,
        items,
        filter,
        SweepOptions {
            // A one-byte query cannot fit; defer the legacy zero-budget error
            // until identifiers and the first pattern have been validated.
            max_bytes: max_bytes.max(1),
            ..SweepOptions::default()
        },
    )
    .map(|plan| plan.queries)
    .map_err(|error| match error {
        ChError::QueryTooLarge(_) if max_bytes == 0 => ChError::QueryTooLarge(0),
        other => other,
    })
}

/// Batch by actual escaped SQL bytes and conservative parsed-AST costs, without
/// a fixed indicator count. Exact `match()` rechecks are always retained.
/// Candidates that cannot fit even in an empty batch widen to exact matching.
///
/// `source` and `select` must be identifiers. `filter` and expressions assigned
/// directly to public `ViewColumns` fields are trusted SQL, conservatively
/// charged four nodes per byte (including each repeated expression).
/// This byte allowance includes complex filters, rather than interpreting a
/// partial SQL grammar. AST accounting for generated expressions is
/// iterative and includes every repeated transformed-value expression. This is
/// a parsed-AST guard, not a promise about expanded ASTs or query execution cost.
/// AND short-circuiting depends on engine settings. There is no automatic switch
/// to a dynamic watchlist, whose per-row costs can be much greater.
pub fn sweep_queries_with_options(
    source: &str,
    select: &[&str],
    cols: &ViewColumns,
    items: &[SweepItem],
    filter: Option<&str>,
    options: SweepOptions,
) -> Result<SweepPlan, ChError> {
    if options.max_bytes == 0 || options.max_ast_nodes == 0 {
        return Err(ChError::InvalidSweepOptions);
    }
    let source = ident(source)?;
    let select: Vec<String> = select.iter().map(|c| ident(c)).collect::<Result<_, _>>()?;
    let filter_cost = match filter {
        Some(sql) => sql.len().saturating_mul(4),
        None => 0,
    };
    // Covers SELECT/FROM, output aliases, ARRAY JOIN, both lambdas, tuple
    // projections, and outer expression lists, including empty-array overhead.
    let base_ast = 128usize
        .saturating_add(select.len().saturating_mul(2))
        .saturating_add(source.len())
        .saturating_add(filter_cost);
    let base_bytes = sweep_sql(&source, &select, &[], filter).len();
    let mut plan = SweepPlan {
        queries: Vec::new(),
        strategy: SweepStrategy::ConstantCandidates,
        source_scan_count: 0,
        estimated_ast: Vec::new(),
        exact_only_items: 0,
    };
    let mut batch = Vec::new();
    let mut bytes = base_bytes;
    let mut ast = base_ast;
    for item in items {
        // Constructors validate columns, but the public fields historically
        // also accepted trusted expressions. Preserve that contract with a
        // conservative byte-based allowance for the whole generated item.
        let trusted_expression =
            ident(cols.materialized.get(&item.view).unwrap_or(&cols.raw)).is_err();
        let value = cols.expr(item.view);
        let kind = Kind::Regex {
            case_insensitive: false,
        };
        let exact = predicate(&value, kind, &item.regex, item.view)?;
        let candidate = candidate_predicate(&value, kind, &item.regex, item.view)?;
        let render = |condition: &str| {
            format!(
                "(toInt64({}), '{}', ({condition}))",
                item.id,
                item.view.name()
            )
        };
        let mut expression = match &candidate {
            Some(candidate) => render(&format!("({candidate}) AND ({exact})")),
            None => render(&exact),
        };
        let estimate = |sql: &str| {
            if trusted_expression {
                Ok(sql.len().saturating_mul(4))
            } else {
                expression_ast(sql)
            }
        };
        let mut cost = estimate(&expression)?;
        let mut exact_only = candidate.is_none();
        if base_bytes.saturating_add(expression.len()) > options.max_bytes
            || base_ast.saturating_add(cost) > options.max_ast_nodes
        {
            expression = render(&exact);
            cost = estimate(&expression)?;
            exact_only = true;
        }
        if base_bytes.saturating_add(expression.len()) > options.max_bytes {
            return Err(ChError::QueryTooLarge(options.max_bytes));
        }
        if base_ast.saturating_add(cost) > options.max_ast_nodes {
            return Err(ChError::SweepAstTooLarge(options.max_ast_nodes));
        }
        if !batch.is_empty()
            && (bytes.saturating_add(expression.len()).saturating_add(2) > options.max_bytes
                || ast.saturating_add(cost) > options.max_ast_nodes)
        {
            plan.queries
                .push(sweep_sql(&source, &select, &batch, filter));
            plan.estimated_ast.push(ast);
            batch.clear();
            bytes = base_bytes;
            ast = base_ast;
        }
        bytes += expression.len() + if batch.is_empty() { 0 } else { 2 };
        ast += cost;
        batch.push(expression);
        plan.exact_only_items += usize::from(exact_only);
    }
    if !batch.is_empty() {
        plan.queries
            .push(sweep_sql(&source, &select, &batch, filter));
        plan.estimated_ast.push(ast);
    }
    plan.source_scan_count = plan.queries.len();
    let diagnostic = format!("source_scan_count={:<20}", plan.source_scan_count);
    for sql in &mut plan.queries {
        *sql = sql.replacen("source_scan_count=1                   ", &diagnostic, 1);
    }
    add_common_gate(&mut plan, items, cols, filter, options);
    debug_assert!(
        plan.queries
            .iter()
            .all(|sql| sql.len() <= options.max_bytes)
    );
    debug_assert!(
        plan.estimated_ast
            .iter()
            .all(|n| *n <= options.max_ast_nodes)
    );
    Ok(plan)
}

/// Modify only fitting queries in an already validated dense plan. Keeps
/// query count, ordering, item expressions and exact rechecks unchanged.
fn add_common_gate(
    plan: &mut SweepPlan,
    items: &[SweepItem],
    cols: &ViewColumns,
    filter: Option<&str>,
    options: SweepOptions,
) -> (Vec<String>, usize) {
    let Some(first) = items.first() else {
        return (Vec::new(), 0);
    };
    let column = cols.materialized.get(&first.view).unwrap_or(&cols.raw);
    if ident(column).is_err() || matches!(column.as_str(), "view" | "indicator_id" | "wc_hit") {
        return (Vec::new(), 0);
    }
    let Some((view, needles)) = common_required_needles(items) else {
        return (Vec::new(), 0);
    };
    let value = cols.expr(view);
    let gate = needles
        .iter()
        .map(|n| format!("position({value}, {}) > 0", quote(n)))
        .collect::<Vec<_>>()
        .join(" AND ");
    let old_suffix = filter.map(|f| format!("\nWHERE {f}")).unwrap_or_default();
    // A newline before the closing parenthesis preserves trailing SQL comments
    // in a trusted filter without swallowing added syntax.
    let new_suffix = match filter {
        Some(f) => format!("\nWHERE ({gate}) AND (\n{f}\n)"),
        None => format!("\nWHERE {gate}"),
    };
    // Deliberately overcharge the complete new suffix, including the existing
    // filter already charged by the dense budgeter. Optional gate may widen.
    let extra_ast = new_suffix.len().saturating_mul(4);
    let mut applied = 0;
    for (sql, estimate) in plan.queries.iter_mut().zip(&mut plan.estimated_ast) {
        let Some(base) = sql.strip_suffix(old_suffix.as_str()) else {
            continue;
        };
        let bytes = base.len().saturating_add(new_suffix.len());
        let ast = estimate.saturating_add(extra_ast);
        if bytes > options.max_bytes || ast > options.max_ast_nodes {
            continue;
        }
        *sql = format!("{base}{new_suffix}");
        *estimate = ast;
        applied += 1;
    }
    (needles, applied)
}

pub(super) fn sweep_sql(
    source: &str,
    select: &[String],
    expressions: &[String],
    filter: Option<&str>,
) -> String {
    let mut sql = format!(
        "/* wildcard sweep: strategy=constant_candidates source_scans=1 source_scan_count=1                    */\nSELECT {}{}tupleElement(wc_hit, 1) AS indicator_id, tupleElement(wc_hit, 2) AS view\nFROM {source}\nARRAY JOIN arrayMap(x -> (tupleElement(x, 1), tupleElement(x, 2)), arrayFilter(x -> tupleElement(x, 3), [{}])) AS wc_hit",
        select.join(", "),
        if select.is_empty() { "" } else { ", " },
        expressions.join(", ")
    );
    if let Some(f) = filter {
        sql.push_str(&format!("\nWHERE {f}"));
    }
    sql
}

// The generated expression grammar is functions, literal tuples/arrays, scalar
// identifiers, and operators. A function costs two nodes (function + argument
// list); each identifier/literal costs one. Tuple/group and array openers cost
// two, even when grouping creates no nodes. A same-level AND/OR chain is one
// variadic function with one argument list; changing operator starts another.
// Unary signs are overcounted. The iterative delimiter stack has a hard cap.
fn expression_ast(sql: &str) -> Result<usize, ChError> {
    let bytes = sql.as_bytes();
    let mut pos = 0;
    let mut cost = 0usize;
    let mut levels = vec![0u8];
    while pos < bytes.len() {
        let b = bytes[pos];
        if b.is_ascii_whitespace() || b == b',' {
            if b == b',' {
                *levels.last_mut().ok_or(ChError::UnbudgetableSweepSql)? = 0;
            }
            pos += 1;
            continue;
        }
        if matches!(b, b')' | b']') {
            if levels.len() <= 1 {
                return Err(ChError::UnbudgetableSweepSql);
            }
            levels.pop();
            pos += 1;
            continue;
        }
        if b == b'\'' {
            pos = quoted_end(bytes, pos)?;
            cost = cost.saturating_add(1);
        } else if b.is_ascii_alphabetic() || b == b'_' {
            let start = pos;
            while pos < bytes.len()
                && (bytes[pos].is_ascii_alphanumeric() || matches!(bytes[pos], b'_' | b'.'))
            {
                pos += 1;
            }
            let word = &sql[start..pos];
            while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
                pos += 1;
            }
            if matches!(word, "AND" | "OR") {
                let operator = if word == "AND" { 1 } else { 2 };
                let level = levels.last_mut().ok_or(ChError::UnbudgetableSweepSql)?;
                if *level != operator {
                    cost = cost.saturating_add(2);
                }
                *level = operator;
            } else if bytes.get(pos) == Some(&b'(') {
                cost = cost.saturating_add(2);
                pos += 1;
                levels.push(0);
            } else {
                cost = cost.saturating_add(if word == "NOT" { 2 } else { 1 });
            }
        } else if b.is_ascii_digit() {
            while pos < bytes.len() && bytes[pos].is_ascii_digit() {
                pos += 1;
            }
            cost = cost.saturating_add(1);
        } else if matches!(
            b,
            b'(' | b'[' | b'+' | b'-' | b'*' | b'/' | b'=' | b'>' | b'<'
        ) {
            cost = cost.saturating_add(2);
            if matches!(b, b'(' | b'[') {
                levels.push(0);
            }
            pos += 1;
        } else {
            return Err(ChError::UnbudgetableSweepSql);
        }
        if levels.len() > 256 {
            return Err(ChError::UnbudgetableSweepSql);
        }
    }
    if levels.len() != 1 {
        return Err(ChError::UnbudgetableSweepSql);
    }
    Ok(cost)
}

fn quoted_end(bytes: &[u8], mut pos: usize) -> Result<usize, ChError> {
    let quote = bytes[pos];
    pos += 1;
    while pos < bytes.len() {
        if bytes[pos] == b'\\' {
            pos = pos.saturating_add(2);
        } else if bytes[pos] == quote {
            pos += 1;
            if bytes.get(pos) == Some(&quote) {
                pos += 1;
            } else {
                return Ok(pos);
            }
        } else {
            pos += 1;
        }
    }
    Err(ChError::UnbudgetableSweepSql)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::View;

    fn item(view: View) -> SweepItem {
        SweepItem {
            id: -1,
            view,
            regex: "abc.*def".into(),
        }
    }

    fn plan(items: &[SweepItem], options: SweepOptions) -> Result<SweepPlan, ChError> {
        sweep_queries_with_options(
            "logs",
            &["id"],
            &ViewColumns::new("v").unwrap(),
            items,
            None,
            options,
        )
    }

    fn items(patterns: &[&str], view: View) -> Vec<SweepItem> {
        patterns
            .iter()
            .enumerate()
            .map(|(id, p)| SweepItem {
                id: id as i64,
                view,
                regex: (*p).into(),
            })
            .collect()
    }
    fn ungated(items: &[SweepItem], cols: &ViewColumns, filter: Option<&str>) -> SweepPlan {
        let expressions: Vec<_> = items
            .iter()
            .map(|item| {
                let value = cols.expr(item.view);
                let kind = Kind::Regex {
                    case_insensitive: false,
                };
                let exact = predicate(&value, kind, &item.regex, item.view).unwrap();
                let condition = candidate_predicate(&value, kind, &item.regex, item.view)
                    .unwrap()
                    .map_or_else(
                        || exact.clone(),
                        |candidate| format!("({candidate}) AND ({exact})"),
                    );
                format!(
                    "(toInt64({}), '{}', ({condition}))",
                    item.id,
                    item.view.name()
                )
            })
            .collect();
        let cost = 128
            + 2
            + "logs".len()
            + filter.map_or(0, |f| f.len() * 4)
            + expressions
                .iter()
                .map(|e| expression_ast(e).unwrap())
                .sum::<usize>();
        SweepPlan {
            queries: vec![sweep_sql("logs", &["id".into()], &expressions, filter)],
            strategy: SweepStrategy::ConstantCandidates,
            source_scan_count: 1,
            estimated_ast: vec![cost],
            exact_only_items: 0,
        }
    }
    #[test]
    fn quoting_and_budget_fallback_preserve_original_plan() {
        let list = items(&["a'b"], View::Raw);
        let cols = ViewColumns::new("v").unwrap();
        let options = SweepOptions::default();
        let original = ungated(&list, &cols, None);
        for tight in [
            SweepOptions {
                max_bytes: original.queries[0].len(),
                ..options
            },
            SweepOptions {
                max_ast_nodes: original.estimated_ast[0],
                ..options
            },
        ] {
            let mut plan = original.clone();
            assert_eq!(add_common_gate(&mut plan, &list, &cols, None, tight).1, 0);
            assert_eq!(plan, original);
        }
        let mut plan = original.clone();
        assert_eq!(add_common_gate(&mut plan, &list, &cols, None, options).1, 1);
        assert!(plan.queries[0].contains(&format!("position(v, {}) > 0", quote("a'b"))));
        assert_eq!(plan.source_scan_count, original.source_scan_count);
    }
    #[test]
    fn user_filter_remains_parenthesized_and_exact_checks_remain() {
        let list = items(&["abc123", "abc456"], View::Raw);
        let cols = ViewColumns::new("v").unwrap();
        let filter = Some("id=0 OR id=1 -- trailing comment");
        let options = SweepOptions::default();
        let mut plan = ungated(&list, &cols, filter);
        assert_eq!(
            add_common_gate(&mut plan, &list, &cols, filter, options).1,
            1
        );
        assert!(plan.queries[0].contains("AND (\nid=0 OR id=1 -- trailing comment\n)"));
        for p in ["abc123", "abc456"] {
            assert!(plan.queries[0].contains(&format!("match(v, {})", quote(p))));
        }
    }
    #[test]
    fn alias_sensitive_or_trusted_expressions_disable_gate() {
        let list = items(&["abc.*x", "abc.*y"], View::Raw);
        let options = SweepOptions::default();
        for column in ["view", "indicator_id", "wc_hit", "if(id=0, 'abcx', 'abcy')"] {
            let mut cols = ViewColumns::new("v").unwrap();
            cols.raw = column.into();
            let original = ungated(&list, &cols, None);
            let mut plan = original.clone();
            assert_eq!(add_common_gate(&mut plan, &list, &cols, None, options).1, 0);
            assert_eq!(plan, original);
        }
    }
    #[test]
    fn explicit_ast_budget_splits_and_reports_every_query() {
        let atom = SweepItem {
            regex: "abc|def".into(),
            ..item(View::Raw)
        };
        let single = plan(std::slice::from_ref(&atom), SweepOptions::default()).unwrap();
        let options = SweepOptions {
            max_ast_nodes: single.estimated_ast[0],
            ..SweepOptions::default()
        };
        let many = plan(&vec![atom; 300], options).unwrap();
        assert_eq!(many.source_scan_count, 300);
        assert_eq!(many.estimated_ast, vec![single.estimated_ast[0]; 300]);
        assert!(
            many.queries
                .iter()
                .all(|q| q.contains("source_scan_count=300 "))
        );
        assert!(
            many.queries
                .iter()
                .all(|q| q.contains("match(v, 'abc|def')"))
        );
        assert_eq!(many.exact_only_items, 0);
    }

    #[test]
    fn optional_candidates_widen_for_ast_budget() {
        let exact_expression = "(toInt64(-1), 'raw', (match(v, 'abc.*def')))";
        let exact_cost = 128 + 2 + "logs".len() + expression_ast(exact_expression).unwrap();
        let options = SweepOptions {
            max_ast_nodes: exact_cost,
            ..SweepOptions::default()
        };
        let actual = plan(&[item(View::Raw)], options).unwrap();
        assert_eq!(actual.estimated_ast, vec![exact_cost]);
        assert_eq!(actual.exact_only_items, 1);
        assert!(!actual.queries[0].contains("like("));
        assert!(actual.queries[0].contains("match(v, 'abc.*def')"));
        assert!(matches!(
            plan(
                &[item(View::Raw)],
                SweepOptions {
                    max_ast_nodes: exact_cost - 1,
                    ..options
                }
            ),
            Err(ChError::SweepAstTooLarge(_))
        ));
    }

    #[test]
    fn complex_views_are_counted_at_every_occurrence() {
        let raw = plan(
            &[SweepItem {
                regex: "abc|def".into(),
                ..item(View::Raw)
            }],
            SweepOptions::default(),
        )
        .unwrap();
        let transformed = plan(
            &[SweepItem {
                regex: "abc|def".into(),
                ..item(View::RevDomain)
            }],
            SweepOptions::default(),
        )
        .unwrap();
        assert!(transformed.estimated_ast[0] > raw.estimated_ast[0] * 3);
        let options = SweepOptions {
            max_ast_nodes: transformed.estimated_ast[0] - 1,
            ..SweepOptions::default()
        };
        let widened = plan(
            &[SweepItem {
                regex: "abc|def".into(),
                ..item(View::RevDomain)
            }],
            options,
        )
        .unwrap();
        assert_eq!(widened.exact_only_items, 1);
        assert!(widened.estimated_ast[0] < transformed.estimated_ast[0]);
        let materialized = sweep_queries_with_options(
            "logs",
            &["id"],
            &ViewColumns::conventional("v").unwrap(),
            &[SweepItem {
                regex: "abc|def".into(),
                ..item(View::RevDomain)
            }],
            None,
            SweepOptions::default(),
        )
        .unwrap();
        assert_eq!(materialized.estimated_ast, raw.estimated_ast);
    }

    #[test]
    fn legacy_zero_byte_budget_preserves_errors_and_empty_sweeps() {
        let cols = ViewColumns::new("v").unwrap();
        assert_eq!(
            sweep_queries("logs", &["id"], &cols, &[], None, 0),
            Ok(vec![])
        );
        assert_eq!(
            sweep_queries("logs", &["id"], &cols, &[item(View::Raw)], None, 0),
            Err(ChError::QueryTooLarge(0))
        );
        let invalid = SweepItem {
            regex: "(?=x)".into(),
            ..item(View::Raw)
        };
        assert_eq!(
            sweep_queries(
                "logs",
                &["id"],
                &cols,
                std::slice::from_ref(&invalid),
                None,
                0
            ),
            sweep_queries("logs", &["id"], &cols, &[invalid], None, 200_000)
        );
    }

    #[test]
    fn zero_options_are_invalid_even_for_empty_new_api() {
        for options in [
            SweepOptions {
                max_bytes: 0,
                ..SweepOptions::default()
            },
            SweepOptions {
                max_ast_nodes: 0,
                ..SweepOptions::default()
            },
        ] {
            assert_eq!(plan(&[], options), Err(ChError::InvalidSweepOptions));
        }
        assert_eq!(
            plan(&[], SweepOptions::default())
                .unwrap()
                .source_scan_count,
            0
        );
    }

    #[test]
    fn escaped_literals_are_one_node_and_do_not_hide_expressions() {
        assert_eq!(expression_ast("match(v, 'a\\\' OR x')").unwrap(), 4);
        assert_eq!(
            expression_ast("match(v, 'a\\\\') AND match(v, 'b')").unwrap(),
            10
        );
        assert!(expression_ast("match(v, 'unfinished)").is_err());
        // Counting remains iterative on hostile nesting; the backend may reject
        // this syntax independently, but the budgeter cannot overflow its stack.
        assert!(expression_ast(&"(".repeat(100_000)).is_err());
    }

    #[test]
    fn large_regex_without_candidate_still_has_a_small_ast() {
        let items = [SweepItem {
            regex: "x".repeat(9000),
            ..item(View::Raw)
        }];
        let actual = plan(&items, SweepOptions::default()).unwrap();
        assert_eq!(actual.exact_only_items, 1);
        assert!(actual.estimated_ast[0] < 200);
        assert!(actual.queries[0].len() > 9000);
    }

    #[test]
    fn trusted_filters_and_selected_columns_are_charged() {
        let cols = ViewColumns::new("v").unwrap();
        let items = [SweepItem {
            regex: "abc|def".into(),
            ..item(View::Raw)
        }];
        let plain = plan(&items, SweepOptions::default()).unwrap();
        let filter = "ts > now() - INTERVAL 7 DAY AND id IN (1, 2, 3)";
        let filtered = sweep_queries_with_options(
            "logs",
            &["id", "ts"],
            &cols,
            &items,
            Some(filter),
            SweepOptions::default(),
        )
        .unwrap();
        assert_eq!(
            filtered.estimated_ast[0],
            plain.estimated_ast[0] + 2 + filter.len() * 4
        );
        let complex = "id IN (SELECT id FROM x WHERE a = '/* literal */') /* comment */";
        let filtered = sweep_queries_with_options(
            "logs",
            &["id"],
            &cols,
            &items,
            Some(complex),
            SweepOptions::default(),
        )
        .unwrap();
        assert_eq!(
            filtered.estimated_ast[0],
            plain.estimated_ast[0] + complex.len() * 4
        );
        let huge = format!("id IN ({})", "1,".repeat(20_000));
        assert!(matches!(
            sweep_queries_with_options(
                "logs",
                &["id"],
                &cols,
                &items,
                Some(&huge),
                SweepOptions::default()
            ),
            Err(ChError::SweepAstTooLarge(_))
        ));
    }

    #[test]
    fn public_view_columns_retain_trusted_expression_support() {
        let mut cols = ViewColumns::new("v").unwrap();
        cols.raw = "if(length(v) > 10, substring(v, 1, 10), v)".into();
        let actual = sweep_queries_with_options(
            "logs",
            &["id"],
            &cols,
            &[item(View::Decoded)],
            None,
            SweepOptions::default(),
        )
        .unwrap();
        assert!(actual.queries[0].contains("lower(decodeURLComponent(if(length(v)"));
        assert!(actual.estimated_ast[0] > 1000);
        assert_eq!(
            sweep_queries(
                "logs",
                &["id"],
                &cols,
                &[item(View::Decoded)],
                None,
                200_000
            )
            .unwrap(),
            actual.queries
        );
        let too_tight = SweepOptions {
            max_ast_nodes: 200,
            ..SweepOptions::default()
        };
        assert!(matches!(
            sweep_queries_with_options(
                "logs",
                &["id"],
                &cols,
                &[item(View::Decoded)],
                None,
                too_tight
            ),
            Err(ChError::SweepAstTooLarge(_))
        ));
    }
}
