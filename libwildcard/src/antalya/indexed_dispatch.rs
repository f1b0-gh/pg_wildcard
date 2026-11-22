// SPDX-License-Identifier: Apache-2.0
//! Opt-in bounded byte-index filtering and rule dispatch.
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

use super::{
    ChError, GramFileCounts, SweepItem, SweepOptions, SweepPlan, SweepStrategy, ViewColumns, ident,
    predicate, quote, sweep::expression_ast, sweep_queries_with_options,
};
use crate::core::{View, grams, verify::Kind};

const MAX_RULES: usize = 2000;
const MAX_GROUPS: usize = 255;
const MAX_MEMBERS: usize = 64;
const MAX_UNASSIGNED: usize = 256;
const OVERFLOW: &str = "_wildcard_overflow_";
const OVERFLOW_TOKEN: &str = "_wildc";

/// Why the opt-in request used the existing constant-candidate sweep instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ByteDispatchFallback {
    /// Empty or more than 2000 rules.
    RuleCount,
    /// Only raw-view rules over an identifier are currently supported.
    ViewOrExpression,
    /// Too few rules could be assigned to bounded selective fragment groups.
    Coverage,
    /// The complete dispatch query exceeded its SQL or parsed-AST budget.
    QueryBudget,
}

/// Source-gate strength; all variants retain the exact per-rule recheck.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ByteDispatchGate {
    /// A rule has no proven required fragment, or dispatch fell back entirely.
    Disabled,
    /// Up to three required fragments per rule, OR across rules and overflow.
    RequiredFragments,
    /// The conjunction gate exceeded a query budget; one fragment per rule fits.
    SingleFragmentBudget,
}

/// Dispatch diagnostics; physical index use must be checked on the target engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ByteDispatchPlan {
    pub sweep: SweepPlan,
    pub dispatch_groups: usize,
    pub unassigned_items: usize,
    pub index_gate: bool,
    pub gate_strategy: ByteDispatchGate,
    pub fallback: Option<ByteDispatchFallback>,
}

// Hex is ASCII even for invalid UTF-8. Each aligned six-character window is a
// byte trigram; half-byte windows add candidates only. No array lambda captures
// the original String (ClickHouse can replicate it once per array element).
// At most 16398 windows. Long values always pass via the non-hex overflow token.
fn byte_tokens(value: &str) -> String {
    format!(
        "ngrams(concat(hex(substring({value}, 1, 8192)), if(length({value}) > 8192, {}, '')), 6)",
        quote(OVERFLOW)
    )
}

/// Explicit opt-in text index on bounded hex-encoded byte fragments.
/// Existing parts need a separate `MATERIALIZE INDEX` operation. This can add
/// substantial write/storage costs; measure against the intended indexed system.
/// The index is optional for correctness and does not change the source value.
pub fn byte_gram_index_ddl(table: &str, index: &str, column: &str) -> Result<String, ChError> {
    let (table, index, column) = (ident(table)?, ident(index)?, ident(column)?);
    Ok(format!(
        "ALTER TABLE {table} ADD INDEX {index} {} TYPE text(tokenizer='array') GRANULARITY 1",
        byte_tokens(&column)
    ))
}

struct Dispatch {
    groups: Vec<(String, Vec<usize>)>,
    unassigned: Vec<usize>,
}

fn helper_prefix(
    source: &str,
    select: &[&str],
    cols: &ViewColumns,
    filter: Option<&str>,
) -> Result<String, ChError> {
    (0..128)
        .map(|n| format!("_wc_dispatch{n}_"))
        .find(|p| {
            !source.contains(p)
                && !cols.raw.contains(p)
                && !cols.materialized.values().any(|v| v.contains(p))
                && !select.iter().any(|s| s.contains(p))
                && !filter.is_some_and(|s| s.contains(p))
        })
        .ok_or(ChError::UnbudgetableSweepSql)
}

