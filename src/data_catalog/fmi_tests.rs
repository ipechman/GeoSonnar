use super::*;

fn boundary_rows(index: usize, rows: usize) -> FmiChunkData {
    FmiChunkData {
        schema_version: 1,
        plan_sha256: "a".repeat(64),
        chunk_index: index,
        body_sha256: "b".repeat(64),
        raw_rows: rows,
        excluded_boundary_rows: rows,
        strikes: Vec::new(),
    }
}

#[test]
fn cumulative_row_budget_accepts_the_limit_and_preserves_prior_chunks_on_refusal() {
    let mut accepted = BTreeMap::new();
    accept_chunk(&mut accepted, boundary_rows(0, provider::MAX_STRIKES - 1)).unwrap();
    accept_chunk(&mut accepted, boundary_rows(1, 1)).unwrap();
    let before = serde_json::to_value(&accepted).unwrap();
    let error = accept_chunk(&mut accepted, boundary_rows(2, 1)).unwrap_err();
    assert!(error.contains("Cumulative FMI row limit"));
    assert_eq!(serde_json::to_value(&accepted).unwrap(), before);
    assert!(!accepted.contains_key(&2));
}

#[test]
fn row_count_overflow_is_refused_without_changing_accepted_data() {
    let mut accepted = BTreeMap::new();
    accept_chunk(&mut accepted, boundary_rows(0, 1)).unwrap();
    let before = serde_json::to_value(&accepted).unwrap();
    assert!(accept_chunk(&mut accepted, boundary_rows(1, usize::MAX)).is_err());
    assert_eq!(serde_json::to_value(&accepted).unwrap(), before);
}
