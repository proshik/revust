//! Stable within a build, derived colors for archives without a saved palette.
use std::hash::{DefaultHasher, Hash, Hasher};
pub fn derived(name: &str) -> String {
    if name.is_empty() {
        return String::new();
    }
    let mut hash = DefaultHasher::new();
    name.hash(&mut hash);
    let palette = [6, 5, 2, 3, 4, 1, 14, 13];
    palette[(hash.finish() % palette.len() as u64) as usize].to_string()
}
