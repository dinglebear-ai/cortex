//! Tree-wide guard for the lock-then-connection acquisition order.
//!
//! SQLite writes in this process need two scarce resources: the process-wide
//! write lock (`db::pool::write_lock`) and a pooled connection. Taking the
//! connection first pins it for the entire time the caller is queued on the
//! lock, and that wait is unbounded — during the 2026-08-24 incident the orphan
//! sweep held the lock for 15m25s, so a connection sat checked out and unusable
//! for over fifteen minutes while its holder did nothing but wait.
//!
//! `config::PoolBudget` partitions the pool on the assumption that a connection
//! is held only while work is being done. Every connection-before-lock site
//! breaks that assumption, so the budget is only true while this ordering is
//! uniform — which is why this is a test and not a comment. `db::write_conn`
//! makes the correct order the easy one; these tests make it the only one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A function that calls `write_lock()` directly instead of pairing lock and
/// connection through `db::write_conn`.
///
/// Legitimate only where the connection arrives from a caller that is already
/// *using* it, so the lock is second by construction and no `write_conn` exists
/// to reverse. Every entry has to say why; adding one is the reviewable act
/// that keeps the exception set from growing back into the default.
struct RawWriteLockSite {
    file: &'static str,
    function: &'static str,
    reason: &'static str,
}

const RAW_WRITE_LOCK_SITES: &[RawWriteLockSite] = &[
    RawWriteLockSite {
        file: "db/graph.rs",
        function: "merge_graph_delta",
        reason: "merges into the caller's per-connection TEMP staging tables, \
                 which were built over a chunked log scan that must not run \
                 under the write lock",
    },
    RawWriteLockSite {
        file: "db/graph.rs",
        function: "swap_graph_projection",
        reason: "swaps from the caller's per-connection TEMP staging tables, \
                 same construction as merge_graph_delta",
    },
    RawWriteLockSite {
        file: "db/maintenance_tests.rs",
        function: "orphan_child_sweep_scans_without_the_global_write_lock",
        reason: "test fixture that holds the lock and no connection, to prove \
                 the orphan scan does not need the lock",
    },
];

/// Where a pooled connection was taken, and how long it stays checked out.
#[derive(Debug)]
struct LiveConnection {
    line: usize,
    /// Last line on which the connection is still checked out.
    until: usize,
}

fn source_files() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }
    // CARGO_MANIFEST_DIR is the crate root regardless of where `cargo test` is
    // invoked from (worktree, CI runner, etc.).
    let mut files = Vec::new();
    walk(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    files.sort();
    // This file names every pattern it searches for as a literal.
    files.retain(|path| path.file_name().and_then(|n| n.to_str()) != Some("lock_order_tests.rs"));
    files
}

fn relative(path: &Path) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    path.strip_prefix(&root)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// Everything outside a line comment. Doc comments start with `//` too, so this
/// drops prose mentions of `write_lock()` along with them.
fn code_of(line: &str) -> &str {
    match line.find("//") {
        Some(idx) => &line[..idx],
        None => line,
    }
}

fn brace_delta(code: &str) -> i32 {
    code.chars().fold(0, |depth, ch| match ch {
        '{' => depth + 1,
        '}' => depth - 1,
        _ => depth,
    })
}

/// `fn name(` at the start of a line, with any combination of the usual
/// modifiers in front. Closures are not functions here on purpose: a closure
/// body belongs to the acquisition scope of its enclosing function.
fn function_name(code: &str) -> Option<&str> {
    let trimmed = code.trim_start();
    let mut rest = trimmed;
    for prefix in [
        "pub(crate) ",
        "pub(super) ",
        "pub(self) ",
        "pub ",
        "default ",
        "const ",
        "async ",
        "unsafe ",
        "extern ",
    ] {
        while let Some(stripped) = rest.strip_prefix(prefix) {
            rest = stripped;
        }
    }
    let rest = rest.strip_prefix("fn ")?;
    let name: &str = rest
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .next()?;
    (!name.is_empty()).then_some(name)
}

/// `needle` as a call, not as a fragment of a longer identifier. Test function
/// names describe what they exercise (`..._does_not_require_sqlite_write_lock`),
/// so a bare substring search reads their own names as call sites.
fn calls(code: &str, needle: &str) -> bool {
    let mut from = 0;
    while let Some(offset) = code[from..].find(needle) {
        let at = from + offset;
        let boundary = code[..at]
            .chars()
            .next_back()
            .is_none_or(|ch| !(ch.is_alphanumeric() || ch == '_'));
        if boundary {
            return true;
        }
        from = at + needle.len();
    }
    false
}

fn takes_pooled_connection(code: &str) -> bool {
    code.contains("pool.get()") || code.contains("pool.get_timeout(")
}

/// Both ways of reaching the write lock: the raw guard, and the helper that
/// takes the lock before borrowing a connection.
fn takes_write_lock(code: &str) -> bool {
    calls(code, "write_lock()") || calls(code, "write_conn(") || calls(code, "try_write_conn_for(")
}

/// The binding a `let` introduces, so an explicit `drop(name)` can end its
/// liveness before its block does.
fn bound_name(code: &str) -> Option<&str> {
    let rest = code.trim_start().strip_prefix("let ")?;
    let rest = rest.strip_prefix("mut ").unwrap_or(rest);
    let name: &str = rest
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .next()?;
    (!name.is_empty()).then_some(name)
}

