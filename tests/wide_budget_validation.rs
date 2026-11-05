//! Resource diagnostics only; these controls do not score or narrow a grid.
use geosonnar::{
    data_size::{format_gib_pair, gib_to_bytes},
    wide_area_pair::{WideBudgets, check_preliminary_budgets, preliminary_output_bytes},
};

#[test]
fn issue_one_large_grid_reports_only_output_when_work_limit_is_disabled() {
    let locations = 18_387_820;
    let mut limits = WideBudgets {
        work_limit_enabled: false,
        max_work_units: 0,
        max_output_bytes: gib_to_bytes(1.598).unwrap(),
        ..Default::default()
    };
    let before = serde_json::to_vec(&limits).unwrap();
    let minimum = preliminary_output_bytes(locations).unwrap();
    assert_eq!(minimum, 1_782_007_936);
    let error = check_preliminary_budgets(locations, &limits).unwrap_err();
    assert!(error.starts_with("Output allowance too small:"));
    assert!(error.contains("1.660 GiB") && error.contains("1.598 GiB"));
    assert!(error.contains("Increase the output allowance"));
    assert!(error.contains("not a final output-size forecast"));
    assert!(!error.contains("work"));
    assert_eq!(serde_json::to_vec(&limits).unwrap(), before);
    limits.max_output_bytes = minimum;
    assert_eq!(
        check_preliminary_budgets(locations, &limits).unwrap(),
        minimum
    );
    assert_eq!(locations, 18_387_820);
}

#[test]
fn preliminary_output_and_enabled_work_failures_are_separate_and_exact() {
    let locations = 18_387_820;
    let minimum = preliminary_output_bytes(locations).unwrap();
    let mut limits = WideBudgets {
        work_limit_enabled: true,
        max_work_units: locations - 1,
        max_output_bytes: minimum - 1,
        ..Default::default()
    };
    let (required, allowed) = format_gib_pair(minimum, minimum - 1);
    assert_ne!(required, allowed);
    let before = serde_json::to_vec(&limits).unwrap();
    let error = check_preliminary_budgets(locations, &limits).unwrap_err();
    assert!(error.starts_with("Output allowance too small:"));
    assert!(error.contains(&required) && error.contains(&allowed));
    let lines: Vec<_> = error.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains(&required) && lines[0].contains(&allowed));
    assert!(lines[1].starts_with("Enabled work-unit limit too small:"));
    assert!(lines[1].contains("18387820") && lines[1].contains("18387819"));
    assert_eq!(serde_json::to_vec(&limits).unwrap(), before);
    limits.max_output_bytes = minimum;
    let error = check_preliminary_budgets(locations, &limits).unwrap_err();
    assert!(error.starts_with("Enabled work-unit limit too small:"));
    assert!(error.contains("18387820") && error.contains("18387819"));
    assert!(!error.contains("Output allowance"));
    limits.max_work_units = locations;
    assert_eq!(
        check_preliminary_budgets(locations, &limits).unwrap(),
        minimum
    );
    limits.max_work_units = 0;
    limits.work_limit_enabled = false;
    assert_eq!(
        check_preliminary_budgets(locations, &limits).unwrap(),
        minimum
    );
    assert!(preliminary_output_bytes(u64::MAX).is_err());
}
