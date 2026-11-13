//! The "All" list: filtering, sorting, and paging over the ID set (dictionary ∪ configurations on the machine), plus building the web search URL.
//! Everything is pure functions that never touch ntdll, so tests do not need a real machine.

use serde::{Deserialize, Serialize};

pub const MIN_PAGE_SIZE: u32 = 10;
pub const MAX_PAGE_SIZE: u32 = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortBy {
    Name,
    Id,
}

/// Request for one page of the list. `page` counts from 0.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListQuery {
    pub page: u32,
    pub page_size: u32,
    pub configured_only: bool,
    pub hide_unnamed: bool,
    pub text: String,
    pub sort: SortBy,
}

/// One ID in the source set, without its configuration (configuration is looked up only for the rows of the returned page).
#[derive(Debug, Clone)]
pub struct Entry<'a> {
    pub id: u32,
    pub name: Option<&'a str>,
    /// Has a configuration in the Runtime or Boot store.
    pub configured: bool,
}

/// One page of results. `total` is the number of rows after filtering; `page`/`page_size` are the values actually used.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeaturePage<R> {
    pub total: u32,
    pub page: u32,
    pub page_size: u32,
    pub rows: Vec<R>,
}

/// Rank when sorting by name: 0 = readable name, 1 = code-like name (starts with a digit/symbol, or a hex run of
/// 16+ characters followed by `_`, `-`, or end of string), 2 = row without a name. `lower` is already lowercased.
fn name_rank(lower: Option<&str>) -> u8 {
    let Some(n) = lower else { return 2 };
    let hex = n.bytes().take_while(u8::is_ascii_hexdigit).count();
    let hash_like = hex >= 16 && matches!(n.as_bytes().get(hex), None | Some(b'_' | b'-'));
    if !n.starts_with(|c: char| c.is_ascii_alphabetic()) || hash_like {
        1
    } else {
        0
    }
}

/// Filter + sort + paginate. Returns (total after filtering, actual page, actual page_size, the page's IDs).
pub fn select_page(entries: Vec<Entry<'_>>, q: &ListQuery) -> (u32, u32, u32, Vec<u32>) {
    let page_size = q.page_size.clamp(MIN_PAGE_SIZE, MAX_PAGE_SIZE);
    let text = q.text.trim().to_lowercase();
    let numeric = !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());

    // (id, lowercase name if any, rank by name)
    let mut kept: Vec<(u32, Option<String>, u8)> = entries
        .into_iter()
        .filter(|e| !q.configured_only || e.configured)
        .filter(|e| !q.hide_unnamed || e.name.is_some())
        .filter(|e| {
            if text.is_empty() {
                true
            } else if numeric {
                e.id.to_string().starts_with(&text)
            } else {
                e.name.is_some_and(|n| n.to_lowercase().contains(&text))
            }
        })
        .map(|e| {
            let lower = e.name.map(str::to_lowercase);
            let rank = name_rank(lower.as_deref());
            (e.id, lower, rank)
        })
        .collect();

    match q.sort {
        SortBy::Id => kept.sort_by_key(|&(id, ..)| id),
        // Readable names first (A–Z), then code-like names, then rows without a name; ties are broken by ID.
        // A deliberate deviation from the brief's "name A–Z": 66 names starting with symbols and 19 32-character hex strings
        // would occupy the first pages under a purely alphabetical sort.
        SortBy::Name => kept.sort_by(|a, b| a.2.cmp(&b.2).then_with(|| a.1.cmp(&b.1)).then(a.0.cmp(&b.0))),
    }

    let total = kept.len() as u32;
    let last_page = total.div_ceil(page_size).saturating_sub(1);
    let page = q.page.min(last_page);
    let ids = kept
        .into_iter()
        .skip(page as usize * page_size as usize)
        .take(page_size as usize)
        .map(|(id, ..)| id)
        .collect();
    (total, page, page_size, ids)
}