/// Report every line where the write lock is taken while a pooled connection
/// acquired earlier in the same function is still checked out.
fn ordering_violations(source: &str) -> Vec<(usize, usize, String)> {
    let lines: Vec<&str> = source.lines().collect();
    let mut violations = Vec::new();
    let mut live: Vec<(LiveConnection, Option<String>)> = Vec::new();
    let mut depth = 0_i32;
    let mut current_fn = String::new();

    for (idx, raw) in lines.iter().enumerate() {
        let code = code_of(raw);
        if let Some(name) = function_name(code) {
            live.clear();
            current_fn = name.to_string();
        }

        live.retain(|(conn, _)| idx <= conn.until);

        if takes_write_lock(code)
            && let Some((conn, _)) = live.first()
        {
            violations.push((conn.line + 1, idx + 1, current_fn.clone()));
        }

        if takes_pooled_connection(code) {
            // Checked out until its own block closes; `drop(name)` below can end
            // it sooner.
            let opened = depth + brace_delta(&code[..code.find("pool.get").unwrap_or(0)]);
            let until = scope_end(&lines, idx, opened);
            live.push((
                LiveConnection { line: idx, until },
                bound_name(code).map(str::to_string),
            ));
        }

        for (conn, name) in &mut live {
            if let Some(name) = name
                && code.contains(&format!("drop({name})"))
                && idx > conn.line
            {
                conn.until = idx;
            }
        }

        depth += brace_delta(code);
    }
    violations
}

/// Last line on which a binding declared at `from` with block depth `opened` is
/// still in scope.
fn scope_end(lines: &[&str], from: usize, opened: i32) -> usize {
    let mut depth = opened;
    for (offset, raw) in lines.iter().enumerate().skip(from) {
        depth += brace_delta(code_of(raw));
        if depth < opened {
            return offset;
        }
    }
    lines.len()
}

#[test]
fn no_call_site_takes_a_pooled_connection_before_the_write_lock() {
    let mut findings: Vec<String> = Vec::new();
    for path in source_files() {
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (conn_line, lock_line, function) in ordering_violations(&source) {
            findings.push(format!(
                "{}: fn {function} borrows a pooled connection at line {conn_line} \
                 and only then takes the write lock at line {lock_line}",
                relative(&path),
            ));
        }
    }
    assert!(
        findings.is_empty(),
        "connection-before-lock pins a pooled connection for the whole (unbounded) \
         write-lock wait. Take the lock first — `db::write_conn(pool)` does both in \
         order — or release the connection before the write phase:\n{}",
        findings.join("\n")
    );
}

#[test]
fn raw_write_lock_calls_are_enumerated() {
    let allowed: BTreeMap<(&str, &str), &str> = RAW_WRITE_LOCK_SITES
        .iter()
        .map(|site| ((site.file, site.function), site.reason))
        .collect();

    let mut found: Vec<(String, String)> = Vec::new();
    for path in source_files() {
        let file = relative(&path);
        // `db/pool.rs` defines the lock and both acquisition helpers.
        if file == "db/pool.rs" {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        let mut current_fn = String::new();
        for raw in source.lines() {
            let code = code_of(raw);
            if let Some(name) = function_name(code) {
                current_fn = name.to_string();
            }
            if calls(code, "write_lock()") {
                found.push((file.clone(), current_fn.clone()));
            }
        }
    }
    found.sort();
    found.dedup();

    let unlisted: Vec<&(String, String)> = found
        .iter()
        .filter(|(file, function)| !allowed.contains_key(&(file.as_str(), function.as_str())))
        .collect();
    assert!(
        unlisted.is_empty(),
        "`write_lock()` pairs with a connection only through `db::write_conn`. \
         These call sites do neither — convert them, or enumerate them in \
         RAW_WRITE_LOCK_SITES with the reason their connection cannot be taken \
         second:\n{unlisted:#?}"
    );

    let stale: Vec<&(&str, &str)> = allowed
        .keys()
        .filter(|(file, function)| !found.iter().any(|(f, fun)| f == file && fun == function))
        .collect();
    assert!(
        stale.is_empty(),
        "RAW_WRITE_LOCK_SITES lists call sites that no longer exist; \
         delete them so the table keeps meaning what it says:\n{stale:#?}"
    );
}

/// The scanner has to see through blocks and `drop`, or it would either miss
/// real regressions or flag the legitimate read-then-write split. Pinned here
/// rather than left to the tree, which is expected to contain zero violations.
#[test]
fn scanner_distinguishes_a_held_connection_from_a_released_one() {
    let violating = r#"
fn bad(pool: &DbPool) -> Result<()> {
    let conn = pool.get()?;
    let _guard = write_lock();
    conn.execute("UPDATE t SET x = 1", [])
}
"#;
    assert_eq!(ordering_violations(violating).len(), 1);

    let scoped = r#"
fn scoped(pool: &DbPool) -> Result<()> {
    let rows: Vec<i64> = {
        let conn = pool.get()?;
        read(&conn)?
    };
    let conn = crate::db::write_conn(pool)?;
    write(&conn, &rows)
}
"#;
    assert!(ordering_violations(scoped).is_empty());

    let dropped = r#"
fn dropped(pool: &DbPool) -> Result<()> {
    let conn = pool.get()?;
    let rows = read(&conn)?;
    drop(conn);
    let conn = crate::db::write_conn(pool)?;
    write(&conn, &rows)
}
"#;
    assert!(ordering_violations(dropped).is_empty());

    let name_mentions_the_lock = r#"
fn existing_cursor_read_does_not_require_sqlite_write_lock() {
    let conn = pool.get().unwrap();
    read(&conn);
}
"#;
    assert!(ordering_violations(name_mentions_the_lock).is_empty());

    let separate_functions = r#"
fn reader(pool: &DbPool) -> Result<()> {
    let conn = pool.get()?;
    read(&conn)
}

fn writer(pool: &DbPool) -> Result<()> {
    let conn = crate::db::write_conn(pool)?;
    write(&conn)
}
"#;
    assert!(ordering_violations(separate_functions).is_empty());
}
