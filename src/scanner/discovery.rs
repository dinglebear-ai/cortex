use super::*;

pub(super) fn collect_supported_files_bounded(
    path: &Path,
    result: &mut DiscoveryResult,
    entries: &mut usize,
    path_bytes: &mut usize,
    start_after: Option<&Path>,
    budget: &DiscoveryBudget,
    started: Instant,
) {
    if result.capped || started.elapsed() >= budget.deadline {
        result.capped = true;
        return;
    }
    if let Err(error) = validate_path(path) {
        classify_path_error(&error, &mut result.diagnostics);
        record_file_error(&mut result.diagnostics, path, &error);
        return;
    }
    if path.is_file() {
        if supported_discovered_file(path) {
            result.files.push(path.to_path_buf());
        } else {
            result.diagnostics.unsupported_files += 1;
        }
        return;
    }
    if !should_descend_transcript_dir(path) {
        return;
    }
    let mut cursor = start_after.map(Path::to_path_buf);
    let mut completed_directory = None;
    // Resuming through already-visited ancestors is navigation, not new work.
    // Charging them again can consume a small page before reaching its cursor.
    if let Some(ancestor) = cursor
        .as_deref()
        .and_then(|cursor| cursor.strip_prefix(path).ok())
        .and_then(|relative| relative.components().next())
        .map(|component| path.join(component))
        .filter(|ancestor| ancestor.is_dir())
    {
        collect_supported_files_bounded(
            &ancestor,
            result,
            entries,
            path_bytes,
            cursor.as_deref(),
            budget,
            started,
        );
        if result.capped {
            return;
        }
        completed_directory = Some(ancestor);
        cursor = result.next_cursor.clone().or(cursor);
    }
    loop {
        let read_dir = match fs::read_dir(path) {
            Ok(read_dir) => read_dir,
            Err(error) => {
                record_discovered_path_error(&mut result.diagnostics, path, &error.into());
                return;
            }
        };
        // read_dir order is unspecified. Keep only the lexical next page, bounded
        // by the remaining path/count allowance, rather than using its last raw
        // entry as a cursor and discarding everything collected when the cap hits.
        let mut children = std::collections::BTreeSet::new();
        let mut child_bytes = 0usize;
        let mut more = false;
        let mut first_excluded: Option<PathBuf> = None;
        let remaining_entries = budget.max_entries.saturating_sub(*entries);
        let remaining_bytes = budget.max_path_bytes.saturating_sub(*path_bytes);
        for entry in read_dir {
            if started.elapsed() >= budget.deadline {
                result.capped = true;
                return; // No cursor advance for an incompletely ordered page.
            }
            match entry {
                Ok(entry) => {
                    let child = entry.path();
                    if completed_directory.as_ref() == Some(&child)
                        || cursor.as_deref().is_some_and(|cursor| {
                            child <= cursor
                                && (!cursor.starts_with(&child)
                                    || (child == cursor && !child.is_dir()))
                        })
                    {
                        continue;
                    }
                    if first_excluded.as_ref().is_some_and(|limit| &child >= limit) {
                        more = true;
                        continue;
                    }
                    child_bytes = child_bytes.saturating_add(child.as_os_str().len());
                    children.insert(child);
                    while children.len() > remaining_entries || child_bytes > remaining_bytes {
                        let Some(last) = children.pop_last() else {
                            break;
                        };
                        child_bytes -= last.as_os_str().len();
                        first_excluded = Some(last);
                        more = true;
                    }
                }
                Err(error) => {
                    record_discovered_path_error(&mut result.diagnostics, path, &error.into());
                }
            }
        }
        let mut restart = false;
        while let Some(child) = children.pop_first() {
            if result.capped
                || started.elapsed() >= budget.deadline
                || *entries >= budget.max_entries
                || child.as_os_str().len() > budget.max_path_bytes.saturating_sub(*path_bytes)
            {
                result.capped = true;
                break;
            }
            *entries += 1;
            *path_bytes += child.as_os_str().len();
            result.next_cursor = Some(child.clone());
            if child.is_dir() {
                // Release buffered siblings before descent: the path allowance
                // bounds retained memory across the entire traversal, not per level.
                children.clear();
                collect_supported_files_bounded(
                    &child,
                    result,
                    entries,
                    path_bytes,
                    cursor.as_deref(),
                    budget,
                    started,
                );
                if result.capped {
                    return;
                }
                completed_directory = Some(child);
                cursor = result.next_cursor.clone();
                restart = true;
                break;
            } else if supported_discovered_file(&child) {
                result.files.push(child);
            } else {
                result.diagnostics.unsupported_files += 1;
            }
        }
        if restart {
            continue;
        }
        result.capped |= more;
        return;
    }
}
