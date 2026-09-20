use textlint_v8::{Textlint, third_party_notices};

#[test]
fn reports_standalone_and_preset_rules() {
    let mut textlint = Textlint::new().unwrap();
    let result = textlint
        .lint(
            "# 1. “見出し” 😀\n\n*強調*\n\n---\n\n## 次\n\n- 1. 箇書き\n\n特殊　空白\n\n無効な制御文字\u{b}\n\n革命的な技術です。\n\nこれは見ることができないわけではない。\n",
            "sample.md",
        )
        .unwrap();
    let ids: Vec<_> = result
        .messages
        .iter()
        .map(|message| message.rule_id.as_str())
        .collect();

    assert!(ids.contains(&"@0x6b/no-emoji"));
    assert!(ids.contains(&"@0x6b/no-emphasis"));
    assert!(ids.contains(&"@0x6b/no-hr-before-heading"));
    assert!(ids.contains(&"@0x6b/no-numbered-headings-and-bullets"));
    assert!(ids.contains(&"@0x6b/no-smart-quotes"));
    assert!(ids.contains(&"@0x6b/normalize-whitespaces"));
    assert!(ids.contains(&"@textlint-ja/ai-writing/no-ai-hype-expressions"));
    assert!(ids.contains(&"@textlint-rule/no-invalid-control-character"));
    assert!(ids.contains(&"ja-technical-writing/no-double-negative-ja"));
    let numbered_messages: Vec<_> = result
        .messages
        .iter()
        .filter(|message| message.rule_id == "@0x6b/no-numbered-headings-and-bullets")
        .collect();
    assert_eq!(numbered_messages.len(), 1);
    assert_eq!(numbered_messages[0].line, 9);

    let second_result = textlint
        .lint("試したが失敗したが、再試行した。", "second.md")
        .unwrap();
    assert!(second_result.messages.iter().any(|message| {
        message.rule_id == "ja-technical-writing/no-doubled-conjunctive-particle-ga"
    }));
}

#[test]
fn fixes_text_and_formats_results() {
    let mut textlint = Textlint::new().unwrap();
    let fixed = textlint.fix("特殊　空白", "sample.md").unwrap();

    assert_eq!(fixed.output, "特殊 空白");
    assert!(
        fixed
            .applying_messages
            .iter()
            .any(|message| message.rule_id == "@0x6b/normalize-whitespaces")
    );

    let json = textlint.format_fix_results(&[fixed], "json").unwrap();
    assert!(json.contains("\"output\":\"特殊 空白\""));

    let lint = textlint.lint("本文 😀", "sample.md").unwrap();
    let stylish = textlint.format_lint_results(&[lint], "stylish").unwrap();
    assert!(stylish.contains("@0x6b/no-emoji"));
}

#[test]
fn embeds_third_party_notices() {
    let notices = third_party_notices();
    assert!(notices.contains("kuromoji 0.1.2"));
    assert!(notices.contains("@textlint/kernel 15.8.0"));
    assert!(notices.contains("mecab-ipadic-2.7.0-20070801"));
}
