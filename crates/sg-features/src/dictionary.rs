//! Feature name dictionary — the `FeatureDictionary.pfs` data from ViVe (GPL-3.0, @thebookisclosed).

use std::collections::HashMap;
use std::sync::OnceLock;

static RAW: &[u8] = include_bytes!("../data/FeatureDictionary.pfs");
static EMBEDDED: OnceLock<Dictionary> = OnceLock::new();

/// Looks up names and IDs. Source lines have the form `Name,ID`.
pub struct Dictionary {
    /// (id, name, lowercase name) in file order.
    rows: Vec<(u32, &'static str, String)>,
    by_id: HashMap<u32, usize>,
    by_name: HashMap<String, usize>,
}

impl Dictionary {
    /// The embedded dictionary, lazily loaded once.
    pub fn embedded() -> &'static Dictionary {
        EMBEDDED.get_or_init(|| {
            let text = std::str::from_utf8(RAW).unwrap_or("");
            Dictionary::parse(text.trim_start_matches('\u{feff}'))
        })
    }

    fn parse(text: &'static str) -> Dictionary {
        let mut rows = Vec::new();
        let mut by_id = HashMap::new();
        let mut by_name = HashMap::new();
        for line in text.lines() {
            let Some((name, id)) = line.trim().rsplit_once(',') else {
                continue;
            };
            let name = name.trim();
            let Ok(id) = id.trim().parse::<u32>() else {
                continue;
            };
            if name.is_empty() {
                continue;
            }
            let idx = rows.len();
            rows.push((id, name, name.to_lowercase()));
            by_id.entry(id).or_insert(idx);
            by_name.entry(name.to_lowercase()).or_insert(idx);
        }
        Dictionary {
            rows,
            by_id,
            by_name,
        }
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Every ID that has a name (each ID once, unspecified order).
    pub fn ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.by_id.keys().copied()
    }

    pub fn name_of(&self, id: u32) -> Option<&str> {
        self.by_id.get(&id).map(|&i| self.rows[i].1)
    }

    /// Names are case-insensitive.
    pub fn id_of(&self, name: &str) -> Option<u32> {
        self.by_name
            .get(&name.trim().to_lowercase())
            .map(|&i| self.rows[i].0)
    }

    /// Digits only → match IDs (the digit string occurs within the ID); otherwise match part of a name, case-insensitively.
    /// Order: exact match, prefix match, substring match; within the same rank, file order is kept.
    pub fn search(&self, query: &str, limit: usize) -> Vec<(u32, &str)> {
        let q = query.trim().to_lowercase();
        if q.is_empty() || limit == 0 {
            return Vec::new();
        }
        let numeric = q.bytes().all(|b| b.is_ascii_digit());
        let mut hits: Vec<(u8, usize)> = Vec::new();
        for (i, (id, _, lower)) in self.rows.iter().enumerate() {
            let rank = if numeric {
                let s = id.to_string();
                if s == q {
                    0
                } else if s.starts_with(&q) {
                    1
                } else if s.contains(&q) {
                    2
                } else {
                    continue;
                }
            } else if *lower == q {
                0
            } else if lower.starts_with(&q) {
                1
            } else if lower.contains(&q) {
                2
            } else {
                continue;
            };
            hits.push((rank, i));
        }
        hits.sort_by_key(|&(rank, i)| (rank, i));
        hits.into_iter()
            .take(limit)
            .map(|(_, i)| (self.rows[i].0, self.rows[i].1))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dictionary_lookup_both_ways() {
        let d = Dictionary::embedded();
        assert!(d.len() > 10_000);
        assert_eq!(d.name_of(56192424), Some("CameraQI2505"));
        assert_eq!(d.id_of("CameraQI2505"), Some(56192424));
        assert_eq!(d.id_of("cameraqi2505"), Some(56192424));
        assert_eq!(d.name_of(1), None);
        assert_eq!(d.id_of("không-có-thật"), None);
    }

    #[test]
    fn dictionary_search_partial_case_insensitive() {
        let d = Dictionary::embedded();
        let hits = d.search("cameraqi25", 10);
        assert!(
            hits.iter()
                .any(|&(id, n)| id == 56192424 && n == "CameraQI2505")
        );
        assert_eq!(hits, d.search("CAMERAQI25", 10));
        // Searching by number matches IDs.
        let by_id = d.search("56192424", 5);
        assert_eq!(by_id.first().map(|x| x.0), Some(56192424));
        // Limit and empty string.
        assert!(d.search("a", 3).len() <= 3);
        assert!(d.search("   ", 3).is_empty());
    }

    #[test]
    fn dictionary_search_is_fast() {
        let d = Dictionary::embedded();
        let _ = d.len(); // load first, so only the search is timed
        let t = std::time::Instant::now();
        let n = d.search("e", 50).len() + d.search("spotlight", 50).len();
        let ms = t.elapsed().as_millis();
        eprintln!("search: {n} kết quả trong {ms} ms (2 lần tìm)");
        assert!(ms < 500, "search quá chậm: {ms} ms");
    }
}