// Isolate source expressions from the default renderer's fixed output aliases.
fn dense_fallback(
    source: &str,
    select: &[&str],
    cols: &ViewColumns,
    items: &[SweepItem],
    filter: Option<&str>,
    options: SweepOptions,
) -> Result<SweepPlan, ChError> {
    if items.is_empty() {
        return sweep_queries_with_options(source, select, cols, items, filter, options);
    }
    let source = ident(source)?;
    let selected = select
        .iter()
        .map(|s| ident(s))
        .collect::<Result<Vec<_>, _>>()?;
    let prefix = helper_prefix(&source, select, cols, filter)?;
    let input = format!("{prefix}input");
    let row = format!("{prefix}row");
    let mut bound = ViewColumns::new(&format!("{prefix}value"))?;
    let mut expressions = Vec::new();
    for view in items.iter().map(|i| i.view).collect::<BTreeSet<_>>() {
        let column = format!("{prefix}{}", view.name());
        expressions.push(format!("{} AS {column}", cols.expr(view)));
        bound.materialized.insert(view, column);
    }
    let projection = if selected.is_empty() {
        "1".into()
    } else {
        selected.join(", ")
    };
    let filter = filter
        .map(|f| format!("\nWHERE (\n{f}\n)"))
        .unwrap_or_default();
    let cte = format!(
        "WITH {input} AS (SELECT tuple({projection}) AS {row}, {}\nFROM {source}{filter}\n)\n",
        expressions.join(", ")
    );
    let output = selected
        .iter()
        .enumerate()
        .map(|(i, col)| {
            format!(
                "tupleElement({row}, {}) AS {}, ",
                i + 1,
                col.rsplit('.').next().unwrap_or(col)
            )
        })
        .collect::<String>();
    let before = format!("{cte}SELECT {output}indicator_id, view\nFROM (\n");
    let after = "\n)";
    let bytes = before.len().saturating_add(after.len());
    // Overcharge the trusted wrapper, without interpreting a partial SQL grammar.
    let ast = bytes.saturating_mul(4);
    if bytes >= options.max_bytes {
        return Err(ChError::QueryTooLarge(options.max_bytes));
    }
    if ast >= options.max_ast_nodes {
        return Err(ChError::SweepAstTooLarge(options.max_ast_nodes));
    }
    let mut plan = sweep_queries_with_options(
        &input,
        &[&row],
        &bound,
        items,
        None,
        SweepOptions {
            max_bytes: options.max_bytes - bytes,
            max_ast_nodes: options.max_ast_nodes - ast,
        },
    )
    .map_err(|e| match e {
        ChError::QueryTooLarge(_) => ChError::QueryTooLarge(options.max_bytes),
        ChError::SweepAstTooLarge(_) => ChError::SweepAstTooLarge(options.max_ast_nodes),
        other => other,
    })?;
    for (query, estimate) in plan.queries.iter_mut().zip(&mut plan.estimated_ast) {
        *query = format!("{before}{query}{after}");
        *estimate = estimate.saturating_add(ast);
    }
    Ok(plan)
}

// Lazy maximum-coverage heap: stale counts are upper bounds, recomputed when
// popped. Each posting has <=64 members; limits bound all planner work.
fn dispatch(needles: &[Vec<String>]) -> Option<Dispatch> {
    let mut postings: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, grams) in needles.iter().enumerate() {
        for gram in grams {
            postings.entry(gram.clone()).or_default().push(i + 1);
        }
    }
    let mut heap = BinaryHeap::new();
    for (gram, members) in &postings {
        if members.len() <= MAX_MEMBERS {
            heap.push((members.len(), Reverse(gram.clone())));
        }
    }
    let mut remaining = vec![true; needles.len()];
    let mut groups = Vec::new();
    while groups.len() < MAX_GROUPS {
        let Some((bound, Reverse(gram))) = heap.pop() else {
            break;
        };
        let members: Vec<_> = postings[&gram]
            .iter()
            .copied()
            .filter(|i| remaining[i - 1])
            .collect();
        if members.is_empty() {
            continue;
        }
        if members.len() != bound {
            heap.push((members.len(), Reverse(gram)));
            continue;
        }
        for i in &members {
            remaining[i - 1] = false;
        }
        groups.push((gram, members));
    }
    let unassigned: Vec<_> = remaining
        .iter()
        .enumerate()
        .filter_map(|(i, remains)| remains.then_some(i + 1))
        .collect();
    if groups.is_empty()
        || unassigned.len() > MAX_UNASSIGNED
        || unassigned.len() * 4 > needles.len()
    {
        return None;
    }
    Some(Dispatch { groups, unassigned })
}

