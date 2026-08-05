pub const INVENTORY_SCHEMA: &str = "cortex.homelab_inventory.v1";
pub const MAP_SCHEMA: &str = "cortex.homelab_map.v2";
pub const MAX_RAW_ARTIFACT_BYTES: usize = 512 * 1024;
pub const MAX_HTTP_BODY_BYTES: usize = 512 * 1024;
pub const MAX_COMMAND_OUTPUT_BYTES: usize = 256 * 1024;
pub const MAX_RAW_BATCH_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_JSON_DEPTH: usize = 12;
pub const MAX_ARRAY_ENTRIES: usize = 200;
pub const MAX_SECTION_ITEMS: usize = 250;
pub const DEFAULT_COLLECTION_DEADLINE_SECS: u64 = 45;
pub const DEFAULT_COLLECTOR_DEADLINE_SECS: u64 = 12;
pub const DEFAULT_PROBE_DEADLINE_SECS: u64 = 5;

pub fn cap_vec<T>(items: &mut Vec<T>, limit: usize) -> bool {
    if items.len() <= limit {
        return false;
    }
    items.truncate(limit);
    true
}

pub fn truncate_text(input: &str, max_bytes: usize) -> (String, bool) {
    if input.len() <= max_bytes {
        return (input.to_string(), false);
    }
    let mut end = max_bytes;
    while !input.is_char_boundary(end) {
        end -= 1;
    }
    (input[..end].to_string(), true)
}

#[cfg(test)]
#[path = "limits_tests.rs"]
mod tests;
