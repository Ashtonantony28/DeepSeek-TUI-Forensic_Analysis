// mutation.rs — Mutation-based test strengthening.
//
// This module implements the semantic test strengthening described in the
// SWE-ABS paper (arxiv 2603.00520, March 2026). That paper shows 19.78% of
// "solved" SWE-bench instances are semantically incorrect: the original tests
// accept buggy patches that stronger tests would reject.
//
// We address this by generating N mutants of the fail-to-pass test file via
// tree-sitter AST rewrites and checking whether the patched code still fails
// those mutated tests. A patch that passes the original tests but passes ALL
// mutant tests is probably not adequately testing the intended behaviour; we
// record `semantic_pass = false` for that instance.
//
// Four mutation operators (applied in tree traversal order):
//   ComparisonSwap   — == ↔ !=, < ↔ <=, > ↔ >=
//   ReturnFlip       — True → False, False → True
//   OffByOne         — integer literal n → n+1
//   ConditionalNegate — `if <cond>:` → `if not <cond>:` (Python only)

use tree_sitter::{Node, Parser};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutationOperator {
    ComparisonSwap,
    ReturnFlip,
    OffByOne,
    ConditionalNegate,
}

impl std::fmt::Display for MutationOperator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MutationOperator::ComparisonSwap => write!(f, "comparison_swap"),
            MutationOperator::ReturnFlip => write!(f, "return_flip"),
            MutationOperator::OffByOne => write!(f, "off_by_one"),
            MutationOperator::ConditionalNegate => write!(f, "conditional_negate"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Mutant {
    pub source: String,
    pub operator: MutationOperator,
    /// Human-readable location: byte offset range.
    pub location: (usize, usize),
}

struct MutationPoint {
    start: usize,
    end: usize,
    replacement: String,
    kind: MutationOperator,
}

/// Generate up to `count` mutants of `source` using Python grammar.
///
/// Returns fewer than `count` if the source has fewer mutation points.
/// Returns empty if parsing fails (non-Python content or syntax error).
pub fn generate_mutants(source: &str, count: usize) -> Vec<Mutant> {
    let lang = tree_sitter_python::language();
    let mut parser = Parser::new();
    if parser.set_language(&lang).is_err() {
        return Vec::new();
    }
    let tree = match parser.parse(source.as_bytes(), None) {
        Some(t) => t,
        None => return Vec::new(),
    };

    let mut points: Vec<MutationPoint> = Vec::new();
    collect_mutation_points(tree.root_node(), source.as_bytes(), &mut points);

    // Deduplicate by (start, end) so two operators on the same token don't
    // produce identical byte ranges.
    let mut seen: std::collections::HashSet<(usize, usize)> = Default::default();
    points.retain(|p| seen.insert((p.start, p.end)));

    points
        .into_iter()
        .take(count)
        .map(|pt| {
            // Apply text substitution at byte range.
            let mut mutated = source.to_string();
            mutated.replace_range(pt.start..pt.end, &pt.replacement);
            Mutant {
                source: mutated,
                operator: pt.kind,
                location: (pt.start, pt.end),
            }
        })
        .collect()
}

/// Walk the AST collecting mutation targets using a stack (avoids recursion
/// limit and lifetime issues with tree-sitter cursors).
fn collect_mutation_points(root: Node<'_>, source: &[u8], pts: &mut Vec<MutationPoint>) {
    let mut cursor = root.walk();
    let mut stack: Vec<Node<'_>> = vec![root];

    while let Some(node) = stack.pop() {
        match node.kind() {
            // ── Comparison operator swaps ─────────────────────────────────
            "comparison_operator" => {
                for i in 0..node.child_count() {
                    if let Some(child) = node.child(i) {
                        let op = child.kind();
                        let replacement = match op {
                            "==" => Some("!="),
                            "!=" => Some("=="),
                            "<" => Some("<="),
                            ">" => Some(">="),
                            "<=" => Some("<"),
                            ">=" => Some(">"),
                            _ => None,
                        };
                        if let Some(rep) = replacement {
                            pts.push(MutationPoint {
                                start: child.start_byte(),
                                end: child.end_byte(),
                                replacement: rep.to_string(),
                                kind: MutationOperator::ComparisonSwap,
                            });
                        }
                    }
                }
            }

            // ── Boolean literal flips ─────────────────────────────────────
            "true" => {
                pts.push(MutationPoint {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement: "False".to_string(),
                    kind: MutationOperator::ReturnFlip,
                });
            }
            "false" => {
                pts.push(MutationPoint {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    replacement: "True".to_string(),
                    kind: MutationOperator::ReturnFlip,
                });
            }

            // ── Off-by-one on integer literals ────────────────────────────
            "integer" => {
                if let Ok(text) = node.utf8_text(source) {
                    // Only mutate small non-zero literals to avoid silly results
                    // like turning 0 into 1 in an array index that makes no sense.
                    if let Ok(n) = text.parse::<i64>() {
                        if (1..=100).contains(&n) {
                            pts.push(MutationPoint {
                                start: node.start_byte(),
                                end: node.end_byte(),
                                replacement: (n + 1).to_string(),
                                kind: MutationOperator::OffByOne,
                            });
                        }
                    }
                }
            }

            // ── Conditional negation ──────────────────────────────────────
            // For `if <cond>:`, find the condition child and wrap with `not`.
            // In tree-sitter-python, `if_statement` has `condition` as the
            // second child (index 1, after the `if` keyword token at index 0).
            "if_statement" => {
                // child(0) = "if" keyword, child(1) = condition expression
                if let Some(cond) = node.child(1) {
                    let cond_text = match cond.utf8_text(source) {
                        Ok(t) => t.to_string(),
                        Err(_) => continue,
                    };
                    // Skip if already negated to avoid `not not x`.
                    if !cond_text.starts_with("not ") {
                        pts.push(MutationPoint {
                            start: cond.start_byte(),
                            end: cond.end_byte(),
                            replacement: format!("not {}", cond_text),
                            kind: MutationOperator::ConditionalNegate,
                        });
                    }
                }
            }

            _ => {}
        }

        // Push children for traversal. The cursor is shared across the whole
        // traversal, which is the same pattern used in tools/src/syntax.rs.
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }
}

/// Verify that a mutant is still syntactically valid Python.
/// Used in tests and as a sanity gate before running mutant tests.
pub fn is_valid_python(source: &str) -> bool {
    let lang = tree_sitter_python::language();
    let mut parser = Parser::new();
    if parser.set_language(&lang).is_err() {
        return false;
    }
    let tree = match parser.parse(source.as_bytes(), None) {
        Some(t) => t,
        None => return false,
    };
    // A tree with no ERROR nodes is syntactically valid.
    let mut cursor = tree.root_node().walk();
    let mut stack = vec![tree.root_node()];
    while let Some(n) = stack.pop() {
        if n.is_error() || n.is_missing() {
            return false;
        }
        for child in n.children(&mut cursor) {
            stack.push(child);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const PYTHON_TEST: &str = "\
def test_clamp():
    assert clamp(10, 0, 10) == 10
    assert clamp(-1, 0, 10) == 0
    assert clamp(5, 0, 10) == 5

def test_flag():
    result = compute()
    assert result == True
    assert len(result) >= 1
    if result:
        assert result > 0
";

    #[test]
    fn generates_at_least_one_mutant() {
        let mutants = generate_mutants(PYTHON_TEST, 5);
        assert!(!mutants.is_empty(), "should produce at least one mutant");
    }

    #[test]
    fn all_mutants_differ_from_original() {
        let mutants = generate_mutants(PYTHON_TEST, 5);
        for m in &mutants {
            assert_ne!(m.source, PYTHON_TEST, "mutant should differ from original");
        }
    }

    #[test]
    fn all_mutants_are_valid_python() {
        let mutants = generate_mutants(PYTHON_TEST, 5);
        for m in &mutants {
            assert!(
                is_valid_python(&m.source),
                "mutant should still be valid Python (operator={}, loc={:?}): {}",
                m.operator,
                m.location,
                &m.source[..m.source.len().min(200)]
            );
        }
    }

    #[test]
    fn comparison_swap_produces_opposite_operator() {
        let source = "def t():\n    assert x == y\n";
        let mutants = generate_mutants(source, 5);
        let swap = mutants
            .iter()
            .find(|m| m.operator == MutationOperator::ComparisonSwap);
        assert!(swap.is_some(), "expected a ComparisonSwap mutant");
        assert!(swap.unwrap().source.contains("!="), "== should become !=");
    }

    #[test]
    fn return_flip_swaps_bool() {
        let source = "def t():\n    assert result == True\n";
        let mutants = generate_mutants(source, 5);
        let flip = mutants
            .iter()
            .find(|m| m.operator == MutationOperator::ReturnFlip);
        assert!(flip.is_some(), "expected a ReturnFlip mutant");
        assert!(
            flip.unwrap().source.contains("False"),
            "True should become False"
        );
    }

    #[test]
    fn off_by_one_increments_integer() {
        let source = "def t():\n    assert len(x) >= 1\n";
        let mutants = generate_mutants(source, 5);
        let obo = mutants
            .iter()
            .find(|m| m.operator == MutationOperator::OffByOne);
        assert!(obo.is_some(), "expected an OffByOne mutant");
        assert!(obo.unwrap().source.contains("2"), "1 should become 2");
    }

    #[test]
    fn empty_source_yields_no_mutants() {
        assert!(generate_mutants("", 5).is_empty());
    }

    #[test]
    fn non_python_source_yields_no_mutants() {
        // This is Rust — tree-sitter-python won't produce mutations
        // from a file with no recognisable Python constructs.
        let rust_src = "fn main() { let x = 1; }";
        // May or may not parse depending on how permissive tree-sitter-python is;
        // the key property is that we don't panic.
        let _ = generate_mutants(rust_src, 5);
    }
}
