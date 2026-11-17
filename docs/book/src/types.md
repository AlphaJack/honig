# Types

## Request and response types

A schema with `readOnly` or `writeOnly` properties becomes two types:

| Type | Used for | Fields |
|---|---|---|
| `Login` | responses | `token` (`readOnly`). No `password`. |
| `LoginRequest` | request bodies and parameters | `password` (`writeOnly`). No `token`. |

Each is required in Rust where the schema requires it. A missing password is a compile error and not a runtime check. Types that contain one split too. An example is `Session` and `SessionRequest`.

Every struct has `new`. It takes the required properties in document order. A property with a `default` starts at it. Any other optional property starts empty:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/schemas.rs:new}}
```

A property added to the description as required becomes an argument. Code that builds the type stops compiling until it sets it.

A property that is optional and nullable is `api::Nullable<T>`. Its variants are `Absent`, `Null` and `Value(T)`. This is how a `PATCH` body tells "leave it" from "clear it". A property that is only one of required or nullable is `Option<T>`.

A `const` is a unit struct that only decodes its value. Arrays with `minItems == maxItems` are `[T; N]`. The formats `int8` to `uint16` are the Rust integers of the same width.

## Conditional schemas

`if`/`then` blocks that test one property against constants become an enum. It is like a discriminated `oneOf`:

```yaml
Notify:
  type: object
  properties: {type: {type: string}}
  allOf:
    - {if: {properties: {type: {const: email}}}, then: {required: [email], properties: {email: {type: string}}}}
    - {if: {properties: {type: {const: sms}}},   then: {required: [phone], properties: {phone: {type: string}}}}
```

This gives `enum Notify { Email(NotifyEmail), Sms(NotifySms), Otherwise(NotifyOtherwise), Other(Value) }`. `NotifyEmail` has `email: String` as required.

- `Otherwise` holds every other value. Its type has the base properties.
- A lone `if`/`then`/`else` becomes the same kind of enum. `else` is the `Otherwise` variant. `else: false` leaves none.
- An `if` on `enum: [email, mail]` gives one variant for both values: `EmailOrMail`.
- Conditions that cannot be typed, such as ranges or `not`, are still enforced. The server checks them on every request.

`$dynamicRef` generics are monomorphized. `ProductPage: {$ref: Page, $defs: {item: {$dynamicAnchor: item, $ref: Product}}}` becomes a struct with `items: Vec<Product>`. A recursive `Tree<T>` bound to `string` becomes `StringTree { value: String, children: Vec<StringTree> }`.

## Your own derives and types

Generated types take extra derives. Use `"*"` for all types or the Rust name of one type:

```toml
[package.metadata.honig.api.derives]
"*" = ["Hash"]
Order = ["sqlx::FromRow"]
```

Derives the generator already emits are skipped. String enums carry an `Other(String)` variant for unknown values. So derives that need unit-only enums do not fit them. Examples are `sqlx::Type` and `Copy`.

Put `x-rust-type` on the schema to reuse a type that you already have. honig then generates no type for it:

```yaml
Money:
  x-rust-type: crate::domain::Money
  type: object
  ...
```

`types::Money` becomes an alias of your type. It must serialize like the schema. It must implement `Debug`, `Clone`, `PartialEq` and the serde traits. It must also check the constraints of the schema: honig checks none of them, neither where the schema is declared nor where a body or a field uses it.

A type from another crate takes typify's object form, which Oxide's description uses:

```yaml
SledUuid:
  type: string
  format: uuid
  x-rust-type: {crate: omicron-uuid-kinds, version: "*", path: omicron_uuid_kinds::SledUuid}
```

honig uses it only when your crate depends on that crate. Otherwise it warns and generates the type.

## String formats

The `formats` table picks the library type of each string format:

```toml
[package.metadata.honig.api]
formats = { uuid = true, dates = "chrono", url = true, ipnet = true }
```

`dates` is `"chrono"`, `"jiff"` or `"string"`. Each library named must be a dependency with its `serde` feature. `cargo honig init` writes the table from the dependencies the crate has then.

Without the table the dependencies decide: a library with its `serde` feature gives its type. A dependency added later then changes field types, so `generate` warns and prints the line that keeps the current types. It also warns when one of these libraries does not have its `serde` feature, and when both `chrono` and `jiff` are there. Dates then use `chrono`.

{{#include generated/formats.md}}

A format without its library is a `String`. The server checks its text on every request as the library would on parsing. `generate` turns on the runtime feature that each check needs. [Adapters](adapters.md) lists the features.

A library reads more than its format allows. `jiff` and `chrono` read `2024-06-19 08:30Z` as a `date-time`. `uuid` reads `{...}` and `url` reads a space. So a library type in a property, a branch of a union or a newtype is decoded with the same check of its text first. Then the library reads it. A fraction of a second past nine digits is cut: both date libraries hold nanoseconds.

Mail addresses and host names are types of the runtime. Only a valid value can be one of them. `try_from` and decoding check the value. They need the `idn` feature of the runtime. `generate` turns it on when the description uses them:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/schemas.rs:email}}
```

{{#include generated/checked-text.md}}

`http-date` is a `String` checked as one of the three forms of RFC 9110 section 5.6.7. `duration` is a `String` checked as an ISO 8601 duration of RFC 3339 Appendix A. No std type holds `P1M`: a month has no fixed length. `uri-reference`, `iri`, `iri-reference`, `json-pointer`, `relative-json-pointer`, `uri-template` and `regex` are `String`s checked by their grammars (RFC 3986, 3987, 6901, 6570 and ECMA-262). `uuid` without its library takes only the hyphenated form of RFC 9562.

`contentEncoding: base64url` gives `api::Base64Url`. It holds bytes in the URL-safe alphabet.

`{type: number, format: decimal}` and `decimal128` are `api::Decimal`. It keeps the digits of the number. `12345678901234567.89` arrives and leaves as written. An `f64` would round it.
It reads the exact text wherever the number is. This includes `oneOf` branches. It rounds only in the `Other` of a union. That variant holds a value that no branch takes as a `serde_json::Value`.

For arithmetic parse its digits into a decimal type with `d.as_str().parse::<rust_decimal::Decimal>()`.
`{type: string, format: decimal}` is a `String` with every digit. The server checks it as a decimal. Use it past the 28 significant digits that `rust_decimal` holds.

JSON the description leaves untyped is a `serde_json::Value`:

- the properties that an object does not declare with `unknown_fields = "keep"`
- a `{}` schema and an untyped `additionalProperties`
- the `Other` of a union

A number in such a value rounds when it is past `u64`. It also rounds when it has more significant digits than an `f64` holds, which is about 17. RFC 8259 section 6 allows this. A client that reads such an object and writes it back can change that number. To keep a number exact declare it with `format: decimal`.

A string with a `pattern` is a type of its own. Only valid values can be that type. The `regress` feature of the runtime checks it. `generate` turns the feature on when the description uses `pattern`:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/schemas.rs:pattern}}
```

Decoding checks in the same way. So a request with a bad value gets `400`.
