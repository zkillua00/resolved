//! Build-time conversion to Tree-sitter's existing small-state table format.
//! Keep an initial dense prefix for fast common-state lookups; preserve state IDs, actions, scanners, and grammar ABI.
use std::{
    collections::BTreeMap,
    fmt::Write,
    path::{Path, PathBuf},
};

fn array<'a>(source: &'a str, name: &str) -> (&'a str, &'a str, &'a str) {
    let start = source
        .find(&format!("static const {name}"))
        .expect("pinned parser array");
    let body = start + source[start..].find("{\n").expect("array initializer") + 2;
    let end = body + source[body..].find("\n};").expect("array terminator");
    (&source[..body], &source[body..end], &source[end..])
}

pub fn prepare(path: &Path) -> PathBuf {
    println!("cargo:rerun-if-env-changed=RESOLVED_TREE_SITTER_DENSE");
    let helper = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/tree-sitter-compact.rs");
    println!("cargo:rerun-if-changed={}", helper.display());
    // Reproducible baseline for equivalence and performance checks.
    if std::env::var_os("RESOLVED_TREE_SITTER_DENSE").is_some() {
        return path.to_owned();
    }
    let source = std::fs::read_to_string(path).expect("read pinned parser");
    let compact = transform(&source);
    let output = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo output directory"))
        .join("parser-compact.c");
    std::fs::write(&output, compact).expect("write compact parser");
    output
}

pub fn transform(source: &str) -> String {
    let count_line = source
        .lines()
        .find(|line| line.starts_with("#define LARGE_STATE_COUNT "))
        .expect("large state count");
    let count: usize = count_line
        .split_whitespace()
        .last()
        .unwrap()
        .parse()
        .unwrap();
    let retained = if count > 1024 { 512 } else { 128 };
    if retained == count {
        return source.to_owned();
    }
    let (_, old_small, _) = array(source, "uint16_t ts_small_parse_table[]");
    // Each comma-delimited initializer emits one uint16_t. Designators must
    // match its position; fail closed if a future generator changes the format.
    let mut offset = 0usize;
    for entry in old_small
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if let Some(entry) = entry.strip_prefix('[') {
            let (index, _) = entry.split_once("] = ").expect("small table designator");
            assert_eq!(index.parse::<usize>().unwrap(), offset);
        }
        offset += 1;
    }
    let (_, dense, _) = array(
        source,
        "uint16_t ts_parse_table[LARGE_STATE_COUNT][SYMBOL_COUNT]",
    );
    let mut dense_zero = String::new();
    let mut packed = String::new();
    let mut map = String::new();
    let mut rows = 0;
    for row in dense.split("  },").map(str::trim).filter(|s| !s.is_empty()) {
        let (state, body) = row
            .strip_prefix('[')
            .expect("row initializer")
            .split_once("] = {\n")
            .expect("dense row");
        let state = state
            .strip_prefix("STATE(")
            .and_then(|s| s.strip_suffix(')'))
            .unwrap_or(state);
        let state: usize = state.parse().unwrap();
        assert_eq!(state, rows);
        rows += 1;
        if state < retained {
            write!(dense_zero, "  [STATE({state})] = {{\n{body}  }},\n").unwrap();
            continue;
        }
        let mut groups: BTreeMap<u16, Vec<&str>> = BTreeMap::new();
        let mut symbols = std::collections::BTreeSet::new();
        for line in body.lines().map(str::trim).filter(|s| !s.is_empty()) {
            let (symbol, value) = line
                .strip_prefix('[')
                .expect("symbol initializer")
                .split_once("] = ")
                .unwrap();
            assert!(symbols.insert(symbol), "duplicate symbol");
            let value = value.strip_suffix(',').unwrap();
            let value = value
                .strip_prefix("ACTIONS(")
                .or_else(|| value.strip_prefix("STATE("))
                .expect("action or state");
            let value: u16 = value.strip_suffix(')').unwrap().parse().unwrap();
            if value != 0 {
                groups.entry(value).or_default().push(symbol);
            }
        }
        writeln!(map, "  [SMALL_STATE({state})] = {offset},").unwrap();
        writeln!(packed, "  [{offset}] = {},", groups.len()).unwrap();
        offset += 1;
        for (value, symbols) in groups {
            writeln!(
                packed,
                "    {value}, {}, {},",
                symbols.len(),
                symbols.join(", ")
            )
            .unwrap();
            offset += 2 + symbols.len();
        }
    }
    assert_eq!(rows, count);
    let (before, _, after) = array(
        source,
        "uint16_t ts_parse_table[LARGE_STATE_COUNT][SYMBOL_COUNT]",
    );
    let result = format!("{before}{dense_zero}{after}");
    let (before, body, after) = array(&result, "uint16_t ts_small_parse_table[]");
    let result = format!("{before}{body}\n{packed}{after}");
    let (before, body, after) = array(&result, "uint32_t ts_small_parse_table_map[]");
    format!("{before}{map}{body}{after}").replacen(
        count_line,
        &format!("#define LARGE_STATE_COUNT {retained}"),
        1,
    )
}
