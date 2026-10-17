//! Size-bounded document selection with a plain inert fallback.
use crate::wrap;
pub fn render(
    source: &str,
    width: usize,
    indent: usize,
    full: impl FnOnce() -> Vec<String>,
) -> Vec<String> {
    if source.len() <= 65536 && width > indent {
        return full()
            .iter()
            .flat_map(|row| {
                wrap::text(row)
                    .split('\n')
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .collect();
    }
    let padding = " ".repeat(indent);
    source
        .split('\n')
        .flat_map(|line| wrap::wrap(&padding, line, width.max(indent + 1)))
        .collect()
}
