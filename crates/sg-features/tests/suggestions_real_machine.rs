//! Condition (2) of the suggestions list: every id in `suggestions.toml` must have a configuration on a real machine.
//! READ-ONLY (`query_all`/`query`), no write function is called. Run manually:
//! `cargo test -p sg-features --test suggestions_real_machine -- --ignored --nocapture`.

use sg_features::{platform, Dictionary, Store};

const SUGGESTIONS_TOML: &str = include_str!("../data/suggestions.toml");

#[derive(serde::Deserialize)]
struct S {
    id: u32,
    name: String,
}

#[derive(serde::Deserialize)]
struct F {
    s: Vec<S>,
}

#[test]
#[ignore = "đọc ntdll thật trên máy này (chỉ đọc)"]
fn suggestions_have_configuration_on_this_machine() {
    let f: F = toml::from_str(SUGGESTIONS_TOML).unwrap();
    let all = platform::query_all(Store::Runtime).expect("đọc toàn bộ cấu hình");
    println!("tổng cấu hình runtime: {}", all.len());
    let dict = Dictionary::embedded();
    for s in &f.s {
        let in_all: Vec<_> = all.iter().filter(|c| c.id == s.id).collect();
        let one = platform::query(s.id, Store::Runtime);
        println!(
            "{} {} (từ điển: {:?}) -> query_all: {:?}; query: {:?}",
            s.id,
            s.name,
            dict.name_of(s.id),
            in_all.iter().map(|c| (c.priority, c.state)).collect::<Vec<_>>(),
            one.map(|c| (c.priority, c.state)),
        );
        assert!(
            !in_all.is_empty() || one.is_some(),
            "{} không có cấu hình trên máy này",
            s.id
        );
    }
}
