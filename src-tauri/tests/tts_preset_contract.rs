use std::collections::{HashMap, HashSet};

#[path = "../src/tts_dictionary/preset_matcher.rs"]
mod preset_matcher;

#[test]
fn bundled_presets_are_unique_and_keep_ambiguous_words_unset() {
    let mut entries = HashMap::new();
    for line in include_str!("../data/tts-presets.tsv").lines() {
        let (written, spoken) = line.split_once('\t').expect("TSV record");
        assert!(!written.is_empty());
        assert!(!spoken.is_empty());
        assert!(
            entries.insert(written, spoken).is_none(),
            "duplicate: {written}"
        );
    }
    assert_eq!(entries.len(), 80_205);
    assert_eq!(preset_matcher::count(), 80_205);
    let initial = preset_matcher::search("", 100, &HashSet::new());
    assert_eq!(initial.len(), 100);
    assert_eq!(initial[0], ("α線".into(), "アルファセン".into()));
    let initial_without_first = preset_matcher::search("", 100, &HashSet::from(["α線".into()]));
    assert_eq!(initial_without_first.len(), 100);
    assert_eq!(initial_without_first[0].0, "β線");
    assert!(preset_matcher::search("銀行", 50, &HashSet::new())
        .contains(&("銀行".into(), "ギンコウ".into())));
    assert_eq!(preset_matcher::lookup("銀行").as_deref(), Some("ギンコウ"));
    assert!(
        !preset_matcher::search("銀行", 50, &HashSet::from(["銀行".into()]))
            .iter()
            .any(|entry| entry.0 == "銀行")
    );
    assert_eq!(entries.get("銀行"), Some(&"ギンコウ"));
    assert!(!entries.contains_key("明日"));
    assert!(!entries.contains_key("重複"));
    assert_eq!(preset_matcher::apply("銀行", &[]), "ギンコウ");
    assert_eq!(preset_matcher::apply("明日", &[]), "明日");
    assert_eq!(
        preset_matcher::apply("銀行", &[("銀行".into(), "バンク".into())]),
        "バンク"
    );
    assert_eq!(
        preset_matcher::apply("■銀行", &[("■".into(), String::new())]),
        "ギンコウ"
    );
    assert_eq!(preset_matcher::apply("銀", &[]), "銀");
    assert_eq!(preset_matcher::ready_prefix_len("銀", &[]), 0);
    assert_eq!(
        preset_matcher::ready_prefix_len("確認します。銀", &[]),
        "確認します。".len()
    );
    assert_eq!(
        preset_matcher::ready_prefix_len("銀行です。", &[]),
        "銀行です。".len()
    );
    let custom = [("OpenAI".into(), "オープンエーアイ".into())];
    assert_eq!(
        preset_matcher::ready_prefix_len("それはOpen", &custom),
        "それは".len()
    );
    assert_eq!(preset_matcher::apply("OpenAI", &custom), "オープンエーアイ");
    let overlapping = [
        ("ABCD".into(), "エービーシーディー".into()),
        ("BCDE".into(), "別読み".into()),
    ];
    assert_eq!(preset_matcher::ready_prefix_len("ABCD", &overlapping), 4);
    let mut pending = "私は銀".to_string();
    let ready = preset_matcher::ready_prefix_len(&pending, &[]);
    let mut sent = preset_matcher::apply(&pending[..ready], &[]);
    pending = pending[ready..].to_string();
    pending.push_str("行に行きます。");
    sent.push_str(&preset_matcher::apply(&pending, &[]));
    assert_eq!(sent, "私はギンコウに行きます。");
}
