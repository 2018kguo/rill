//! The JSON writing both trees share.

use std::fmt::Write as _;

pub(crate) fn json_string(out: &mut String, value: &str) {
    out.push('"');
    let bytes = value.as_bytes();
    let mut run = 0;
    for (index, &byte) in bytes.iter().enumerate() {
        let escape = match byte {
            b'"' => "\\\"",
            b'\\' => "\\\\",
            b'\n' => "\\n",
            b'\r' => "\\r",
            b'\t' => "\\t",
            0..=0x1f => "",
            _ => continue,
        };
        out.push_str(&value[run..index]);
        if escape.is_empty() {
            let _ = write!(out, "\\u{byte:04x}");
        } else {
            out.push_str(escape);
        }
        run = index + 1;
    }
    out.push_str(&value[run..]);
    out.push('"');
}

pub(crate) fn json_u32(out: &mut String, mut value: u32) {
    let mut digits = [0u8; 10];
    let mut at = digits.len();
    loop {
        at -= 1;
        digits[at] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    out.extend(digits[at..].iter().map(|&digit| digit as char));
}
