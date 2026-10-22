// Lone surrogates cannot enter Rust strings. Every micromark and remark-cjk-friendly
// class treats a lone surrogate code unit and a BMP private-use character alike
// ("other"), and both are one UTF-16 unit, so U+D800–DFFF cross as U+E000–E7FF and
// positions agree.
const LONE_SURROGATE =
  /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/g;
const TRANSPORT_RANGE = /[-]/u;
const TRANSPORTED = /[-]/gu;

/** The transport source, or `null` when the answer already uses the transport range. */
export function encodeLoneSurrogates(
  source: string
): { source: string; mapped: boolean } | null {
  if (source.isWellFormed()) {
    return { source, mapped: false };
  }
  if (TRANSPORT_RANGE.test(source)) {
    return null;
  }
  return {
    source: source.replace(LONE_SURROGATE, (unit) =>
      String.fromCharCode(unit.charCodeAt(0) - 0xd8_00 + 0xe0_00)
    ),
    mapped: true,
  };
}

export function restoreLoneSurrogates(value: string): string {
  return value.replace(TRANSPORTED, (unit) =>
    String.fromCharCode(unit.charCodeAt(0) - 0xe0_00 + 0xd8_00)
  );
}
