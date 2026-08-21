use rusqlite::types::Value;

pub(super) fn bounded_limit(limit: usize, maximum: usize) -> usize {
    limit.clamp(1, maximum)
}

pub(super) fn push_filter(
    sql: &mut String,
    values: &mut Vec<Value>,
    expression: &str,
    value: impl Into<Value>,
) {
    sql.push_str(" AND ");
    sql.push_str(expression);
    values.push(value.into());
}

pub(super) fn text_cursor(
    sql: &mut String,
    values: &mut Vec<Value>,
    columns: (&str, &str),
    cursor: Option<(&str, i64)>,
    asc: bool,
) {
    if let Some((sort, id)) = cursor {
        let op = if asc { ">" } else { "<" };
        push_filter(
            sql,
            values,
            &format!(
                "({0} {op} ? OR ({0} = ? AND {1} {op} ?))",
                columns.0, columns.1
            ),
            sort.to_owned(),
        );
        values.push(sort.to_owned().into());
        values.push(id.into());
    }
}

pub(super) fn int_cursor(
    sql: &mut String,
    values: &mut Vec<Value>,
    columns: (&str, &str),
    cursor: Option<(i64, i64)>,
    asc: bool,
) {
    if let Some((sort, id)) = cursor {
        let op = if asc { ">" } else { "<" };
        push_filter(
            sql,
            values,
            &format!(
                "({0} {op} ? OR ({0} = ? AND {1} {op} ?))",
                columns.0, columns.1
            ),
            sort,
        );
        values.push(sort.into());
        values.push(id.into());
    }
}
