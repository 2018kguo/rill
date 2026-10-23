//! Code point sets: sorted, disjoint, inclusive ranges, plus the Unicode data the
//! regex engine needs (property tables, simple case folding).

use super::unicode_tables::{CASE_FOLDS, PROPERTIES};

pub const MAX_CODE_POINT: u32 = 0x10FFFF;

/// A set of code points as sorted, disjoint, non-adjacent inclusive ranges.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct CharSet {
    ranges: Vec<(u32, u32)>,
}

impl CharSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_ranges(ranges: &[(u32, u32)]) -> Self {
        let mut set = Self { ranges: ranges.to_vec() };
        set.normalize();
        set
    }

    pub fn single(c: u32) -> Self {
        Self { ranges: vec![(c, c)] }
    }

    pub fn all() -> Self {
        Self { ranges: vec![(0, MAX_CODE_POINT)] }
    }

    pub fn ranges(&self) -> &[(u32, u32)] {
        &self.ranges
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// The only member, when the set has exactly one.
    pub fn as_single(&self) -> Option<u32> {
        match self.ranges.as_slice() {
            [(lo, hi)] if lo == hi => Some(*lo),
            _ => None,
        }
    }

    pub fn len(&self) -> u64 {
        self.ranges.iter().map(|(lo, hi)| u64::from(hi - lo) + 1).sum()
    }

    pub fn contains(&self, c: u32) -> bool {
        self.ranges
            .binary_search_by(|&(lo, hi)| {
                if hi < c {
                    std::cmp::Ordering::Less
                } else if lo > c {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .is_ok()
    }

    pub fn add_range(&mut self, lo: u32, hi: u32) {
        self.union(&CharSet { ranges: vec![(lo, hi)] });
    }

    pub fn add(&mut self, c: u32) {
        self.add_range(c, c);
    }

    /// Merges `other` in (both are normalized, so a linear merge suffices).
    pub fn union(&mut self, other: &CharSet) {
        if other.ranges.is_empty() {
            return;
        }
        if self.ranges.is_empty() {
            self.ranges = other.ranges.clone();
            return;
        }
        let (a, b) = (&self.ranges, &other.ranges);
        let mut merged: Vec<(u32, u32)> = Vec::with_capacity(a.len() + b.len());
        let (mut i, mut j) = (0, 0);
        while i < a.len() || j < b.len() {
            let next = if j == b.len() || (i < a.len() && a[i].0 <= b[j].0) {
                i += 1;
                a[i - 1]
            } else {
                j += 1;
                b[j - 1]
            };
            match merged.last_mut() {
                Some(last) if next.0 <= last.1.saturating_add(1) => last.1 = last.1.max(next.1),
                _ => merged.push(next),
            }
        }
        self.ranges = merged;
    }

    pub fn complement(&self) -> CharSet {
        let mut ranges = Vec::with_capacity(self.ranges.len() + 1);
        let mut next = 0u32;
        for &(lo, hi) in &self.ranges {
            if lo > next {
                ranges.push((next, lo - 1));
            }
            next = hi + 1;
        }
        if next <= MAX_CODE_POINT {
            ranges.push((next, MAX_CODE_POINT));
        }
        CharSet { ranges }
    }

    pub fn intersect(&self, other: &CharSet) -> CharSet {
        let mut ranges = Vec::new();
        let (mut i, mut j) = (0, 0);
        while i < self.ranges.len() && j < other.ranges.len() {
            let (a_lo, a_hi) = self.ranges[i];
            let (b_lo, b_hi) = other.ranges[j];
            let lo = a_lo.max(b_lo);
            let hi = a_hi.min(b_hi);
            if lo <= hi {
                ranges.push((lo, hi));
            }
            if a_hi < b_hi {
                i += 1;
            } else {
                j += 1;
            }
        }
        CharSet { ranges }
    }

    pub fn subtract(&self, other: &CharSet) -> CharSet {
        self.intersect(&other.complement())
    }

    /// Adds every code point that is case-equivalent to a member (simple case
    /// folding, as V8 applies it under the `i` flag with `u` or `v`).
    pub fn case_closure(&self) -> CharSet {
        let folding = folding();
        let mut extra: Vec<(u32, u32)> = Vec::new();
        // Walk the members that have case variants (sorted) against the ranges.
        let mut range = 0;
        for &(c, class) in &folding.members {
            while range < self.ranges.len() && self.ranges[range].1 < c {
                range += 1;
            }
            if range == self.ranges.len() {
                break;
            }
            if self.ranges[range].0 <= c {
                extra.extend(folding.classes[class as usize].iter().map(|&m| (m, m)));
            }
        }
        if extra.is_empty() {
            return self.clone();
        }
        let mut set = self.clone();
        set.union(&CharSet::from_ranges(&extra));
        set
    }

    fn normalize(&mut self) {
        self.ranges.sort_unstable();
        let mut merged: Vec<(u32, u32)> = Vec::with_capacity(self.ranges.len());
        for &(lo, hi) in &self.ranges {
            if let Some(last) = merged.last_mut() {
                if lo <= last.1.saturating_add(1) {
                    last.1 = last.1.max(hi);
                    continue;
                }
            }
            merged.push((lo, hi));
        }
        self.ranges = merged;
    }
}

/// Case folding classes with more than one member.
struct Folding {
    /// Every code point in a non-trivial class, sorted, with its class index.
    members: Vec<(u32, u32)>,
    classes: Vec<Vec<u32>>,
}

fn folding() -> &'static Folding {
    static FOLDING: std::sync::OnceLock<Folding> = std::sync::OnceLock::new();
    FOLDING.get_or_init(|| {
        let mut by_representative: std::collections::BTreeMap<u32, Vec<u32>> = std::collections::BTreeMap::new();
        for &(c, rep) in CASE_FOLDS {
            let class = by_representative.entry(rep).or_insert_with(|| vec![rep]);
            class.push(c);
        }
        let classes: Vec<Vec<u32>> = by_representative.into_values().collect();
        let mut members: Vec<(u32, u32)> = classes.iter().enumerate().flat_map(|(i, class)| class.iter().map(move |&c| (c, i as u32))).collect();
        members.sort_unstable();
        Folding { members, classes }
    })
}

/// The representative of `c`'s simple case folding class (its smallest member).
pub fn fold(c: u32) -> u32 {
    if c < 0x80 {
        // ASCII letters fold to their uppercase form, the smallest member of the class.
        return if (0x61..=0x7A).contains(&c) { c - 0x20 } else { c };
    }
    match CASE_FOLDS.binary_search_by_key(&c, |&(from, _)| from) {
        Ok(index) => CASE_FOLDS[index].1,
        Err(_) => c,
    }
}

/// Looks up a property escape by the name inside `\p{...}`. Returns `None` for
/// properties the tables do not carry.
pub fn property(name: &str) -> Option<CharSet> {
    let canonical = canonical_property_name(name)?;
    let (_, encoded) = PROPERTIES.iter().find(|(key, _)| *key == canonical)?;
    let mut ranges = Vec::new();
    let mut bytes = encoded.iter();
    let mut read = || -> Option<u32> {
        let mut value = 0u32;
        let mut shift = 0;
        loop {
            let byte = *bytes.next()?;
            value |= u32::from(byte & 0x7F) << shift;
            if byte & 0x80 == 0 {
                return Some(value);
            }
            shift += 7;
        }
    };
    let mut next = 0u32;
    while let Some(gap) = read() {
        let lo = next + gap;
        let hi = lo + read()?;
        ranges.push((lo, hi));
        next = hi + 1;
    }
    Some(CharSet { ranges })
}

fn canonical_property_name(name: &str) -> Option<String> {
    if let Some((key, value)) = name.split_once('=') {
        return match key {
            "General_Category" | "gc" => general_category(value).map(|v| format!("gc={v}")),
            "Script" | "sc" => script(value).map(|v| format!("sc={v}")),
            "Script_Extensions" | "scx" => script(value).map(|v| format!("scx={v}")),
            _ => None,
        };
    }
    if let Some(value) = general_category(name) {
        return Some(format!("gc={value}"));
    }
    let binary = match name {
        "White_Space" | "space" => "White_Space",
        "Alphabetic" | "Alpha" => "Alphabetic",
        "ASCII_Hex_Digit" | "AHex" => "ASCII_Hex_Digit",
        "Uppercase" | "Upper" => "Uppercase",
        "Lowercase" | "Lower" => "Lowercase",
        "Any" => "Any",
        "ASCII" => "ASCII",
        "XID_Start" | "XIDS" => "XID_Start",
        "XID_Continue" | "XIDC" => "XID_Continue",
        _ => return None,
    };
    Some(binary.to_string())
}

fn general_category(value: &str) -> Option<&'static str> {
    const ALIASES: &[(&str, &str)] = &[
        ("L", "L"), ("Letter", "L"), ("LC", "LC"), ("Cased_Letter", "LC"),
        ("Lu", "Lu"), ("Uppercase_Letter", "Lu"), ("Ll", "Ll"), ("Lowercase_Letter", "Ll"),
        ("Lt", "Lt"), ("Titlecase_Letter", "Lt"), ("Lm", "Lm"), ("Modifier_Letter", "Lm"),
        ("Lo", "Lo"), ("Other_Letter", "Lo"), ("M", "M"), ("Mark", "M"), ("Combining_Mark", "M"),
        ("Mn", "Mn"), ("Nonspacing_Mark", "Mn"), ("Mc", "Mc"), ("Spacing_Mark", "Mc"),
        ("Me", "Me"), ("Enclosing_Mark", "Me"), ("N", "N"), ("Number", "N"),
        ("Nd", "Nd"), ("Decimal_Number", "Nd"), ("digit", "Nd"), ("Nl", "Nl"), ("Letter_Number", "Nl"),
        ("No", "No"), ("Other_Number", "No"), ("P", "P"), ("Punctuation", "P"), ("punct", "P"),
        ("Pc", "Pc"), ("Connector_Punctuation", "Pc"), ("Pd", "Pd"), ("Dash_Punctuation", "Pd"),
        ("Ps", "Ps"), ("Open_Punctuation", "Ps"), ("Pe", "Pe"), ("Close_Punctuation", "Pe"),
        ("Pi", "Pi"), ("Initial_Punctuation", "Pi"), ("Pf", "Pf"), ("Final_Punctuation", "Pf"),
        ("Po", "Po"), ("Other_Punctuation", "Po"), ("S", "S"), ("Symbol", "S"),
        ("Sm", "Sm"), ("Math_Symbol", "Sm"), ("Sc", "Sc"), ("Currency_Symbol", "Sc"),
        ("Sk", "Sk"), ("Modifier_Symbol", "Sk"), ("So", "So"), ("Other_Symbol", "So"),
        ("Z", "Z"), ("Separator", "Z"), ("Zs", "Zs"), ("Space_Separator", "Zs"),
        ("Zl", "Zl"), ("Line_Separator", "Zl"), ("Zp", "Zp"), ("Paragraph_Separator", "Zp"),
        ("C", "C"), ("Other", "C"), ("Cc", "Cc"), ("Control", "Cc"), ("cntrl", "Cc"),
        ("Cf", "Cf"), ("Format", "Cf"), ("Cs", "Cs"), ("Surrogate", "Cs"),
        ("Co", "Co"), ("Private_Use", "Co"), ("Cn", "Cn"), ("Unassigned", "Cn"),
    ];
    ALIASES.iter().find(|(alias, _)| *alias == value).map(|(_, canonical)| *canonical)
}

fn script(value: &str) -> Option<&'static str> {
    const ALIASES: &[(&str, &str)] = &[
        ("Greek", "Greek"), ("Grek", "Greek"), ("Han", "Han"), ("Hani", "Han"),
        ("Hangul", "Hangul"), ("Hang", "Hangul"), ("Katakana", "Katakana"), ("Kana", "Katakana"),
        ("Hiragana", "Hiragana"), ("Hira", "Hiragana"),
    ];
    ALIASES.iter().find(|(alias, _)| *alias == value).map(|(_, canonical)| *canonical)
}
