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

## Enums and unions

String and integer enums are closed. Each listed value is a variant. Any other value fails `try_from` and decoding. So a request with one gets `400`, and a client reading a response with one gets a decode error. Update the description and generate again when the server adds a value.

A `oneOf` or `anyOf` is closed too: a value that no branch takes fails decoding. A union has an `Other(serde_json::Value)` variant only when the schema itself takes any value. That is a branch of `{}`, or keywords such as `properties` without a `type`. Then any value of another kind is valid.

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

This gives `enum Notify { Email(NotifyEmail), Sms(NotifySms), Otherwise(NotifyOtherwise) }`. `NotifyEmail` has `email: String` as required.

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

Derives the generator already emits are skipped. String and integer enums are closed and `Copy`: one unit variant per listed value. So derives that need unit-only enums fit them, such as `sqlx::Type`.

Put `x-rust-type` on the schema to reuse a type that you already have. honig then generates no type for it:

```yaml
Money:
  x-rust-type: crate::domain::Money
  type: object
  ...
```

`types::Money` becomes an alias of your type. It must serialize like the schema. It must implement `Debug`, `Clone`, `PartialEq` and the serde traits. It must also check the constraints of the schema: honig checks none of them, neither where the schema is declared nor where a body or a field uses it.

A type from another crate takes typify's object form:

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

A format without its library is its checked text, such as `api::UuidText` or `api::DateTimeText`. Only text of the format is one: `try_from`, `parse` and decoding check it as the library would on parsing. It reads as a `str`. `generate` turns on the runtime feature that each check needs. [Adapters](adapters.md) lists the features.

A library reads more than its format allows. `jiff` and `chrono` read `2024-06-19 08:30Z` as a `date-time`. `uuid` reads `{...}` and `url` reads a space. So a library type in a property, a branch of a union, a newtype, a parameter or a body has its text validated first, as a `String` of that format is. The items of a body read as it arrives are validated one at a time. Then the library reads it. A server decoding a request and a client reading a response check alike. A fraction of a second past nine digits is cut: both date libraries hold nanoseconds.

Mail addresses and host names are types of the runtime. Only a valid value can be one of them. `try_from` and decoding check the value. They need the `idn` feature of the runtime. `generate` turns it on when the description uses them:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/schemas.rs:email}}
```

{{#include generated/checked-text.md}}

`http-date` is `api::HttpDateText`, one of the three forms of RFC 9110 section 5.6.7. `duration` is `api::DurationText`, an ISO 8601 duration of RFC 3339 Appendix A. No std type holds `P1M`: a month has no fixed length. `uri-reference`, `iri`, `iri-reference`, `json-pointer`, `relative-json-pointer`, `uri-template` and `regex` are checked text of their grammars (RFC 3986, 3987, 6901, 6570 and ECMA-262). `uuid` without its library takes only the hyphenated form of RFC 9562.

`contentEncoding: base64url` gives `api::Base64Url`. It holds bytes in the URL-safe alphabet.

`{type: number, format: decimal}` and `decimal128` are `api::Decimal`. It keeps the digits of the number. `12345678901234567.89` arrives and leaves as written. An `f64` would round it.
It reads the exact text wherever the number is. This includes `oneOf` branches. It rounds only in the `Other` of an open union. That variant holds a value that no branch takes as a `serde_json::Value`.

For arithmetic parse its digits into a decimal type with `d.as_str().parse::<rust_decimal::Decimal>()`.
`{type: string, format: decimal}` is `api::DecimalText` with every digit, checked as a decimal. `decimal128` keeps every digit too, as a number or as `api::Decimal128Text`, and holds the limits of IEEE 754 decimal128: at most 34 significant digits within its exponent range. Use it past the 28 significant digits that `rust_decimal` holds.

JSON the description leaves untyped is a `serde_json::Value`:

- the properties that an object does not declare with `unknown_fields = "keep"`
- a `{}` schema and an untyped `additionalProperties`
- the `Other` of an open union

A number in such a value rounds when it is past `u64`. It also rounds when it has more significant digits than an `f64` holds, which is about 17. RFC 8259 section 6 allows this. A client that reads such an object and writes it back can change that number. To keep a number exact declare it with `format: decimal`.

## Checked types

A string with a `pattern`, `minLength` or `maxLength` is a type of its own. So is an integer or number with `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum` or `multipleOf`. Only valid values can be that type: `try_from` and decoding check the keywords and return the first one that the value breaks. The `regress` feature of the runtime checks `pattern`. `generate` turns the feature on when the description uses it. A `pattern` that is not an ECMA-262 regular expression fails generation: its type could not check it:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/schemas.rs:pattern}}
```

So a request with a bad value gets `400`, and a client reading a response with one gets a decode error.

Where the field type is known, `try_into` builds the value. A spec `default` is filled in already checked:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/schemas.rs:try-into}}
```

A builder converts every field and returns the first one that does not convert, from `build`. So the whole value needs one `?`. `builder` takes the required properties, so leaving one out is still a compile error. Each setter takes anything that converts to the type of its field: a number for a checked number, a `&str` for a string, a checked string or an enum:

```rust
{{#include ../../../tests/e2e/tests/v32_validation.rs:builder}}
```

The error names the field as the description spells it: `` `age`: 150 must be less than 150 ``. The builder of `User` is `types::builders::User`.

`try_from`, `parse` and `build` all fail with `api::BuildError`, so a function that mixes them needs no `Box<dyn Error>`. In a handler, `?` turns it into a `500`: the handler's inputs are checked already, so a value that its schema refuses came from the server's own data.

A checked type reads as its value: a string one derefs to `str` and compares with `&str`, a number one derefs to its number and compares with it. `get` and `into_inner` return the value, and `parse` reads one from text.

Integer keywords are exact. A fractional bound rounds inward, `multipleOf: 2.5` takes the integers that are multiples of 5, and a bound that no value of the type meets refuses every value: `maximum: -1` on a `uint8` leaves none.

## Whole schemas

Some keywords say what no Rust type can: `not`, `contains`, `dependentRequired`, `if`/`then` beyond the constants of one property, the one branch that a `oneOf` takes, tuples and `$dynamicRef`. A struct or union whose own schema has them is a checked type of its shape: `Product` holds a `ProductFields`, `Vehicle` holds a `VehicleVariants`. It reads as its shape. `try_from` and decoding check the whole schema. Its builder fills the fields and checks the whole value in `build`:

```rust
{{#include ../../../tests/e2e/tests/v32_kitchen_sink.rs:whole-schema}}
```

A property, parameter or body with such keywords of its own has a checked type named after it, such as `ProductAttrs`. A type of yours (`x-rust-type`) is never wrapped: it keeps its own invariants.
