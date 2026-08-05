//! Bounded parser for exact Git commit metadata and numstat summaries.

#[path = "commits_support.rs"]
mod support;
use std::collections::HashSet;
use support::{
    COMMIT_MARKER, add_numstat, bounded_field, email_hash, is_object_id, object_id, parents,
    timestamp, utf8,
};
pub use support::{
    COMMIT_SHOW_FORMAT, CommitParseError, CommitParseErrorKind, CommitParseOptions,
    CommitPathChange, ParsedCommit,
};

pub fn commit_show_arguments(
    shas: &[String],
    max_commits: usize,
) -> Result<Vec<String>, CommitParseError> {
    if max_commits == 0 {
        return Err(CommitParseError::new(
            0,
            0,
            CommitParseErrorKind::InvalidOptions("max_commits"),
        ));
    }
    if shas.len() > max_commits {
        return Err(CommitParseError::new(
            0,
            0,
            CommitParseErrorKind::TooManyCommits { max: max_commits },
        ));
    }
    if shas.is_empty() {
        return Err(CommitParseError::new(
            0,
            0,
            CommitParseErrorKind::InvalidOptions("commit list is empty"),
        ));
    }
    if shas.iter().any(|sha| !is_object_id(sha.as_bytes())) {
        return Err(CommitParseError::new(
            0,
            0,
            CommitParseErrorKind::InvalidRequestedObjectId,
        ));
    }
    let mut arguments = vec![
        "show".to_string(),
        "--no-walk=unsorted".to_string(),
        "--no-color".to_string(),
        "--no-ext-diff".to_string(),
        "--no-textconv".to_string(),
        "--diff-merges=first-parent".to_string(),
        "--find-renames".to_string(),
        "--numstat".to_string(),
        "-z".to_string(),
        format!("--format={COMMIT_SHOW_FORMAT}"),
    ];
    arguments.extend(shas.iter().cloned());
    Ok(arguments)
}

pub fn parse_commit_show(
    input: &[u8],
    options: CommitParseOptions,
) -> Result<Vec<ParsedCommit>, CommitParseError> {
    if options.max_input_bytes == 0 {
        return Err(CommitParseError::new(
            0,
            0,
            CommitParseErrorKind::InvalidOptions("max_input_bytes"),
        ));
    }
    if options.max_commits == 0 {
        return Err(CommitParseError::new(
            0,
            0,
            CommitParseErrorKind::InvalidOptions("max_commits"),
        ));
    }
    if input.len() > options.max_input_bytes {
        return Err(CommitParseError::new(
            0,
            0,
            CommitParseErrorKind::InputTooLong {
                actual: input.len(),
                max: options.max_input_bytes,
            },
        ));
    }
    if input.is_empty() {
        return Ok(Vec::new());
    }
    if input.last() != Some(&0) {
        return Err(CommitParseError::new(
            0,
            0,
            CommitParseErrorKind::MissingTerminator,
        ));
    }

    let tokens = input.split(|byte| *byte == 0).collect::<Vec<_>>();
    let mut token_index = 0usize;
    let mut commits = Vec::new();
    let mut seen = HashSet::new();

    while token_index + 1 < tokens.len() {
        let commit_index = commits.len();
        if commit_index >= options.max_commits {
            return Err(CommitParseError::new(
                commit_index,
                token_index,
                CommitParseErrorKind::TooManyCommits {
                    max: options.max_commits,
                },
            ));
        }
        if tokens[token_index] != COMMIT_MARKER {
            return Err(CommitParseError::new(
                commit_index,
                token_index,
                CommitParseErrorKind::ExpectedMarker,
            ));
        }
        let fields = [
            "sha",
            "parents",
            "author_name",
            "author_email",
            "authored_at",
            "committed_at",
            "subject",
            "separator",
        ];
        for (offset, name) in fields.iter().enumerate() {
            if token_index + offset + 1 >= tokens.len() {
                return Err(CommitParseError::new(
                    commit_index,
                    token_index + offset,
                    CommitParseErrorKind::MissingMetadataField(name),
                ));
            }
        }
        let base = token_index + 1;
        let sha = object_id(tokens[base], "commit", commit_index, base)?;
        if !seen.insert(sha.clone()) {
            return Err(CommitParseError::new(
                commit_index,
                base,
                CommitParseErrorKind::DuplicateCommit,
            ));
        }
        let parent_shas = parents(tokens[base + 1], commit_index, base + 1)?;
        let author_name = options
            .store_author_name
            .then(|| utf8(tokens[base + 2], "author name", commit_index, base + 2))
            .transpose()?;
        bounded_field(tokens[base + 3], commit_index, base + 3)?;
        let author_email_hash = options
            .store_author_email_hash
            .then(|| email_hash(tokens[base + 3]))
            .flatten();
        let authored_at = timestamp(tokens[base + 4], "authored_at", commit_index, base + 4)?;
        let committed_at = timestamp(tokens[base + 5], "committed_at", commit_index, base + 5)?;
        let subject = utf8(tokens[base + 6], "subject", commit_index, base + 6)?;
        if !tokens[base + 7].is_empty() {
            return Err(CommitParseError::new(
                commit_index,
                base + 7,
                CommitParseErrorKind::InvalidMetadataSeparator,
            ));
        }
        token_index = base + 8;
        let mut commit = ParsedCommit {
            sha,
            parent_shas,
            author_name,
            author_email_hash,
            authored_at,
            committed_at,
            subject,
            changed_files: 0,
            insertions: 0,
            deletions: 0,
            binary_files: 0,
            changed_paths: Vec::new(),
            paths_truncated: false,
        };
        let mut first_numstat = true;

        while token_index + 1 < tokens.len() && tokens[token_index] != COMMIT_MARKER {
            let mut header = tokens[token_index];
            if first_numstat {
                header = header.strip_prefix(&[10]).unwrap_or(header);
                first_numstat = false;
            }
            if header.is_empty() {
                token_index += 1;
                continue;
            }
            bounded_field(header, commit_index, token_index)?;
            let path = header.splitn(3, |byte| *byte == 9).nth(2).ok_or_else(|| {
                CommitParseError::new(
                    commit_index,
                    token_index,
                    CommitParseErrorKind::InvalidNumstat,
                )
            })?;
            if path.is_empty() {
                if token_index + 2 >= tokens.len()
                    || tokens[token_index + 1].is_empty()
                    || tokens[token_index + 2].is_empty()
                {
                    return Err(CommitParseError::new(
                        commit_index,
                        token_index,
                        CommitParseErrorKind::MissingRenamePath,
                    ));
                }
                add_numstat(
                    &mut commit,
                    header,
                    Some(tokens[token_index + 1]),
                    tokens[token_index + 2],
                    options,
                    commit_index,
                    token_index,
                )?;
                token_index += 3;
            } else {
                add_numstat(
                    &mut commit,
                    header,
                    None,
                    path,
                    options,
                    commit_index,
                    token_index,
                )?;
                token_index += 1;
            }
        }
        commits.push(commit);
    }
    Ok(commits)
}

#[cfg(test)]
#[path = "commits_tests.rs"]
mod tests;
