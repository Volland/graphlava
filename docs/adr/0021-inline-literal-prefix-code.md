---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# Inline literals use a prefix code that gives doubles 62 payload bits

## Context and Problem Statement

[ADR-0011](0011-id-layout.md) gives every inline literal a 4-bit tag, a 4-bit subtype and 56 payload bits. Only doubles with eight zero low mantissa bits fit, so most computed doubles (0.1, similarity scores, probabilities) become dictionary literals: not order-preserving, not range-scannable, and garbage on every overwrite. Kind tags `0x9`..`0xF` are unused. The layout is frozen at v0, so it must be fixed now.

## Considered Options

<option id="prefix-code" outcome="chosen">

Bit 63 set marks an inline literal (tags `0x8`..`0xF`). Bits 63..62 = `10`: an inline double with a 62-bit payload (sign, 9-bit rebiased exponent, 52-bit mantissa, sign-flipped). Bits 63..62 = `11`: every other inline subtype, 4-bit subtype in bits 61..58 and a 58-bit payload. Integers widen to 58 bits; decimals become fixed-point with 9 fractional digits; dateTime and date keep a has-timezone bit below the instant.

</option>

<option id="keep-56" outcome="rejected">

Keep 4-bit subtype and 56-bit payload: doubles stay lossy-or-dictionary, so range filters on scores need a value index and churn the dictionary.

</option>

<option id="wide-id" outcome="rejected">

128-bit IDs: every double inline, but every key doubles in size (already rejected in ADR-0011).

</option>

## Decision Outcome

Chosen option: "prefix-code", because every double with an unbiased exponent in [-255, 254] (about 1e-77 to 1e77, plus ±0 and ±infinity) is now inline and exact, integers gain 4x range, and the numeric runs (double, integer, decimal) stay adjacent in key order.

### Consequences

<consequence kind="good">

Embedding scores, weights and timestamps are inline, exact, order-preserving and never interned. The non-literal tags `0x0`..`0x7` are unchanged.

</consequence>

<consequence kind="bad">

No spare kind tags remain above `0x7`; new non-literal kinds must use `0x7` sub-kinds. Only 7 of 16 subtypes in class `11` are assigned. A dateTime keeps its instant and whether it had a timezone, not the original offset: `TZ()` returns `Z` for zoned values (documented SPARQL deviation). NaN, subnormals and out-of-range doubles are dictionary literals.

</consequence>

## More Information

Amends [ADR-0011](0011-id-layout.md) (inline literal layout and numeric adjacency). Specified in `tg.md/02-id-encoding.md`; modelled in `formal/MetaCore/Id.lean` (`encDbl_strictMono`, `double_before_int`, `int_before_decimal`).
