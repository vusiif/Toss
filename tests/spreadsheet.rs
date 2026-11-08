//! The normal Toss entry must read tables without an experimental probe flag.
use std::path::{Path, PathBuf};
use std::process::Command;
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("third_party/toss-sheet/tests/fixtures")
        .join(name)
}
fn run(args: &[&std::ffi::OsStr]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_toss"))
        .args(args)
        .output()
        .expect("toss starts")
}
#[cfg(feature = "spreadsheet")]
#[test]
fn automatic_csv_and_explicit_view_are_equivalent() {
    let path = fixture("simple.csv");
    let automatic = run(&[path.as_os_str()]);
    let explicit = run(&["view".as_ref(), path.as_os_str()]);
    assert!(automatic.status.success(), "{automatic:?}");
    assert!(explicit.status.success(), "{explicit:?}");
    assert_eq!(automatic.stdout, explicit.stdout);
    let text = String::from_utf8(automatic.stdout).unwrap();
    assert!(text.contains("Alpha,42"), "{text}");
    assert!(!text.contains('\u{1b}'));
    assert!(automatic.stderr.is_empty());
}
#[cfg(feature = "spreadsheet")]
#[test]
fn xlsx_and_tsv_use_table_handler_instead_of_zip_or_info() {
    for (name, value) in [("basic.xlsx", "Alpha,42"), ("edge.tsv", "Alpha,中文")] {
        let out = run(&[fixture(name).as_os_str()]);
        assert!(out.status.success(), "{name}: {out:?}");
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(text.contains(value), "{text}");
        assert!(!text.contains("Size:"));
    }
}
#[cfg(feature = "spreadsheet")]
#[test]
fn custom_path_read_and_actual_parser_errors_reach_normal_entry() {
    let good = run(&[fixture("custom-relative.xlsx").as_os_str()]);
    assert!(good.status.success(), "{good:?}");
    let missing = run(&[fixture("custom-missing.xlsx").as_os_str()]);
    assert!(!missing.status.success());
    assert!(
        String::from_utf8_lossy(&missing.stderr).contains("custom/missing.xml"),
        "{missing:?}"
    );
    let invalid = run(&[fixture("invalid-utf8.csv").as_os_str()]);
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("UTF-8"));
}
#[test]
fn info_does_not_parse_or_extract_spreadsheets() {
    let out = run(&["info".as_ref(), fixture("broken.xlsx").as_os_str()]);
    assert!(out.status.success(), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("spreadsheet"));
}
#[cfg(not(feature = "spreadsheet"))]
#[test]
fn tables_without_feature_report_unsupported_instead_of_extracting() {
    let out = run(&[fixture("basic.xlsx").as_os_str()]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
}

#[cfg(feature = "spreadsheet")]
fn json_view(name: &str, options: &[&str]) -> serde_json::Value {
    let path = fixture(name);
    let mut args = vec![
        "view".as_ref(),
        path.as_os_str(),
        "--format".as_ref(),
        "json".as_ref(),
    ];
    args.extend(options.iter().map(std::ffi::OsStr::new));
    let out = run(&args);
    assert!(out.status.success(), "{out:?}");
    assert!(out.stderr.is_empty(), "{out:?}");
    serde_json::from_slice(&out.stdout).expect("stdout is one valid JSON document")
}
#[cfg(feature = "spreadsheet")]
#[test]
fn json_export_keeps_scalar_types_and_selects_exact_sheet() {
    let value = json_view("basic.xlsx", &[]);
    assert_eq!(value["sheet"], "Data");
    assert_eq!(value["data"][0][0], "Alpha");
    assert_eq!(value["data"][0][1].as_f64(), Some(42.0));
    assert_eq!(value["data"][2][1], true);
    let second = json_view("basic.xlsx", &["--sheet", "Second"]);
    assert_eq!(second["sheet"], "Second");
    assert_eq!(second["data"][0][0], "second-only");
}
#[cfg(feature = "spreadsheet")]
#[test]
fn json_export_preserves_quoted_sheet_and_multiline_text() {
    let value = json_view("basic.xlsx", &["--sheet", "Quote\"Sheet"]);
    assert_eq!(value["sheet"], "Quote\"Sheet");
    assert_eq!(value["headers"][1], "line\nbreak");
    assert_eq!(value["data"][0][0], "back\\slash\nand\ttab");
}
#[cfg(feature = "spreadsheet")]
#[test]
fn no_header_and_delimiter_preserve_first_row_and_numeric_text() {
    let value = json_view("semicolon.csv", &["--no-header", "--delimiter", ";"]);
    assert_eq!(value["headers"], serde_json::json!(["A", "B"]));
    assert_eq!(
        value["data"],
        serde_json::json!([["code", "text"], ["001", "中文"]])
    );
    let offset = json_view("offset.xlsx", &["--no-header"]);
    assert_eq!(offset["headers"], serde_json::json!(["D", "E"]));
    assert_eq!(offset["data"][0][0], "Header");
}
#[cfg(feature = "spreadsheet")]
#[test]
fn empty_and_missing_formula_caches_keep_json_null_distinct_from_empty_string() {
    let value = json_view("explicit-empty-formula-cache.xlsx", &[]);
    assert!(value["data"][0][1].is_null());
    assert_eq!(value["data"][1][1], "");
    let empty = json_view("style-only.xlsx", &[]);
    assert_eq!(empty["rows"], 0);
    assert_eq!(empty["columns"], 0);
    assert_eq!(empty["headers"], serde_json::json!([]));
    assert_eq!(empty["data"], serde_json::json!([]));
}
#[cfg(feature = "spreadsheet")]
#[test]
fn table_option_errors_exit_before_emitting_data() {
    for (name, options) in [
        ("basic.xlsx", vec!["--sheet", "Missing"]),
        ("basic.xlsx", vec!["--delimiter", ";"]),
        ("simple.csv", vec!["--format", "yaml"]),
        ("simple.csv", vec!["--delimiter", ";;"]),
    ] {
        let path = fixture(name);
        let mut args = vec!["view".as_ref(), path.as_os_str()];
        args.extend(options.iter().map(std::ffi::OsStr::new));
        let out = run(&args);
        assert_eq!(out.status.code(), Some(2), "{out:?}");
        assert!(out.stdout.is_empty());
    }
    let path = fixture("simple.csv");
    let out = run(&[
        "view".as_ref(),
        path.as_os_str(),
        path.as_os_str(),
        "--format".as_ref(),
        "json".as_ref(),
    ]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    let out = run(&[
        "view".as_ref(),
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("README.md")
            .as_os_str(),
        "--format".as_ref(),
        "json".as_ref(),
    ]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
}
#[cfg(feature = "spreadsheet")]
#[test]
fn explicit_csv_matches_existing_redirected_output() {
    let path = fixture("basic.xlsx");
    let default = run(&[path.as_os_str()]);
    let explicit = run(&[
        "view".as_ref(),
        path.as_os_str(),
        "--format".as_ref(),
        "csv".as_ref(),
    ]);
    assert!(explicit.status.success());
    assert_eq!(default.stdout, explicit.stdout);
}
#[cfg(feature = "spreadsheet")]
#[test]
fn explicit_header_preserves_export_contract_and_conflicts_are_rejected() {
    assert_eq!(
        json_view("basic.xlsx", &[]),
        json_view("basic.xlsx", &["--header"])
    );
    let path = fixture("basic.xlsx");
    for args in [
        vec!["--header", "--no-header"],
        vec!["--no-header", "--header"],
    ] {
        let mut command = vec!["view".as_ref(), path.as_os_str()];
        command.extend(args.iter().map(std::ffi::OsStr::new));
        let output = run(&command);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
}
#[cfg(feature = "spreadsheet")]
#[test]
fn csv_encoding_selection_and_inference_are_strict_and_visible() {
    let expected = json_view("encoding-utf8.csv", &["--no-header"]);
    for name in [
        "encoding-utf8-bom.csv",
        "encoding-utf16le.csv",
        "encoding-utf16be.csv",
    ] {
        let value = json_view(name, &["--no-header"]);
        assert_eq!(value["data"], expected["data"]);
    }
    let path = fixture("encoding-gb18030.csv");
    let automatic = run(&[
        "view".as_ref(),
        path.as_os_str(),
        "--no-header".as_ref(),
        "--format".as_ref(),
        "json".as_ref(),
    ]);
    assert!(automatic.status.success());
    assert!(String::from_utf8_lossy(&automatic.stderr).contains("inferred as GB18030"));
    let value: serde_json::Value = serde_json::from_slice(&automatic.stdout).unwrap();
    assert_eq!(value["data"], expected["data"]);
    assert_eq!(
        json_view(
            "encoding-gb18030.csv",
            &["--no-header", "--encoding", "gb18030"]
        )["data"],
        expected["data"]
    );
    assert_eq!(
        json_view(
            "encoding-gb18030.csv",
            &["--no-header", "--encoding", "gbk"]
        )["data"],
        expected["data"]
    );
    assert_eq!(
        json_view(
            "encoding-utf16le-no-bom.csv",
            &["--no-header", "--encoding", "utf-16le"]
        )["data"],
        expected["data"]
    );
    let tsv = json_view("encoding-utf16be.tsv", &[]);
    assert_eq!(tsv["data"][0], serde_json::json!(["张三", "00123"]));
    for (name, options) in [
        ("encoding-malformed-utf16.csv", vec![]),
        ("encoding-malformed-utf8-bom.csv", vec![]),
        ("encoding-malformed-gb18030.csv", vec![]),
        ("encoding-utf16le.csv", vec!["--encoding", "utf-8"]),
        ("basic.xlsx", vec!["--encoding", "utf-8"]),
    ] {
        let path = fixture(name);
        let mut args = vec![
            "view".as_ref(),
            path.as_os_str(),
            "--format".as_ref(),
            "json".as_ref(),
        ];
        args.extend(options.iter().map(std::ffi::OsStr::new));
        let out = run(&args);
        assert!(!out.status.success(), "{name}");
        assert!(out.stdout.is_empty(), "{name}");
    }
}
#[cfg(not(feature = "spreadsheet"))]
#[test]
fn structured_table_options_still_report_disabled_feature() {
    let path = fixture("basic.xlsx");
    let out = run(&[
        "view".as_ref(),
        path.as_os_str(),
        "--format".as_ref(),
        "json".as_ref(),
    ]);
    assert_eq!(out.status.code(), Some(3));
    assert!(out.stdout.is_empty());
}
