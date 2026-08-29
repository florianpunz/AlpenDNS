Fertiger Meldetext für https://github.com/hickory-dns/hickory-dns/issues/new
(englisch, weil das die Projektsprache dort ist). Vor dem Abschicken kurz
prüfen, ob es die Version noch trifft — eine Suche nach `tsig overflow` ergab
am 2026-08-29 keinen Treffer.

---

**Title:** `Message::from_vec` panics on malformed TSIG record (subtract with overflow in `tsig.rs:387`)

**Body:**

## Summary

Parsing an untrusted DNS message can panic with `attempt to subtract with
overflow` when the message contains a TSIG record whose `RDLENGTH` is smaller
than the fixed fields preceding the MAC size field.

The panic occurs in the *error path* that has already correctly detected the
malformed record.

`hickory-proto` 0.26.1, `src/rr/rdata/tsig.rs:387`:

```rust
let mac_size = decoder
    .read_u16()?
    .verify_unwrap(|&size| decoder.index() + size as usize + 6 <= end_idx)
    .map_err(|size| DecodeError::IncorrectRDataLengthRead {
        read: end_idx - decoder.index(),   // <-- underflows when index() > end_idx
        len: size as usize + 6,
    })?;
```

`end_idx` is derived from `RDLENGTH`. By the time `mac_size` is read, the
decoder has already consumed the algorithm name plus 6 bytes, so
`decoder.index()` can exceed `end_idx` for a small `RDLENGTH`.

## Impact

Depends on the profile the message is parsed in:

| `overflow-checks` | Behaviour |
|---|---|
| on (debug builds, `cargo test`, fuzzing) | panic — denial of service for anything parsing untrusted DNS messages |
| off (default `release`) | no panic; the record is rejected with `incorrect rdata length read`, the wrapped value only reaches the error message |

So release builds are not affected beyond a wrong number in an error string,
but any debug build that parses network data can be taken down by a single
packet.

## Reproducer

```toml
[dependencies]
hickory-proto = "0.26"
```

```rust
#[test]
fn tsig_rdata_length_underflow() {
    // Additional section holds a TSIG record (type 250) whose RDLENGTH is 1 —
    // smaller than the fixed fields that precede the MAC size field.
    let data: &[u8] = &[
        0x29, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x0c, 0x29, 0x00, 0x01, 0x00, 0x00,
        0x00, 0xfa, 0x00, 0x00, 0x0c, 0x00, 0x30, 0xd5, 0x00, 0x01, 0x00, 0x38, 0x00, 0x01, 0x05,
        0x01, 0x00, 0x00, 0x02, 0x08, 0x00, 0x5a,
    ];
    // Expected: Err(..). Actual with overflow checks on: panic.
    let _ = hickory_proto::op::Message::from_vec(data);
}
```

`cargo test` fails with:

```
thread 'tsig_rdata_length_underflow' panicked at
  hickory-proto-0.26.1/src/rr/rdata/tsig.rs:387:23:
attempt to subtract with overflow
```

`cargo test --release` passes.

## Suggested fix

Use a saturating or checked subtraction for the `read` field, e.g.
`end_idx.saturating_sub(decoder.index())`. The value is only used to build the
error message, so saturating is sufficient and cannot mask the rejection.

Found by fuzzing our use of `Message::from_vec` with `cargo-fuzz`.
