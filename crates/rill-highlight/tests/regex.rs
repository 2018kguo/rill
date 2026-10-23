//! ECMAScript semantics the engine must reproduce, with results taken from V8
//! (`new RegExp(pattern, "d" + flags).exec(input)` with `lastIndex = start`).

use rill_highlight::regex::{EmulationOptions, JsRegExp, Matcher};

fn exec(pattern: &str, flags: &str, input: &str, start: usize) -> Option<Vec<i32>> {
    let regex = JsRegExp::new(pattern, flags, &EmulationOptions::default()).unwrap();
    let input: Vec<u16> = input.encode_utf16().collect();
    let (mut raw, mut out) = (Vec::new(), Vec::new());
    regex.exec(&mut Matcher::default(), &input, start, &mut raw, &mut out).then_some(out)
}

#[track_caller]
fn check(pattern: &str, flags: &str, input: &str, start: usize, want: Option<&[i32]>) {
    assert_eq!(exec(pattern, flags, input, start).as_deref(), want, "/{pattern}/{flags} on {input:?} from {start}");
}

#[test]
fn quantified_groups_reset_their_captures() {
    check("(?:(a)|b)+", "v", "ab", 0, Some(&[0, 2, -1, -1]));
    check("(a*)+", "v", "b", 0, Some(&[0, 0, 0, 0]));
    check("(?:a|ab)(?:c|bcd)(d*)", "v", "abcd", 0, Some(&[0, 4, 4, 4]));
}

#[test]
fn lookbehind_matches_right_to_left() {
    check("(?<=(\\d+)(\\d+))$", "v", "1053", 0, Some(&[4, 4, 0, 1, 1, 4]));
    check("(?<!a)b", "v", "ab cb", 0, Some(&[4, 5]));
}

#[test]
fn backreferences() {
    check("(?<a>x)|(?<a>y)\\k<a>", "v", "yy", 0, Some(&[0, 2, -1, -1, 0, 1]));
    check("(a)|\\1b", "v", "b", 0, Some(&[0, 1, -1, -1]));
    check("(?=(ab|a))\\1c", "v", "abc", 0, Some(&[0, 3, 0, 2]));
    check("^(?:(?=(\\s*))\\1)x", "v", "  x", 0, Some(&[0, 3, 0, 2]));
}

#[test]
fn lazy_quantifiers() {
    check("x*?y", "v", "xxy", 0, Some(&[0, 3]));
    check("a{2,3}?", "v", "aaaa", 0, Some(&[0, 2]));
}

#[test]
fn unicode_sets() {
    check("[\\p{L}--[a-z]]", "v", "aB", 0, Some(&[1, 2]));
    check("[[a-z]&&[^aeiou]]+", "v", "abcde", 0, Some(&[1, 4]));
}

#[test]
fn starts_inside_a_surrogate_pair_move_back() {
    check(".", "gv", "a\u{1F600}b", 2, Some(&[1, 3]));
}

#[test]
fn case_folding() {
    check("(?i:\\w)", "v", "\u{17F}", 0, Some(&[0, 1]));
    check("\\b", "iv", "\u{17F}", 0, Some(&[0, 0]));
    check("(?i:\\b)", "v", "\u{17F}", 0, Some(&[0, 0]));
    check("(?i:\\W)", "v", "\u{17F}", 0, None);
    check("[^a]", "iv", "A", 0, None);
    check("\\P{Lu}", "iv", "A", 0, None);
    check("(?i:\\p{Lu})", "v", "s", 0, Some(&[0, 1]));
    check("(?i:[\\u212A])", "v", "k", 0, Some(&[0, 1]));
    check("\u{17F}", "iv", "S", 0, Some(&[0, 1]));
}

#[test]
fn modifier_groups_keep_v8s_latin1_quirk() {
    // Under `i` from a modifier group only, a literal above U+00FF does not match
    // its case variants in Latin-1.
    check("(?i:\u{17F})", "v", "s", 0, None);
    check("(?i:\\u{17F})", "v", "s", 0, None);
    check("(?i:\u{212A})", "v", "k", 0, None);
    check("(?i:k)", "v", "\u{212A}", 0, Some(&[0, 1]));
    check("(?i:\u{3BC})", "v", "\u{39C}", 0, Some(&[0, 1]));
    check("(?i:\u{3BC})", "v", "\u{B5}", 0, None);
    check("(?i:[\u{17F}])", "v", "s", 0, Some(&[0, 1]));
    check("(?i:\u{17F})", "iv", "s", 0, Some(&[0, 1]));
}