/// Opt-in one-source-query sweep using necessary byte trigrams for dispatch.
///
/// Raw-view rules only, <=2000 rules, <=255 groups of <=64 rules. Unsupported or
/// over-budget plans fall back explicitly to `sweep_queries_with_options`.
/// Every dispatched rule retains its exact RE2 check. Duplicate IDs/rules keep
/// their multiplicity. No token-index metadata is used to establish semantics.
///
/// An optional gate requires up to three proven fragments per rule (OR across
/// rules), using the bounded expression from `byte_gram_index_ddl`. If this gate
/// exceeds a query budget, it widens to one fragment per rule before dense fallback.
/// Long values pass via an overflow sentinel. Any rule without a required gram
/// disables this gate; that rule still reaches exact matching. Optional file
/// counts rank proven grams only, and may safely be absent or stale.
/// Source/select identifiers and trusted SQL filters follow the ordinary sweep
/// API. Output aliases are isolated from source filters and column expressions.
#[allow(clippy::too_many_arguments)]
pub fn sweep_byte_dispatch(
    source: &str,
    select: &[&str],
    cols: &ViewColumns,
    items: &[SweepItem],
    filter: Option<&str>,
    options: SweepOptions,
    frequencies: Option<&GramFileCounts>,
) -> Result<ByteDispatchPlan, ChError> {
    let fallback = |reason| {
        Ok(ByteDispatchPlan {
            sweep: dense_fallback(source, select, cols, items, filter, options)?,
            dispatch_groups: 0,
            unassigned_items: items.len(),
            index_gate: false,
            gate_strategy: ByteDispatchGate::Disabled,
            fallback: Some(reason),
        })
    };
    if options.max_bytes == 0 || options.max_ast_nodes == 0 {
        return Err(ChError::InvalidSweepOptions);
    }
    let source = ident(source)?;
    let selected = select
        .iter()
        .map(|s| ident(s))
        .collect::<Result<Vec<_>, _>>()?;
    if items.is_empty() || items.len() > MAX_RULES {
        return fallback(ByteDispatchFallback::RuleCount);
    }
    let value = cols.expr(View::Raw);
    if items.iter().any(|item| item.view != View::Raw) || ident(&value).is_err() {
        return fallback(ByteDispatchFallback::ViewOrExpression);
    }
    if items
        .iter()
        .fold(0usize, |n, item| n.saturating_add(item.regex.len()))
        > options.max_bytes
    {
        return fallback(ByteDispatchFallback::QueryBudget);
    }
    let mut needles = Vec::with_capacity(items.len());
    let mut gate_keys = BTreeSet::new();
    let mut conjunctions = BTreeSet::new();
    let mut index_gate = true;
    for item in items {
        let _ = predicate(
            &value,
            Kind::Regex {
                case_insensitive: false,
            },
            &item.regex,
            item.view,
        )?;
        let required = super::prefilter::required_grams(&item.regex, item.view).unwrap_or_default();
        let ranked = super::prefilter::select_needles(required.clone(), frequencies);
        if let Some(needle) = ranked.first() {
            gate_keys.insert(needle.clone());
            let mut keys = ranked;
            keys.sort();
            conjunctions.insert(keys);
        } else {
            index_gate = false;
        }
        needles.push(
            required
                .into_iter()
                .map(|g| {
                    grams::unpack(g)
                        .1
                        .iter()
                        .map(|s| char::from(*s as u8))
                        .collect()
                })
                .collect(),
        );
    }
    let Some(dispatch) = dispatch(&needles) else {
        return fallback(ByteDispatchFallback::Coverage);
    };
    let prefix = helper_prefix(&source, select, cols, filter)?;
    let val = format!("{prefix}value");
    let row = format!("{prefix}row");
    let idx = format!("{prefix}idx");
    let pat = format!("{prefix}patterns");
    let ids = format!("{prefix}ids");
    let lambda = format!("{prefix}i");
    let pos = format!("{prefix}pos");
    let group = format!("{prefix}group");
    let keys = format!(
        "[{}]",
        dispatch
            .groups
            .iter()
            .map(|(g, _)| quote(g))
            .collect::<Vec<_>>()
            .join(",")
    );
    let members = format!(
        "[{}]",
        dispatch
            .groups
            .iter()
            .map(|(_, ids)| {
                format!(
                    "[{}]",
                    ids.iter()
                        .map(|i| format!("toUInt64({i})"))
                        .collect::<Vec<_>>()
                        .join(",")
                )
            })
            .collect::<Vec<_>>()
            .join(",")
    );
    let mut candidates = format!(
        "arrayFlatten(arrayMap(({pos}, {group}) -> if({pos} > 0, {group}, CAST([], 'Array(UInt64)')), multiSearchAllPositions({val}, {keys}), {members}))"
    );
    if !dispatch.unassigned.is_empty() {
        let leftovers = dispatch
            .unassigned
            .iter()
            .map(|i| {
                let needle = needles[i - 1].first().map_or("", String::as_str);
                format!("(toUInt64({i}), {})", quote(needle))
            })
            .collect::<Vec<_>>()
            .join(",");
        candidates = format!(
            "arrayConcat({candidates}, arrayMap({lambda} -> tupleElement({lambda}, 1), arrayFilter({lambda} -> position({val}, tupleElement({lambda}, 2)) > 0, [{leftovers}])))"
        );
    }
    let matches = format!(
        "arrayFilter({lambda} -> match({val}, arrayElement({pat}, {lambda})), {candidates})"
    );
    let patterns = format!(
        "[{}]",
        items
            .iter()
            .map(|i| quote(&i.regex))
            .collect::<Vec<_>>()
            .join(",")
    );
    let id_list = format!(
        "[{}]",
        items
            .iter()
            .map(|i| format!("toInt64({})", i.id))
            .collect::<Vec<_>>()
            .join(",")
    );
    let encode = |key: &str| {
        quote(
            &key.as_bytes()
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect::<String>(),
        )
    };
    let tokens = format!("{prefix}tokens");
    let token_expr = byte_tokens(&val);
    let coarse_gate = if index_gate {
        let mut encoded: Vec<_> = gate_keys.iter().map(|key| encode(key)).collect();
        encoded.push(quote(OVERFLOW_TOKEN));
        format!("hasAny({}, [{}])", byte_tokens(&value), encoded.join(","))
    } else {
        "1".into()
    };
    let refined_gate = if index_gate {
        let mut clauses = vec![format!("hasAny({tokens}, [{}])", quote(OVERFLOW_TOKEN))];
        clauses.extend(conjunctions.iter().map(|keys| {
            format!(
                "hasAll({tokens}, [{}])",
                keys.iter()
                    .map(|key| encode(key))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }));
        clauses.join(" OR ")
    } else {
        "1".into()
    };
    let projection = if selected.is_empty() {
        "1".into()
    } else {
        selected.join(", ")
    };
    let output = selected
        .iter()
        .enumerate()
        .map(|(i, col)| {
            format!(
                "tupleElement({row}, {}) AS {}, ",
                i + 1,
                col.rsplit('.').next().unwrap_or(col)
            )
        })
        .collect::<String>();
    let base_ast = 320usize
        .saturating_add(selected.len().saturating_mul(8))
        .saturating_add(source.len())
        .saturating_add(filter.map_or(0, |s| s.len().saturating_mul(4)));
    let mut base_ast = base_ast;
    for expr in [&patterns, &id_list, &matches] {
        base_ast = base_ast.saturating_add(expression_ast(expr)?);
    }
    let render = |refined: bool| -> Result<(String, usize), ChError> {
        let (input, gate_ast) = if refined {
            // Bind tokens outside the physical source scope. A hidden source
            // column with this alias cannot shadow it under prefer_column_name_to_alias.
            let filter = filter
                .map(|f| format!("\nWHERE (\n{f}\n)"))
                .unwrap_or_default();
            (
                format!(
                    "WITH {token_expr} AS {tokens}\nSELECT {row}, {val}\nFROM (SELECT tuple({projection}) AS {row}, {value} AS {val}\nFROM {source}{filter}\n)\nWHERE ({refined_gate})"
                ),
                expression_ast(&refined_gate)?.saturating_add(expression_ast(&token_expr)?),
            )
        } else {
            let filter = filter
                .map(|f| format!(" AND (\n{f}\n)"))
                .unwrap_or_default();
            (
                format!(
                    "SELECT tuple({projection}) AS {row}, {value} AS {val}\nFROM {source}\nWHERE ({coarse_gate}){filter}"
                ),
                expression_ast(&coarse_gate)?,
            )
        };
        let sql = format!(
            "/* wildcard sweep: strategy=byte_dispatch source_scan_count=1 */\nWITH {patterns} AS {pat}, {id_list} AS {ids}\nSELECT {output}arrayElement({ids}, {idx}) AS indicator_id, 'raw' AS view\nFROM ({input}\n)\nARRAY JOIN {matches} AS {idx}"
        );
        Ok((sql, base_ast.saturating_add(gate_ast)))
    };
    let (mut sql, mut ast) = render(index_gate)?;
    let mut gate_strategy = if index_gate {
        ByteDispatchGate::RequiredFragments
    } else {
        ByteDispatchGate::Disabled
    };
    if index_gate && (sql.len() > options.max_bytes || ast > options.max_ast_nodes) {
        (sql, ast) = render(false)?;
        gate_strategy = ByteDispatchGate::SingleFragmentBudget;
    }
    if sql.len() > options.max_bytes || ast > options.max_ast_nodes {
        return fallback(ByteDispatchFallback::QueryBudget);
    }

    Ok(ByteDispatchPlan {
        sweep: SweepPlan {
            queries: vec![sql],
            strategy: SweepStrategy::ByteDispatch,
            source_scan_count: 1,
            estimated_ast: vec![ast],
            exact_only_items: needles.iter().filter(|g| g.is_empty()).count(),
        },
        dispatch_groups: dispatch.groups.len(),
        unassigned_items: dispatch.unassigned.len(),
        index_gate,
        gate_strategy,
        fallback: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_partitions_positions_using_only_their_required_grams() {
        // Vary overlap and empty plans; IDs never enter grouping, preserving
        // multiplicity even if all input rules have the same external ID.
        for stride in 1..65 {
            let needles: Vec<_> = (0..1000)
                .map(|i| {
                    if i % 17 == 0 {
                        vec![]
                    } else {
                        vec![format!("{:03}", i / stride), format!("x{:02}", i % 23)]
                    }
                })
                .collect();
            let Some(plan) = dispatch(&needles) else {
                continue;
            };
            assert!(plan.groups.len() <= MAX_GROUPS);
            assert!(plan.unassigned.len() <= MAX_UNASSIGNED);
            let mut seen = vec![0; needles.len()];
            for (gram, positions) in plan.groups {
                assert!(positions.len() <= MAX_MEMBERS);
                for pos in positions {
                    assert!(needles[pos - 1].contains(&gram));
                    seen[pos - 1] += 1;
                }
            }
            for pos in plan.unassigned {
                seen[pos - 1] += 1;
            }
            assert!(seen.iter().all(|n| *n == 1));
        }
    }
}