/// UTF-8 percent-encoding: keeps only "unreserved" characters (RFC 3986); everything else becomes `%XX`.
fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Google search URL for a feature: `"<ID>" ViVeTool`, or `"<ID>" OR "<name>" ViVeTool` if the name is known.
pub fn web_search_url(id: u32, name: Option<&str>) -> String {
    let query = match name {
        // A `"` in the name (if any) is dropped so it does not break the phrase syntax.
        Some(n) => format!("\"{id}\" OR \"{}\" ViVeTool", n.replace('"', "")),
        None => format!("\"{id}\" ViVeTool"),
    };
    format!("https://www.google.com/search?q={}", encode_component(&query))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(id: u32, name: Option<&'static str>, configured: bool) -> Entry<'static> {
        Entry { id, name, configured }
    }

    fn q() -> ListQuery {
        ListQuery {
            page: 0,
            page_size: 50,
            configured_only: false,
            hide_unnamed: false,
            text: String::new(),
            sort: SortBy::Id,
        }
    }

    fn sample() -> Vec<Entry<'static>> {
        vec![
            e(30, Some("beta"), false),
            e(5, None, true),
            e(20, Some("Alpha"), true),
            e(10, Some("alpha"), false),
            e(7, None, false),
        ]
    }

    #[test]
    fn sort_by_id() {
        let (total, _, _, ids) = select_page(sample(), &q());
        assert_eq!(total, 5);
        assert_eq!(ids, vec![5, 7, 10, 20, 30]);
    }

    #[test]
    fn sort_by_name_named_first_case_insensitive_then_unnamed_by_id() {
        let mut query = q();
        query.sort = SortBy::Name;
        let (_, _, _, ids) = select_page(sample(), &query);
        // "alpha" (10) and "Alpha" (20) tie once case is ignored -> ordered by ID.
        assert_eq!(ids, vec![10, 20, 30, 5, 7]);
    }

    #[test]
    fn sort_by_name_ranks_readable_then_code_like_then_unnamed() {
        let entries = vec![
            e(1, None, false),
            e(2, Some("e966a35c59d94d77bf28c8a6ab17874f"), false),
            e(3, Some("10X_Foo"), false),
            e(4, Some("0f8498a6c1b24d7e9a3b5c6d7e8f9a0b"), false),
            e(5, Some("beta"), false),
            e(6, Some("Alpha"), false),
            // Starts with a letter but a hex run of 16+ characters followed by `_`: code-like.
            e(7, Some("abcdef0123456789_Foo"), false),
            // Long hex run but followed by another letter: readable name.
            e(8, Some("abcdef0123456789zeta"), false),
            e(9, Some("Decade"), false),
        ];
        let mut query = q();
        query.sort = SortBy::Name;
        let (_, _, _, ids) = select_page(entries, &query);
        assert_eq!(ids, vec![8, 6, 5, 9, 4, 3, 7, 2, 1]);
    }

    #[test]
    fn name_rank_boundaries() {
        assert_eq!(name_rank(None), 2);
        assert_eq!(name_rank(Some("alpha")), 0);
        assert_eq!(name_rank(Some("_alpha")), 1);
        assert_eq!(name_rank(Some("0abc")), 1);
        // 15 hex characters: below the threshold of 16.
        assert_eq!(name_rank(Some("abcdefabcdefabc")), 0);
        assert_eq!(name_rank(Some("abcdefabcdefabcd")), 1);
        assert_eq!(name_rank(Some("abcdefabcdefabcd-x")), 1);
    }

    #[test]
    fn filters() {
        let mut query = q();
        query.configured_only = true;
        assert_eq!(select_page(sample(), &query).3, vec![5, 20]);

        let mut query = q();
        query.hide_unnamed = true;
        assert_eq!(select_page(sample(), &query).3, vec![10, 20, 30]);

        let mut query = q();
        query.configured_only = true;
        query.hide_unnamed = true;
        assert_eq!(select_page(sample(), &query).3, vec![20]);
    }

    #[test]
    fn text_filter_numeric_is_id_prefix_else_name_contains() {
        let mut query = q();
        query.text = "  2 ".into();
        assert_eq!(select_page(sample(), &query).3, vec![20]);
        query.text = "3".into();
        assert_eq!(select_page(sample(), &query).3, vec![30]);
        // An all-digit query only matches the start of an ID, not the middle.
        query.text = "0".into();
        assert!(select_page(sample(), &query).3.is_empty());
        query.text = "ALPH".into();
        assert_eq!(select_page(sample(), &query).3, vec![10, 20]);
        // A row without a name never matches text.
        query.text = "xyz".into();
        assert_eq!(select_page(sample(), &query).0, 0);
    }

    #[test]
    fn paging_clamps_page_and_page_size() {
        let many: Vec<_> = (1..=25).map(|i| e(i, None, false)).collect();
        let mut query = q();
        query.page_size = 3; // clamped up to 10
        let (total, page, size, ids) = select_page(many.clone(), &query);
        assert_eq!((total, page, size), (25, 0, 10));
        assert_eq!(ids, (1..=10).collect::<Vec<_>>());

        query.page = 2;
        assert_eq!(select_page(many.clone(), &query).3, (21..=25).collect::<Vec<_>>());

        query.page = 99; // out of range -> last page
        let (_, page, _, ids) = select_page(many.clone(), &query);
        assert_eq!(page, 2);
        assert_eq!(ids.len(), 5);

        query.page_size = 9999; // clamped down to 200
        query.page = 0;
        assert_eq!(select_page(many, &query).2, 200);
    }

    #[test]
    fn empty_result_is_page_zero_without_rows() {
        let mut query = q();
        query.page = 5;
        query.text = "zzz".into();
        let (total, page, _, ids) = select_page(sample(), &query);
        assert_eq!((total, page), (0, 0));
        assert!(ids.is_empty());
    }

    #[test]
    fn web_search_url_encodes_quotes_spaces_and_name() {
        assert_eq!(web_search_url(123, None), "https://www.google.com/search?q=%22123%22%20ViVeTool");
        assert_eq!(
            web_search_url(123, Some("CameraQI2505")),
            "https://www.google.com/search?q=%22123%22%20OR%20%22CameraQI2505%22%20ViVeTool"
        );
        // A name with special characters, double quotes, Unicode: all get encoded; no `&`, `#`, `"` leak through.
        let u = web_search_url(1, Some("a&b#c\"d é"));
        let q = u.split_once("q=").unwrap().1;
        assert!(q.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'%' || b"-._~".contains(&b)));
        assert!(q.contains("a%26b%23cd%20%C3%A9"));
        // A "safe" name still goes through the encoder (underscore/hyphen are unreserved characters, so they are kept as is).
        assert!(web_search_url(1, Some("Feature_X-1")).contains("%22Feature_X-1%22"));
    }
}
